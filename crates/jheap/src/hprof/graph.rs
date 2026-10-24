//! Pass 2: the objects, what each weighs and what it holds.

use std::io::{Read, Seek};

use super::cursor::{Cursor, read_id};
use super::index::IdIndex;
use super::metadata::{Metadata, read_class, unknown_sub_record};
use super::tags::*;
use super::{Error, Layout};
use crate::blocks::Blocks;
use crate::hash::FxHashMap;

/// The heap as columns, an entry per object, rather than a structure each:
/// the heap is millions of small things, and what each costs sets how large a
/// dump can be read.
///
/// An object is an index, from 0: the object records in the order written,
/// then the class objects. References to an id that no record defines get
/// indices past those, `defined..total`: they weigh nothing and hold nothing,
/// but the walk still reaches them, as it did in heap-landscape.
pub(crate) struct Graph {
    /// Records and class objects.
    pub(crate) defined: u32,
    /// With the ids only references name.
    pub(crate) total: u32,
    /// Each object's frame: its class's name, in `names`.
    pub(crate) frame: Vec<u32>,
    /// What each object weighs, in units of 8 bytes, as objects are aligned on 8.
    pub(crate) size: Vec<u32>,
    /// Where each object's references start in `refs`; one more for the end.
    pub(crate) offsets: Offsets,
    /// The objects each holds, by index.
    pub(crate) refs: Blocks<u32>,
    pub(crate) names: Names,
    /// Each root's frame, and the object it names, in the order written.
    pub(crate) roots: Vec<(u32, u32)>,
    pub(crate) instance_count: u64,
    pub(crate) object_array_count: u64,
    pub(crate) primitive_array_count: u64,
}

/// Where references start: 32 bits until there are more than that many.
pub(crate) enum Offsets {
    Narrow(Vec<u32>),
    Wide(Vec<u64>),
}

impl Offsets {
    #[inline]
    fn set(&mut self, i: usize, at: usize) {
        match self {
            Offsets::Narrow(o) if at <= u32::MAX as usize => o[i] = at as u32,
            Offsets::Narrow(o) => {
                let mut wide: Vec<u64> = o.iter().map(|&x| x as u64).collect();
                wide[i] = at as u64;
                *self = Offsets::Wide(wide);
            }
            Offsets::Wide(o) => o[i] = at as u64,
        }
    }

    /// The references of object i, as a range of `refs`.
    #[inline]
    pub(crate) fn range(&self, i: usize) -> (usize, usize) {
        match self {
            Offsets::Narrow(o) => (o[i] as usize, o[i + 1] as usize),
            Offsets::Wide(o) => (o[i] as usize, o[i + 1] as usize),
        }
    }
}

/// The names of frames: classes, roots, array types.
#[derive(Default)]
pub(crate) struct Names {
    pub(crate) list: Vec<String>,
    index: FxHashMap<String, u32>,
}

impl Names {
    pub(crate) fn intern(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(name.to_string());
        self.index.insert(name.to_string(), i);
        i
    }
}

/// From an id, as a reference names it, to an object.
struct Resolver {
    records: IdIndex,
    /// Class objects by id, to their index.
    classes: FxHashMap<u64, u32>,
    /// A class object has the id of a record, which it then shadows: the
    /// classes are looked at first.
    collide: bool,
    /// Ids no record defines, given an index when first named.
    undefined: FxHashMap<u64, u32>,
    next: u64,
}

impl Resolver {
    #[inline]
    fn find(&self, id: u64) -> Option<u32> {
        if self.collide
            && let Some(&i) = self.classes.get(&id)
        {
            return Some(i);
        }
        if let Some(i) = self.records.find(id) {
            return Some(i);
        }
        if let Some(&i) = self.classes.get(&id) {
            return Some(i);
        }
        self.undefined.get(&id).copied()
    }

    #[inline]
    fn resolve(&mut self, id: u64) -> u32 {
        match self.find(id) {
            Some(i) => i,
            None => {
                // past u32::MAX the count is caught when the graph is done
                let i = self.next.min(u32::MAX as u64) as u32;
                self.next += 1;
                self.undefined.insert(id, i);
                i
            }
        }
    }
}

/// Where the references of an instance are in its field bytes, and what it
/// weighs in the JVM: the same for every instance of a class, so worked out
/// once per class.
struct Shape {
    /// In the dump, which writes a reference at the width of an id.
    offsets: Box<[u32]>,
    /// In units of 8 bytes; None for a class the dump does not describe, whose
    /// instances weigh their bytes as written.
    size: Option<u32>,
    frame: u32,
}

/// Bytes rounded up to the 8 objects are aligned on, in units of 8.
#[inline]
fn aligned(n: u64) -> u32 {
    n.div_ceil(8).min(u32::MAX as u64) as u32
}

pub(crate) fn build_graph<R: Read + Seek>(
    r: &mut Cursor<R>,
    meta: Metadata,
    layout: &Layout,
    chunk: usize,
    progress: &mut dyn FnMut(u32),
) -> Result<Graph, Error> {
    let Metadata { classes, roots, segments, ids, .. } = meta;
    let id_size = r.id_size;
    let records = ids.len();
    let defined = records as u64 + classes.list.len() as u64;
    if defined >= u32::MAX as u64 {
        return Err(too_many(defined));
    }

    // The frames of the classes first, in the order of the classes
    let mut names = Names::default();
    let class_frames: Vec<u32> = classes.list.iter().map(|c| names.intern(&c.name)).collect();

    let (index, mut shadowed) = IdIndex::build(ids);
    let mut collide = false;
    let mut class_index = FxHashMap::default();
    for (k, class) in classes.list.iter().enumerate() {
        if let Some(record) = index.find(class.id) {
            // the class object comes later, so it is the object
            collide = true;
            shadowed.push(record);
        }
        class_index.insert(class.id, (records + k) as u32);
    }
    let mut resolver = Resolver {
        records: index,
        classes: class_index,
        collide,
        undefined: FxHashMap::default(),
        next: defined,
    };

    let mut frame = vec![0u32; defined as usize];
    let mut size = vec![0u32; defined as usize];
    let mut offsets = Offsets::Narrow(vec![0u32; defined as usize + 1]);
    let mut refs = Blocks::new();

    let mut shapes: FxHashMap<u64, Shape> = FxHashMap::default();
    let mut primitive_frames = [u32::MAX; 256];
    let mut object_array_frame = None;
    let (mut instance_count, mut object_array_count, mut primitive_array_count) = (0, 0, 0);

    // The elements of an object array are read this many at a time: an array
    // longer than a chunk is streamed rather than held
    let per_piece = (chunk / id_size).max(1);
    let file_size = r.size().max(1);
    let mut rec = 0usize;

    for &(start, length) in &segments {
        r.seek(start);
        let end = start + length;
        while r.offset() < end {
            if r.moved() {
                progress((100 * r.offset() / file_size) as u32);
            }
            r.need(1)?;
            let sub = r.u1();
            match sub {
                INSTANCE_DUMP => {
                    r.need(2 * id_size + 8)?;
                    r.skip(id_size as u64 + 4);
                    let class_id = r.id();
                    let n = r.u4() as usize;
                    r.need(n)?;
                    let shape = shapes.entry(class_id).or_insert_with(|| {
                        shape_of(class_id, &classes, &class_frames, &mut names, layout, id_size)
                    });
                    frame[rec] = shape.frame;
                    size[rec] = shape.size.unwrap_or_else(|| aligned(layout.header + n as u64));
                    offsets.set(rec, refs.len());
                    let bytes = r.peek(n);
                    for &at in shape.offsets.iter() {
                        let at = at as usize;
                        if at + id_size > n {
                            break;
                        }
                        let id = read_id(&bytes[at..], id_size);
                        if id != 0 {
                            refs.push(resolver.resolve(id));
                        }
                    }
                    r.skip(n as u64);
                    rec += 1;
                    instance_count += 1;
                }
                OBJECT_ARRAY_DUMP => {
                    r.need(2 * id_size + 8)?;
                    r.skip(id_size as u64 + 4);
                    let n = r.u4() as usize;
                    let array_class = r.id();
                    frame[rec] = match classes.position(array_class) {
                        Some(k) => class_frames[k as usize],
                        None => *object_array_frame.get_or_insert_with(|| names.intern("Object[]")),
                    };
                    size[rec] = aligned(layout.array_header + n as u64 * layout.reference);
                    offsets.set(rec, refs.len());
                    let mut left = n;
                    while left > 0 {
                        let m = left.min(per_piece);
                        r.need(m * id_size)?;
                        for element in r.peek(m * id_size).chunks_exact(id_size) {
                            let id = read_id(element, id_size);
                            if id != 0 {
                                refs.push(resolver.resolve(id));
                            }
                        }
                        r.skip((m * id_size) as u64);
                        left -= m;
                    }
                    rec += 1;
                    object_array_count += 1;
                }
                PRIMITIVE_ARRAY_DUMP => {
                    r.need(id_size + 9)?;
                    r.skip(id_size as u64 + 4);
                    let n = r.u4() as u64;
                    let t = r.u1();
                    let element = type_size(t);
                    if primitive_frames[t as usize] == u32::MAX {
                        primitive_frames[t as usize] = names.intern(primitive_array_name(t));
                    }
                    frame[rec] = primitive_frames[t as usize];
                    size[rec] = aligned(layout.array_header + n * element);
                    offsets.set(rec, refs.len());
                    // its contents hold no reference: never read
                    r.skip(n * element);
                    rec += 1;
                    primitive_array_count += 1;
                }
                CLASS_DUMP => {
                    // already read in pass 1
                    read_class(r)?;
                }
                _ => match root_tail(sub, id_size) {
                    // already read in pass 1
                    Some(tail) => r.skip(id_size as u64 + tail),
                    None => return Err(unknown_sub_record(sub, r.offset() - 1)),
                },
            }
        }
    }
    debug_assert_eq!(rec, records);

    // The class objects themselves are not INSTANCE_DUMPs: their metadata
    // lives outside the heap. They still hold the static fields, and the class
    // loader holds them, so they have to be in the graph for those edges to be
    // walked. They weigh nothing.
    for (k, class) in classes.list.iter().enumerate() {
        let i = records + k;
        frame[i] = class_frames[k];
        offsets.set(i, refs.len());
        let held = [class.loader, class.signers, class.domain];
        for &id in held.iter().chain(&class.statics).chain(&class.constants) {
            if id != 0 {
                refs.push(resolver.resolve(id));
            }
        }
    }
    offsets.set(defined as usize, refs.len());

    // A record shadowed by a later one of the same id is not an object: the
    // later one is
    for record in shadowed {
        size[record as usize] = 0;
    }

    // The roots, by index, those that name no object left out
    let mut root_indices = Vec::with_capacity(roots.len());
    for &(id, tag) in &roots {
        if let Some(i) = resolver.find(id) {
            root_indices.push((names.intern(root_name(tag).unwrap_or("GC Root")), i));
        }
    }

    if resolver.next >= u32::MAX as u64 {
        return Err(too_many(resolver.next));
    }
    Ok(Graph {
        defined: defined as u32,
        total: resolver.next as u32,
        frame,
        size,
        offsets,
        refs,
        names,
        roots: root_indices,
        instance_count,
        object_array_count,
        primitive_array_count,
    })
}

fn shape_of(
    class_id: u64,
    classes: &super::metadata::Classes,
    class_frames: &[u32],
    names: &mut Names,
    layout: &Layout,
    id_size: usize,
) -> Shape {
    let mut offsets = Vec::new();
    // at: in the dump, which writes a reference at the width of an id;
    // fields: in the JVM, where it may be narrower
    let (mut at, mut fields) = (0u64, 0u64);
    // The values are written this class first, then each super class: the
    // field list has to be built in that order, not the other way round.
    let mut cid = class_id;
    for _ in 0..1000 {
        if cid == 0 {
            break;
        }
        let Some(class) = classes.get(cid) else { break };
        for &t in &class.fields {
            if t == TYPE_OBJECT {
                offsets.push(at.min(u32::MAX as u64) as u32);
                at += id_size as u64;
                fields += layout.reference;
            } else {
                at += type_size(t);
                fields += type_size(t);
            }
        }
        cid = class.super_id;
    }
    match classes.position(class_id) {
        Some(k) => Shape {
            offsets: offsets.into_boxed_slice(),
            size: Some(aligned(layout.header + fields)),
            frame: class_frames[k as usize],
        },
        None => Shape {
            offsets: offsets.into_boxed_slice(),
            size: None,
            frame: names.intern(&format!("<class:{class_id}>")),
        },
    }
}

fn too_many(count: u64) -> Error {
    Error::Format(format!(
        "{count} objects: more than the 4,294,967,294 an index of 32 bits can number."
    ))
}
