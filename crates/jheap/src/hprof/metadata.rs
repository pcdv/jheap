//! Pass 1: the strings, classes and roots, and the id of every object.

use std::io::{Read, Seek};

use super::Error;
use super::cursor::Cursor;
use super::tags::*;
use crate::blocks::Blocks;
use crate::hash::FxHashMap;

/// A class as its CLASS_DUMP describes it: what the graph needs of it.
pub(crate) struct Class {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// 0 for java.lang.Object.
    pub(crate) super_id: u64,
    pub(crate) loader: u64,
    pub(crate) signers: u64,
    pub(crate) domain: u64,
    /// The objects its static fields hold.
    pub(crate) statics: Vec<u64>,
    /// The objects its constant pool holds.
    pub(crate) constants: Vec<u64>,
    /// The types of its own instance fields, in the order they are written.
    pub(crate) fields: Vec<u8>,
}

/// The classes in the order the dump first describes them. A class described
/// twice keeps its place and takes the later description.
#[derive(Default)]
pub(crate) struct Classes {
    pub(crate) list: Vec<Class>,
    position: FxHashMap<u64, u32>,
}

impl Classes {
    fn set(&mut self, class: Class) {
        match self.position.get(&class.id) {
            Some(&at) => self.list[at as usize] = class,
            None => {
                self.position.insert(class.id, self.list.len() as u32);
                self.list.push(class);
            }
        }
    }

    #[inline]
    pub(crate) fn position(&self, id: u64) -> Option<u32> {
        self.position.get(&id).copied()
    }

    #[inline]
    pub(crate) fn get(&self, id: u64) -> Option<&Class> {
        self.position(id).map(|at| &self.list[at as usize])
    }
}

/// What pass 1 finds.
pub(crate) struct Metadata {
    pub(crate) version: String,
    pub(crate) id_size: usize,
    pub(crate) string_count: usize,
    pub(crate) classes: Classes,
    /// The object each root names, and the root's tag, in the order written.
    pub(crate) roots: Vec<(u64, u8)>,
    /// Where each heap dump or segment's body is, and its length.
    pub(crate) segments: Vec<(u64, u64)>,
    /// Bytes between the lowest and the highest object address sampled.
    pub(crate) heap_span: u64,
    /// The id of every object record, in the order written: an object's index
    /// in the graph is its place here.
    pub(crate) ids: Blocks<u64>,
}

/// The lowest and the highest object id seen, and how many objects went by.
///
/// The span is sampled, one id in 64, as heap-landscape's parser did: it only
/// has to tell a heap of a few GB from one over 32, and the expected parses
/// record the sampled span.
struct Span {
    low: u64,
    high: u64,
    seen: u64,
}

impl Span {
    #[inline]
    fn widen(&mut self, id: u64) {
        if self.seen & 63 == 0 {
            self.low = self.low.min(id);
            self.high = self.high.max(id);
        }
        self.seen += 1;
    }
}

pub(crate) fn read_metadata<R: Read + Seek>(
    r: &mut Cursor<R>,
    progress: &mut dyn FnMut(u32),
) -> Result<Metadata, Error> {
    let size = r.size();
    let mut strings: FxHashMap<u64, String> = FxHashMap::default();
    let mut class_names: FxHashMap<u64, String> = FxHashMap::default();
    let mut classes = Classes::default();
    let mut roots = Vec::new();
    let mut segments = Vec::new();
    let mut span = Span { low: u64::MAX, high: 0, seen: 0 };
    let mut ids = Blocks::new();

    r.seek(0);
    r.need(31)?;
    let (version, id_size, header_size) = parse_header(r.window())?;
    r.id_size = id_size;
    r.seek(header_size);
    let mut last_progress = 0;

    while r.offset() < size {
        let offset = r.offset();
        r.need(9)?;
        let tag = r.u1();
        r.skip(4);
        let length = r.u4() as u64;
        let body = offset + 9;
        // A record out of step reads garbage as tags and lengths: stop at the
        // first one that cannot be, rather than walk to the end and report an
        // empty heap.
        if !RECORD_TAGS.contains(&tag) {
            return Err(Error::Format(format!(
                "Unknown record tag 0x{tag:x} at offset {offset}: the file is corrupt, \
                 or an earlier record has the wrong length."
            )));
        }
        if body + length > size {
            return Err(Error::Format(format!(
                "Truncated file: the record at offset {offset} is {length} bytes long, \
                 {} more than the file has.",
                body + length - size
            )));
        }

        match tag {
            STRING_IN_UTF8 => {
                r.need((length as usize).max(id_size))?;
                let id = r.id();
                let text = r.peek((length as usize).saturating_sub(id_size));
                strings.insert(id, String::from_utf8_lossy(text).into_owned());
            }
            LOAD_CLASS => {
                // serial, class object id, stack serial, name string id
                r.need((length as usize).max(8 + 2 * id_size))?;
                r.skip(4);
                let class_id = r.id();
                r.skip(4);
                let name_id = r.id();
                let name = match strings.get(&name_id) {
                    Some(name) => format_class_name(name),
                    None => format!("<unknown:{name_id}>"),
                };
                class_names.insert(class_id, name);
            }
            HEAP_DUMP | HEAP_DUMP_SEGMENT => {
                segments.push((body, length));
                // the roots and classes of this segment, its objects skipped unread
                let end = body + length;
                while r.offset() < end {
                    r.need(1)?;
                    let sub = r.u1();
                    if let Some(tail) = root_tail(sub, id_size) {
                        r.need(id_size)?;
                        roots.push((r.id(), sub));
                        r.skip(tail);
                        continue;
                    }
                    match sub {
                        CLASS_DUMP => {
                            let mut class = read_class(r)?;
                            class.name = class_names
                                .get(&class.id)
                                .cloned()
                                .unwrap_or_else(|| format!("<class:{}>", class.id));
                            classes.set(class);
                        }
                        INSTANCE_DUMP => {
                            // id, stack serial, class id, then the field bytes
                            r.need(2 * id_size + 8)?;
                            let id = r.id();
                            span.widen(id);
                            ids.push(id);
                            r.skip(id_size as u64 + 4);
                            let n = r.u4() as u64;
                            r.skip(n);
                        }
                        OBJECT_ARRAY_DUMP => {
                            // id, stack serial, length, array class id, then the elements
                            r.need(id_size + 8)?;
                            let id = r.id();
                            span.widen(id);
                            ids.push(id);
                            r.skip(4);
                            let n = r.u4() as u64;
                            r.skip(id_size as u64 + n * id_size as u64);
                        }
                        PRIMITIVE_ARRAY_DUMP => {
                            // id, stack serial, length, element type, then the elements
                            r.need(id_size + 9)?;
                            let id = r.id();
                            span.widen(id);
                            ids.push(id);
                            r.skip(4);
                            let n = r.u4() as u64;
                            let t = r.u1();
                            r.skip(n * type_size(t));
                        }
                        _ => return Err(unknown_sub_record(sub, r.offset() - 1)),
                    }
                }
            }
            _ => {}
        }

        r.seek(body + length);
        let pct = (100 * r.offset() / size.max(1)) as u32;
        if pct > last_progress {
            last_progress = pct;
            progress(pct);
        }
    }

    Ok(Metadata {
        version,
        id_size,
        string_count: strings.len(),
        classes,
        roots,
        segments,
        heap_span: span.high.saturating_sub(span.low),
        ids,
    })
}

/// The version, the width of ids, and where the records start.
fn parse_header(bytes: &[u8]) -> Result<(String, usize, u64), Error> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(Error::Format(
            "The file is gzipped: the parser seeks, so it reads a dump unzipped.".into(),
        ));
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let version = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if !version.starts_with("JAVA PROFILE") {
        let start: String = version.chars().take(20).collect();
        return Err(Error::Format(format!("Not an HPROF file: {start}")));
    }
    if end + 13 > bytes.len() {
        return Err(Error::Format("Not an HPROF file: its header is cut short".into()));
    }
    let id_size = u32::from_be_bytes(bytes[end + 1..end + 5].try_into().unwrap());
    if id_size != 4 && id_size != 8 {
        return Err(Error::Format(format!(
            "Identifiers of {id_size} bytes: an HPROF file has 4 or 8, so this one is corrupt."
        )));
    }
    // then a timestamp of 8 bytes
    Ok((version, id_size as usize, end as u64 + 13))
}

pub(crate) fn unknown_sub_record(tag: u8, offset: u64) -> Error {
    Error::Format(format!("Unknown heap dump sub-record tag 0x{tag:x} at offset {offset}"))
}

/// A CLASS_DUMP as written, the cursor just past its tag: the ids it names,
/// its static and constant references, the types of its instance fields. Read
/// in both passes, kept in the first.
pub(crate) fn read_class<R: Read + Seek>(r: &mut Cursor<R>) -> Result<Class, Error> {
    let id_size = r.id_size;
    // id, stack serial, super, loader, signers, protection domain, two
    // reserved, instance size, constant pool size
    r.need(id_size * 7 + 4 + 4 + 2)?;
    let id = r.id();
    r.skip(4);
    let super_id = r.id();
    let loader = r.id();
    let signers = r.id();
    let domain = r.id();
    r.skip(2 * id_size as u64);
    r.skip(4); // instance size: the fields say it, at the JVM's widths

    /// A value of a type, as the constant pool and the static fields hold
    /// them: an object's id goes to `refs`.
    fn value<R: Read + Seek>(r: &mut Cursor<R>, refs: &mut Vec<u64>) -> Result<(), Error> {
        r.need(1)?;
        let t = r.u1();
        let size = if t == TYPE_OBJECT { r.id_size } else { type_size(t) as usize };
        r.need(size)?;
        if t != TYPE_OBJECT {
            r.skip(size as u64);
        } else {
            let id = r.id();
            if id != 0 {
                refs.push(id);
            }
        }
        Ok(())
    }

    // Constant pool: the object entries are resolved constants (strings,
    // classes) that the class keeps alive
    let mut constants = Vec::new();
    let count = r.u2();
    for _ in 0..count {
        r.need(2)?;
        r.skip(2); // index
        value(r, &mut constants)?;
    }

    // Static fields: the object ones are the class's own references, and
    // where most of a heap hangs from
    let mut statics = Vec::new();
    r.need(2)?;
    let count = r.u2();
    for _ in 0..count {
        r.need(id_size)?;
        r.skip(id_size as u64); // name
        value(r, &mut statics)?;
    }

    // Instance fields: their types, the layout of every instance
    r.need(2)?;
    let count = r.u2();
    let mut fields = Vec::with_capacity(count as usize);
    for _ in 0..count {
        r.need(id_size + 1)?;
        r.skip(id_size as u64); // name
        fields.push(r.u1());
    }

    Ok(Class {
        id,
        name: String::new(),
        super_id,
        loader,
        signers,
        domain,
        statics,
        constants,
        fields,
    })
}
