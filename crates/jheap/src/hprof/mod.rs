//! The HPROF parser: from a heap dump to the paths from its roots.
//!
//! Two passes over the file, each through a window:
//!
//! 1. the strings, classes and roots, and the id of every object;
//! 2. the objects, what each weighs and holds, then a walk from the roots that
//!    gathers them into paths as it goes.
//!
//! Only the object graph is kept, never the dump: memory grows with the
//! number of objects and references, not with the file.

mod cursor;
mod graph;
mod index;
mod metadata;
mod tags;
mod walk;

pub(crate) use tags::ROOT_NAMES;

/// The revision of what a parse gives: raised with any change to the paths,
/// their bytes or objects, or the report, so that a parse kept by an earlier
/// revision is not read in place of a new one.
pub const REVISION: u32 = 1;

use std::fmt;
use std::io::{self, Read, Seek};

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use cursor::{CHUNK, Cursor};

/// How wide a reference is in the JVM, which the dump does not say: it writes
/// every one at the width of its ids. Compressed, the default of a 64-bit JVM
/// under 32 GB of heap, a reference is 4 bytes and a header 12; wide, 8 and
/// 16; a 32-bit JVM, 4 and 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Pointers {
    #[default]
    #[serde(rename = "compressed")]
    Compressed,
    #[serde(rename = "wide")]
    Wide,
    #[serde(rename = "32-bit")]
    Bits32,
}

impl Pointers {
    pub fn name(self) -> &'static str {
        match self {
            Pointers::Compressed => "compressed",
            Pointers::Wide => "wide",
            Pointers::Bits32 => "32-bit",
        }
    }
}

/// What a parse is asked.
#[derive(Clone)]
pub struct Options {
    /// The width of references in the JVM; None guesses it from the heap's span.
    pub pointers: Option<Pointers>,
    /// The bytes read at a time: small in tests, so that records fall across
    /// the window's edge.
    pub chunk: usize,
    /// The dump's name, for the report and the messages.
    pub name: String,
    /// Milliseconds from some fixed time, for the report's timings. The
    /// default is the system's clock, and zero on wasm32, which has none: the
    /// web app passes its own.
    pub clock: fn() -> f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { pointers: None, chunk: CHUNK, name: "heap dump".into(), clock: system_clock }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn system_clock() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
fn system_clock() -> f64 {
    0.0
}

/// Where a parse is, for a progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Pass 1: strings, classes, roots.
    Metadata,
    /// Pass 2: the objects.
    Objects,
    /// The walk from the roots.
    Paths,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Metadata => "Reading strings and classes",
            Phase::Objects => "Reading objects",
            Phase::Paths => "Aggregating paths",
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// The file could not be read.
    Io(io::Error),
    /// The file is not a heap dump, or not a whole one.
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => e.fmt(f),
            Error::Format(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// The sizes an object is counted in.
pub(crate) struct Layout {
    pub(crate) pointers: Pointers,
    pub(crate) guessed: bool,
    /// Bytes of a reference.
    pub(crate) reference: u64,
    /// Bytes of an object's header.
    pub(crate) header: u64,
    /// Bytes of an array's header, its length included.
    pub(crate) array_header: u64,
}

/// Compressed pointers reach 32 GB of heap at most: a heap wider than that has
/// wide ones.
const COMPRESSED_REACH: u64 = 32 << 30;

pub(crate) fn layout_of(id_size: usize, pointers: Option<Pointers>, heap_span: u64) -> Layout {
    if id_size == 4 {
        return Layout { pointers: Pointers::Bits32, guessed: false, reference: 4, header: 8, array_header: 12 };
    }
    let compressed = match pointers {
        None => heap_span < COMPRESSED_REACH,
        Some(p) => p == Pointers::Compressed,
    };
    let guessed = pointers.is_none();
    if compressed {
        Layout { pointers: Pointers::Compressed, guessed, reference: 4, header: 12, array_header: 16 }
    } else {
        Layout { pointers: Pointers::Wide, guessed, reference: 8, header: 16, array_header: 24 }
    }
}

/// What a parse says of the dump and of itself, as heap-landscape's parse
/// report: the profile document carries it as `heap.report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Report {
    pub file_name: String,
    /// Bytes.
    pub file_size: u64,
    /// 4 or 8 bytes per object id.
    pub id_size: u32,
    /// The layout the shallow sizes are counted in.
    pub pointers: Pointers,
    /// The layout was guessed from the heap's span, not given.
    pub pointers_guessed: bool,
    /// Bytes between the lowest and the highest object address.
    pub heap_span: u64,
    /// The HPROF version string.
    pub version: String,
    pub parse_time_ms: f64,
    /// Reading strings and classes.
    pub metadata_time_ms: f64,
    /// Reading objects.
    pub objects_time_ms: f64,
    /// Walking the graph and gathering the paths.
    pub graph_time_ms: f64,
    pub string_count: u64,
    pub class_count: u64,
    pub root_count: u64,
    /// The roots of each type, in the order first seen.
    #[serde(serialize_with = "in_order", deserialize_with = "read_in_order")]
    pub roots_by_type: Vec<(String, u64)>,
    pub instance_count: u64,
    pub object_array_count: u64,
    pub primitive_array_count: u64,
    pub heap_segment_count: u64,
    pub heap_segment_bytes: u64,
    /// The deepest path found, in frames.
    pub max_depth_reached: u64,
    /// The objects deeper than the paths are kept, filed under their last frames.
    pub depth_limit_hits: u64,
    /// Distinct paths, those of what nothing reaches included.
    pub unique_path_count: u64,
    /// Bytes of all objects.
    pub total_size: u64,
    /// Bytes the roots reach.
    pub reachable_size: u64,
    /// Bytes they do not.
    pub unreachable_size: u64,
}

fn in_order<S: Serializer>(entries: &[(String, u64)], s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(entries.iter().map(|(k, v)| (k, v)))
}

/// A map read in the order it is written, which a map type would not keep.
fn read_in_order<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<(String, u64)>, D::Error> {
    struct InOrder;
    impl<'de> Visitor<'de> for InOrder {
        type Value = Vec<(String, u64)>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map of counts")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry::<String, u64>()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    d.deserialize_map(InOrder)
}

/// One path from a root: its frames, root first, and the objects that end
/// there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    /// Indices in [`Parse::names`].
    pub frames: Vec<u32>,
    pub bytes: u64,
    pub objects: u64,
}

/// A parsed dump: every path from a root with its bytes and objects, what
/// nothing reaches under `(unreachable)`, and the report.
#[derive(Debug, Clone)]
pub struct Parse {
    /// The frames' names: classes, root types, `(unreachable)`.
    pub names: Vec<String>,
    /// Sorted by their names joined, so the same dump gives the same order.
    pub paths: Vec<Path>,
    pub report: Report,
    /// The objects of the heap, the class objects included.
    pub object_count: u64,
    /// The distinct names objects and roots have.
    pub frame_count: u64,
}

impl Parse {
    /// A path's frames by name.
    pub fn frames(&self, path: &Path) -> impl Iterator<Item = &str> {
        path.frames.iter().map(|&f| self.names[f as usize].as_str())
    }
}

/// Parses a heap dump.
///
/// `progress` is told the phase and how far into it the parse is, in percent.
pub fn parse<R: Read + Seek>(
    src: R,
    options: &Options,
    progress: &mut dyn FnMut(Phase, u32),
) -> Result<Parse, Error> {
    let clock = options.clock;
    let start = clock();
    let mut r = Cursor::new(src, options.chunk)?;

    progress(Phase::Metadata, 0);
    let meta = metadata::read_metadata(&mut r, &mut |p| progress(Phase::Metadata, p))?;
    let metadata_time_ms = clock() - start;
    // a profile of CPU samples or allocation sites is an HPROF too, with no heap
    if meta.segments.is_empty() {
        return Err(Error::Format(format!(
            "{} has no heap dump in it: it is an HPROF file, but of something else.",
            options.name
        )));
    }
    let layout = layout_of(meta.id_size, options.pointers, meta.heap_span);

    let mut roots_by_type: Vec<(String, u64)> = Vec::new();
    for &(_, tag) in &meta.roots {
        let name = tags::root_name(tag).map_or_else(|| format!("0x{tag:x}"), str::to_string);
        match roots_by_type.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => roots_by_type.push((name, 1)),
        }
    }
    let version = meta.version.clone();
    let id_size = meta.id_size as u32;
    let heap_span = meta.heap_span;
    let string_count = meta.string_count as u64;
    let class_count = meta.classes.list.len() as u64;
    let root_count = meta.roots.len() as u64;
    let heap_segment_count = meta.segments.len() as u64;
    let heap_segment_bytes = meta.segments.iter().map(|s| s.1).sum();

    let objects_start = clock();
    progress(Phase::Objects, 0);
    let mut g = graph::build_graph(&mut r, meta, &layout, options.chunk, &mut |p| progress(Phase::Objects, p))?;
    if g.defined == 0 {
        return Err(Error::Format(format!("{} has a heap dump with no object in it.", options.name)));
    }
    let objects_time_ms = clock() - objects_start;

    let graph_start = clock();
    progress(Phase::Paths, 0);
    // what nothing reaches is filed under a name of its own
    let frame_count = g.names.list.len() as u64;
    let unreachable = g.names.intern("(unreachable)");
    let walked = walk::walk(&mut g, unreachable, &mut |p| progress(Phase::Paths, p));
    let graph_time_ms = clock() - graph_start;
    progress(Phase::Paths, 100);

    let names = std::mem::take(&mut g.names.list);
    let mut paths: Vec<Path> = walked
        .paths
        .into_iter()
        .map(|(frames, bytes, objects)| Path { frames: frames.into_vec(), bytes, objects })
        .collect();
    let joined = |p: &Path| -> String {
        p.frames.iter().map(|&f| names[f as usize].as_str()).collect::<Vec<_>>().join(";")
    };
    paths.sort_by_cached_key(joined);

    let report = Report {
        file_name: options.name.clone(),
        file_size: r.size(),
        id_size,
        pointers: layout.pointers,
        pointers_guessed: layout.guessed,
        heap_span,
        version,
        parse_time_ms: clock() - start,
        metadata_time_ms,
        objects_time_ms,
        graph_time_ms,
        string_count,
        class_count,
        root_count,
        roots_by_type,
        instance_count: g.instance_count,
        object_array_count: g.object_array_count,
        primitive_array_count: g.primitive_array_count,
        heap_segment_count,
        heap_segment_bytes,
        max_depth_reached: walked.max_depth_reached,
        depth_limit_hits: walked.depth_limit_hits,
        unique_path_count: paths.len() as u64,
        total_size: walked.reachable_size + walked.unreachable_size,
        reachable_size: walked.reachable_size,
        unreachable_size: walked.unreachable_size,
    };
    Ok(Parse { names, paths, report, object_count: g.defined as u64, frame_count })
}
