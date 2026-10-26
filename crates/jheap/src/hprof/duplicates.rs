//! Strings of equal value held in several copies, and what keeping one copy
//! of each would save.
//!
//! Pass 2 hashes the contents of every `byte[]` and `char[]` as it streams
//! past, and notes every String with the index of its value: the contents
//! themselves are never kept. Before the walk, each String is keyed by its
//! value's hash and coder, and the hashes are dropped. After it, the reachable
//! Strings are sorted by key: the copies of a value lie together, and the
//! walk's tree says where each is held. Only the values shown are read again
//! from the dump, for their text.
//!
//! Two Strings are copies when their values are equal, whether in two arrays
//! or in one: G1's string deduplication makes copies share an array, and then
//! only the String objects are repeated. What keeping one copy saves is every
//! String but one, and every array but one.

use std::io::{Read, Seek};

use super::cursor::Cursor;
use super::graph::Graph;
use super::metadata::{StringClass, read_class, unknown_sub_record};
use super::tags::*;
use super::walk::Tree;
use super::{Error, Path};
use crate::blocks::Blocks;
use crate::hash::{ContentHasher, FxHashMap};

/// An object record in this many has its offset noted, so that an array can be
/// read again from the note before it: a few records skipped, not the dump.
const CHECKPOINT: usize = 256;

/// The values named in the profile, the most wasteful first: the others share
/// a frame.
const NAMED: usize = 1000;

/// The characters of a value kept for its text.
const TEXT: usize = 200;

/// The characters of a value shown in its frame.
const FRAME_TEXT: usize = 60;

/// The bytes read at a time when values are read again, scattered across the
/// dump: a window of the parse's size would read megabytes for each.
const READ_BACK: usize = 64 << 10;

/// The frame of the values not named.
pub(crate) const OTHERS: &str = "(other duplicated strings)";

/// Values in more than one copy, the copies' paths, and what they cost.
#[derive(Debug, Clone, Default)]
pub struct Duplicates {
    /// The frames' names: the parse's, then the values'.
    pub names: Vec<String>,
    /// Each String's path from its root, then its value. A copy weighs its
    /// share of what keeping one copy of its value would save, the same for
    /// every copy, and counts as one object.
    pub paths: Vec<Path>,
    /// The values the profile names, the most wasteful first.
    pub values: Vec<Value>,
    /// Values held in more than one copy, those not named included.
    pub count: u64,
    /// Their copies.
    pub copies: u64,
    /// Bytes of the copies, Strings and arrays.
    pub bytes: u64,
    /// Bytes keeping one copy of each value would save.
    pub saving: u64,
    /// Reading the values, after the walk: hashing them in pass 2 is not counted.
    pub time_ms: f64,
}

/// A value held in more than one copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// Its first characters, up to 200.
    pub text: String,
    /// All its characters.
    pub length: u64,
    /// Strings that hold it.
    pub copies: u64,
    /// Distinct arrays among them: fewer than the copies when they share one.
    pub arrays: u64,
    /// Bytes of the Strings and of the distinct arrays.
    pub bytes: u64,
    /// Bytes keeping one String and one array would save.
    pub saving: u64,
    /// Its frame, in [`Duplicates::names`].
    pub frame: u32,
}

/// A String, and its value's key once known.
#[derive(Clone, Copy, Default)]
pub(crate) struct Str {
    /// The value's hash with the coder in its low bit, or the coder alone until
    /// the hashes are known.
    key: u64,
    index: u32,
    value: u32,
}

impl Str {
    fn utf16(&self) -> bool {
        self.key & 1 == 1
    }
}

/// What pass 2 gathers, when the duplicates are asked for. It is held at the
/// parse's peak, with the id index, so it is kept small: 8 bytes an array
/// hashed and 8 a String, where a column of indices beside the hashes and the
/// coder beside each String would take 12 and 16.
pub(crate) struct Gather {
    string: Option<StringClass>,
    segments: Vec<(u64, u64)>,
    /// A bit per object record, set for the arrays hashed: every `byte[]`
    /// and `char[]`.
    arrays: Vec<u64>,
    /// Their hashes, in the order of the records.
    hashes: Blocks<u64>,
    /// Every String's index and its value's.
    strings: Blocks<(u32, u32)>,
    /// A bit per String, set for a UTF-16 one.
    utf16: Vec<u64>,
    /// The offset of every CHECKPOINT-th object record, and its segment.
    checkpoints: Vec<(u64, u32)>,
}

impl Gather {
    pub(crate) fn new(string: Option<StringClass>, segments: Vec<(u64, u64)>, records: usize) -> Self {
        Gather {
            string,
            segments,
            arrays: vec![0; records.div_ceil(64)],
            hashes: Blocks::new(),
            strings: Blocks::new(),
            utf16: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    /// Notes where object record `rec` starts, if it is a checkpoint's.
    #[inline]
    pub(crate) fn mark(&mut self, rec: usize, offset: u64, segment: usize) {
        if rec.is_multiple_of(CHECKPOINT) {
            self.checkpoints.push((offset, segment as u32));
        }
    }

    #[inline]
    pub(crate) fn is_string(&self, class_id: u64) -> bool {
        self.string.is_some_and(|s| s.id == class_id)
    }

    /// A String, from its field bytes; `find` gives the index of an id.
    #[inline]
    pub(crate) fn string(&mut self, rec: usize, bytes: &[u8], id_size: usize, find: impl Fn(u64) -> Option<u32>) {
        let Some(s) = self.string else { return };
        let at = s.value_at as usize;
        if at + id_size > bytes.len() {
            return;
        }
        let id = super::cursor::read_id(&bytes[at..], id_size);
        let Some(value) = (id != 0).then(|| find(id)).flatten() else { return };
        // LATIN1 is 0 and UTF16 1; a String before JDK 9 has no coder
        let coder = s.coder_at.and_then(|at| bytes.get(at as usize)).map_or(0, |&c| (c & 1) as u64);
        let k = self.strings.len();
        if k.is_multiple_of(64) {
            self.utf16.push(0);
        }
        self.utf16[k / 64] |= coder << (k % 64);
        self.strings.push((rec as u32, value));
    }

    /// Whether an array of that type is hashed.
    #[inline]
    pub(crate) fn hashes(&self, t: u8) -> bool {
        self.string.is_some() && (t == TYPE_BYTE || t == TYPE_CHAR)
    }

    /// A hasher for the contents of an array of that type.
    #[inline]
    pub(crate) fn hasher(t: u8) -> ContentHasher {
        ContentHasher::new(t as u64)
    }

    #[inline]
    pub(crate) fn array(&mut self, rec: usize, hash: u64) {
        self.arrays[rec / 64] |= 1 << (rec % 64);
        self.hashes.push(hash);
    }

    /// Each String keyed by its value, those whose value was not hashed left
    /// out, and the hashes dropped: done before the walk, so that they are
    /// not held through it.
    pub(crate) fn resolve(self) -> Resolved {
        let Gather { segments, arrays, hashes, strings, utf16, checkpoints, .. } = self;
        let mut copies = Vec::with_capacity(strings.len());
        copies.extend(
            strings
                .into_values()
                .enumerate()
                .map(|(k, (index, value))| Str { key: utf16[k / 64] >> (k % 64) & 1, index, value }),
        );
        drop(utf16);
        // a String's value mostly follows it in the dump, so this is nearly in order
        copies.sort_unstable_by_key(|c: &Str| c.value);

        // An array's hash is the nth, n the arrays hashed before it: the bits
        // set below its own, counted as the Strings come in order of value
        let mut hashes = hashes.into_values();
        let (mut word, mut below) = (0usize, 0u64);
        let mut last: Option<(u64, u64)> = None;
        copies.retain_mut(|c| {
            let v = c.value as usize;
            // an id no record defines, or a class object
            if v / 64 >= arrays.len() || arrays[v / 64] >> (v % 64) & 1 == 0 {
                return false;
            }
            while word < v / 64 {
                below += arrays[word].count_ones() as u64;
                word += 1;
            }
            let nth = below + (arrays[word] & ((1u64 << (v % 64)) - 1)).count_ones() as u64;
            let hash = match last {
                // an array the String before shares
                Some((n, hash)) if n == nth => hash,
                _ => {
                    let skip = last.map_or(nth, |(n, _)| nth - n - 1);
                    let hash = hashes.nth(skip as usize).expect("a hash for every bit");
                    last = Some((nth, hash));
                    hash
                }
            };
            c.key |= hash & !1;
            true
        });
        copies.shrink_to_fit();
        Resolved { copies, segments, checkpoints }
    }
}

/// The Strings keyed by their values, waiting for the walk.
pub(crate) struct Resolved {
    copies: Vec<Str>,
    segments: Vec<(u64, u64)>,
    checkpoints: Vec<(u64, u32)>,
}

/// A run of copies of one value, in the sorted copies.
struct Group {
    start: u32,
    end: u32,
    saving: u64,
}

impl Resolved {
    /// The duplicates, from the graph after the walk: `frame` holds each
    /// reached object's node in `tree`, and `seen` the objects reached.
    pub(crate) fn finish<R: Read + Seek>(
        self,
        r: &mut Cursor<R>,
        g: &Graph,
        tree: &Tree,
        seen: &[u64],
    ) -> Result<Duplicates, Error> {
        let Resolved { mut copies, segments, checkpoints } = self;
        let bytes = |i: u32| g.size[i as usize] as u64 * 8;

        // What nothing reaches is garbage the next collection takes: its
        // copies cost nothing for long
        copies.retain(|c| seen[c.index as usize / 64] & (1u64 << (c.index % 64)) != 0);
        copies.sort_unstable_by_key(|c| (c.key, c.value));

        let mut out = Duplicates::default();
        let mut groups = Vec::new();
        let mut start = 0;
        while start < copies.len() {
            let key = copies[start].key;
            let mut end = start + 1;
            while end < copies.len() && copies[end].key == key {
                end += 1;
            }
            if end - start > 1 {
                let run = &copies[start..end];
                let (strings, arrays) = cost(run, bytes);
                let total = strings + arrays;
                let saving = total - bytes(run[0].index) - bytes(run[0].value);
                out.count += 1;
                out.copies += run.len() as u64;
                out.bytes += total;
                out.saving += saving;
                groups.push(Group { start: start as u32, end: end as u32, saving });
            }
            start = end;
        }

        // The most wasteful named, the more copies first between equals, then
        // by key, so that the same dump names the same values
        let order = |a: &Group, b: &Group| {
            b.saving.cmp(&a.saving).then((b.end - b.start).cmp(&(a.end - a.start))).then(
                copies[a.start as usize].key.cmp(&copies[b.start as usize].key),
            )
        };
        let mut named: Vec<usize> = (0..groups.len()).collect();
        if named.len() > NAMED {
            named.select_nth_unstable_by(NAMED, |&a, &b| order(&groups[a], &groups[b]));
            named.truncate(NAMED);
        }
        named.sort_by(|&a, &b| order(&groups[a], &groups[b]));

        // Their texts, read in the order of the dump
        let mut texts: Vec<(String, u64)> = vec![(String::new(), 0); named.len()];
        let mut reads: Vec<(u32, bool, usize)> = named
            .iter()
            .enumerate()
            .map(|(k, &grp)| {
                let first = copies[groups[grp].start as usize];
                (first.value, first.utf16(), k)
            })
            .collect();
        reads.sort_unstable();
        let window = r.set_chunk(READ_BACK);
        for (value, utf16, k) in reads {
            texts[k] = read_value(r, &segments, &checkpoints, value, utf16)?;
        }
        r.set_chunk(window);

        out.names = g.names.list.clone();
        let mut frame_of: FxHashMap<usize, u32> = FxHashMap::default();
        let mut taken: FxHashMap<String, u32> = FxHashMap::default();
        for (k, &grp) in named.iter().enumerate() {
            let (text, length) = std::mem::take(&mut texts[k]);
            let mut name = frame_name(&text, length);
            let n = taken.entry(name.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                // two values that start alike, cut at the same place
                name = format!("{name} #{n}");
            }
            let frame = out.names.len() as u32;
            out.names.push(name);
            frame_of.insert(grp, frame);
            let group = &groups[grp];
            let run = &copies[group.start as usize..group.end as usize];
            let (strings, arrays) = cost(run, bytes);
            out.values.push(Value {
                text,
                length,
                copies: run.len() as u64,
                arrays: distinct(run),
                bytes: strings + arrays,
                saving: group.saving,
                frame,
            });
        }
        let others = out.names.len() as u32;
        out.names.push(OTHERS.into());

        // Each copy's share, gathered by its node and value before the paths
        // are spelled out: there are far fewer of those than copies
        let mut held: FxHashMap<(u32, u32), (u64, u64)> = FxHashMap::default();
        for (k, group) in groups.iter().enumerate() {
            let value = frame_of.get(&k).copied().unwrap_or(others);
            let run = &copies[group.start as usize..group.end as usize];
            let n = run.len() as u64;
            let (share, extra) = (group.saving / n, group.saving % n);
            for (j, c) in run.iter().enumerate() {
                let node = g.frame[c.index as usize];
                let entry = held.entry((node, value)).or_default();
                entry.0 += share + ((j as u64) < extra) as u64;
                entry.1 += 1;
            }
        }
        let mut paths: FxHashMap<Box<[u32]>, (u64, u64)> = FxHashMap::default();
        for ((node, value), (bytes, objects)) in held {
            let mut frames = tree.frames_of(node).into_vec();
            frames.push(value);
            let entry = paths.entry(frames.into_boxed_slice()).or_default();
            entry.0 += bytes;
            entry.1 += objects;
        }
        out.paths = paths
            .into_iter()
            .filter(|&(_, (bytes, _))| bytes > 0)
            .map(|(frames, (bytes, objects))| Path { frames: frames.into_vec(), bytes, objects })
            .collect();
        let names = &out.names;
        out.paths.sort_by_cached_key(|p| p.frames.iter().map(|&f| names[f as usize].as_str()).collect::<Vec<_>>().join(";"));
        Ok(out)
    }
}

/// Bytes of a run's Strings, and of its distinct arrays: the run is sorted by
/// value, so an array shared is a value repeated.
fn cost(run: &[Str], bytes: impl Fn(u32) -> u64) -> (u64, u64) {
    let strings = run.iter().map(|c| bytes(c.index)).sum();
    let arrays = run.iter().enumerate().filter(|&(j, c)| j == 0 || run[j - 1].value != c.value).map(|(_, c)| bytes(c.value)).sum();
    (strings, arrays)
}

fn distinct(run: &[Str]) -> u64 {
    run.iter().enumerate().filter(|&(j, c)| j == 0 || run[j - 1].value != c.value).count() as u64
}

/// The text of a String's value and its length in characters, its array read
/// again from the dump: from the checkpoint before it, record by record.
fn read_value<R: Read + Seek>(
    r: &mut Cursor<R>,
    segments: &[(u64, u64)],
    checkpoints: &[(u64, u32)],
    index: u32,
    utf16: bool,
) -> Result<(String, u64), Error> {
    let id_size = r.id_size;
    let index = index as usize;
    let (at, segment) = checkpoints[index / CHECKPOINT];
    let (mut rec, mut segment) = (index / CHECKPOINT * CHECKPOINT, segment as usize);
    r.seek(at);
    loop {
        let (start, length) = segments[segment];
        if r.offset() >= start + length {
            segment += 1;
            if segment == segments.len() {
                return Err(Error::Format(format!("object {index} is past the last heap segment")));
            }
            r.seek(segments[segment].0);
            continue;
        }
        r.need(1)?;
        let sub = r.u1();
        match sub {
            INSTANCE_DUMP => {
                r.need(2 * id_size + 8)?;
                r.skip(2 * id_size as u64 + 4);
                let n = r.u4() as u64;
                r.skip(n);
            }
            OBJECT_ARRAY_DUMP => {
                r.need(id_size + 8)?;
                r.skip(id_size as u64 + 4);
                let n = r.u4() as u64;
                r.skip((n + 1) * id_size as u64);
            }
            PRIMITIVE_ARRAY_DUMP => {
                r.need(id_size + 9)?;
                r.skip(id_size as u64 + 4);
                let n = r.u4() as u64;
                let t = r.u1();
                if rec == index {
                    return decode(r, t, n, utf16);
                }
                r.skip(n * type_size(t));
            }
            CLASS_DUMP => {
                read_class(r)?;
                continue;
            }
            _ => match root_tail(sub, id_size) {
                Some(tail) => {
                    r.skip(id_size as u64 + tail);
                    continue;
                }
                None => return Err(unknown_sub_record(sub, r.offset() - 1)),
            },
        }
        if rec == index {
            return Err(Error::Format(format!("object {index} was a String's value, and is not an array")));
        }
        rec += 1;
    }
}

/// The first characters of an array of `n` elements of type `t`, the cursor at
/// its elements, and its length in characters. A `byte[]` is LATIN1, or UTF-16
/// in the JVM's order, little-endian on the machines that run one; a `char[]`
/// is UTF-16 as the dump writes every value, big-endian.
fn decode<R: Read + Seek>(r: &mut Cursor<R>, t: u8, n: u64, utf16: bool) -> Result<(String, u64), Error> {
    let wide = t == TYPE_CHAR || utf16;
    let length = if t == TYPE_BYTE && utf16 { n / 2 } else { n };
    let read = (length.min(TEXT as u64) * if wide { 2 } else { 1 }) as usize;
    r.need(read)?;
    let bytes = r.peek(read);
    let text = if !wide {
        bytes.iter().map(|&b| b as char).collect()
    } else {
        let unit = |&pair: &[u8; 2]| if t == TYPE_CHAR { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) };
        char::decode_utf16(bytes.as_chunks::<2>().0.iter().map(unit))
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    };
    Ok((text, length))
}

/// A value as a frame: quoted, escaped as Java writes a string, and cut short.
/// `;` is escaped too: it separates the frames of a path where a path is one
/// string, here and in the page.
fn frame_name(text: &str, length: u64) -> String {
    let mut name = String::from("\"");
    let mut shown = 0;
    for c in text.chars() {
        if shown == FRAME_TEXT {
            break;
        }
        match c {
            '"' => name.push_str("\\\""),
            '\\' => name.push_str("\\\\"),
            '\n' => name.push_str("\\n"),
            '\r' => name.push_str("\\r"),
            '\t' => name.push_str("\\t"),
            ';' => name.push_str("\\u003b"),
            c if c.is_control() => name.push_str(&format!("\\u{:04x}", c as u32)),
            c => name.push(c),
        }
        shown += 1;
    }
    if (shown as u64) < length {
        name.push_str(&format!("…\" ({length} chars)"));
    } else {
        name.push('"');
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_quoted_escaped_and_cut() {
        assert_eq!(frame_name("", 0), "\"\"");
        assert_eq!(frame_name("a\"b\\c\nd;e\u{1}", 9), "\"a\\\"b\\\\c\\nd\\u003be\\u0001\"");
        let long = "x".repeat(300);
        assert_eq!(frame_name(&long[..TEXT], 300), format!("\"{}…\" (300 chars)", "x".repeat(FRAME_TEXT)));
    }
}
