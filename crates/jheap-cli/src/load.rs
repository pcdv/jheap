//! Reading a file: a profile as it is, a dump through the parse kept beside
//! it, or parsed, and then kept. The content says which, not the name.

use std::fs::{self, File};
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jheap::profile::document::Document;
use jheap::{Options, Phase, Pointers, Profile, Report};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::style;

/// How a file was read.
pub struct Loaded {
    /// As named, or as found.
    pub path: PathBuf,
    pub profile: Profile,
    /// The parse's report; None for a profile saved without one.
    pub report: Option<Report>,
    /// The kept parse read in place of the dump.
    pub read_from: Option<PathBuf>,
    /// Where the parse was kept, when it was.
    pub kept_at: Option<PathBuf>,
    /// The file is a profile, not a dump.
    pub is_profile: bool,
    /// Milliseconds spent unzipping, not counted in the parse.
    pub unzip_ms: f64,
}

pub struct LoadOptions {
    pub oops: Option<Pointers>,
    pub fresh: bool,
    pub keep: bool,
    pub save: Option<PathBuf>,
    /// Progress on stderr, where a terminal shows it.
    pub progress: bool,
}

/// What a kept parse must match to be read instead of the dump: the same
/// dump, unchanged, the same width of reference asked, and the same parse.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Key {
    tool: String,
    /// jheap's version and the parse's revision: a parse kept by another is
    /// never read in place of this one's, at the cost of one parse after an
    /// update.
    parser: String,
    dump: String,
    size: u64,
    /// Milliseconds since the epoch.
    modified: u64,
    oops: String,
}

/// What a profile document carries besides the profile, when it is a kept
/// parse: what it was kept for, and the parse's report.
#[derive(Deserialize)]
struct Heap {
    #[serde(default)]
    key: Option<Value>,
    #[serde(default)]
    report: Option<Report>,
}

#[derive(Deserialize)]
struct Stored {
    #[serde(flatten)]
    document: Document,
    #[serde(default)]
    heap: Option<Heap>,
}

#[derive(Serialize)]
struct Kept<'a> {
    #[serde(flatten)]
    document: &'a Document,
    heap: KeptHeap<'a>,
}

#[derive(Serialize)]
struct KeptHeap<'a> {
    key: &'a Key,
    report: &'a Report,
}

/// Where the parse of a dump is kept: beside it, named as MAT names its
/// indexes, the dump's name without `.hprof` or `.hprof.gz`.
pub fn kept_path_of(dump: &Path) -> PathBuf {
    let name = dump.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let stem = [".hprof.gz", ".hprof", ".gz"]
        .iter()
        .find(|s| lower.ends_with(*s))
        .map_or(name.as_str(), |s| &name[..name.len() - s.len()]);
    dump.with_file_name(format!("{stem}.jheap.json"))
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn load(path: &Path, o: &LoadOptions) -> Result<Loaded, String> {
    let cannot = |e: io::Error| format!("cannot read {}: {e}", path.display());
    let mut file = File::open(path).map_err(cannot)?;
    let meta = file.metadata().map_err(cannot)?;
    if meta.is_dir() {
        return Err(format!("{} is a directory: name a dump in it, or --dir {0} takes its most recent", path.display()));
    }
    let mut head = [0u8; 32];
    let n = read_up_to(&mut file, &mut head).map_err(cannot)?;
    let head = &head[..n];

    if head.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{') {
        let loaded = read_profile(path)?;
        if o.save.is_some() {
            eprintln!("jheap: --save is for a dump: {} is a profile already", path.display());
        }
        return Ok(loaded);
    }
    let gzipped = head.starts_with(&[0x1f, 0x8b]);
    if !gzipped && !head.starts_with(b"JAVA PROFILE") {
        return Err(format!("{} is neither a heap dump (.hprof, gzipped or not) nor a profile jheap saved", path.display()));
    }

    let key = Key {
        tool: "jheap".into(),
        parser: format!("{}+{}", env!("CARGO_PKG_VERSION"), jheap::hprof::REVISION),
        dump: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size: meta.len(),
        modified: meta.modified().map(millis).unwrap_or(0),
        oops: o.oops.map_or("auto", |p| p.name()).into(),
    };
    let kept_path = kept_path_of(path);
    if !o.fresh
        && let Some(loaded) = read_kept(&kept_path, &key, path)
    {
        if let Some(save) = &o.save {
            save_profile(&loaded.profile, save)?;
        }
        return Ok(loaded);
    }

    let (profile, report, unzip_ms) = parse_dump(path, gzipped, o)?;
    if let Some(save) = &o.save {
        save_profile(&profile, save)?;
    }
    let kept_at = if o.keep { keep(&kept_path, &key, &profile, &report) } else { None };
    Ok(Loaded {
        path: path.to_path_buf(),
        profile,
        report: Some(report),
        read_from: None,
        kept_at,
        is_profile: false,
        unzip_ms,
    })
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn read_profile(path: &Path) -> Result<Loaded, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let doc: Stored = serde_json::from_str(&text)
        .map_err(|e| format!("{} starts as JSON but is not a profile jheap can read: {e}", path.display()))?;
    let profile = doc.document.profile().map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Loaded {
        path: path.to_path_buf(),
        profile,
        report: doc.heap.and_then(|h| h.report),
        read_from: None,
        kept_at: None,
        is_profile: true,
        unzip_ms: 0.0,
    })
}

/// The parse kept for a dump, when there is one and it is of this dump, as it
/// is now, by this parser. Anything else, unreadable or not, is parsed again.
fn read_kept(kept: &Path, key: &Key, dump: &Path) -> Option<Loaded> {
    let text = fs::read_to_string(kept).ok()?;
    let doc: Stored = serde_json::from_str(&text).ok()?;
    let heap = doc.heap?;
    let same: Key = serde_json::from_value(heap.key?).ok()?;
    if same != *key {
        return None;
    }
    let profile = doc.document.profile().ok()?;
    Some(Loaded {
        path: dump.to_path_buf(),
        profile,
        report: heap.report,
        read_from: Some(kept.to_path_buf()),
        kept_at: None,
        is_profile: false,
        unzip_ms: 0.0,
    })
}

/// Keeps a parse beside its dump, written aside and then renamed, so that a
/// run stopped halfway leaves no file that looks whole. A directory that
/// cannot be written to is said, and the command goes on.
fn keep(path: &Path, key: &Key, profile: &Profile, report: &Report) -> Option<PathBuf> {
    let aside = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let document = Document::of(profile);
    let kept = Kept { document: &document, heap: KeptHeap { key, report } };
    let written = File::create(&aside)
        .and_then(|f| {
            let mut w = io::BufWriter::new(f);
            serde_json::to_writer(&mut w, &kept).map_err(io::Error::other)?;
            w.flush()
        })
        .and_then(|_| fs::rename(&aside, path));
    match written {
        Ok(()) => Some(path.to_path_buf()),
        Err(e) => {
            fs::remove_file(&aside).ok();
            eprintln!("jheap: the parse is not kept beside the dump, {e}: --no-keep says not to try");
            None
        }
    }
}

fn save_profile(profile: &Profile, path: &Path) -> Result<(), String> {
    let document = Document::of(profile);
    File::create(path)
        .and_then(|f| {
            let mut w = io::BufWriter::new(f);
            serde_json::to_writer(&mut w, &document).map_err(io::Error::other)?;
            w.flush()
        })
        .map_err(|e| format!("cannot save the profile to {}: {e}", path.display()))
}

/// A file removed when dropped, however the command ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

/// Unzips are written to the system's temporary directory, and the biggest
/// are gigabytes. A run killed before it cleans up (an out-of-memory kill
/// cannot be caught) would leave one behind for good, so those an hour old
/// are removed first.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mine = format!("jheap-{}-", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("jheap-") || !name.ends_with(".hprof") || name.starts_with(&mine) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(3600));
        if old {
            fs::remove_file(entry.path()).ok();
        }
    }
}

fn parse_dump(path: &Path, gzipped: bool, o: &LoadOptions) -> Result<(Profile, Report, f64), String> {
    // The parser seeks, and a gzip stream cannot be seeked: it is unzipped to
    // a temporary file first, disk rather than memory
    let show = o.progress && io::stderr().is_terminal();
    let mut scratch = None;
    let mut unzip_ms = 0.0;
    if gzipped {
        let tmp = std::env::temp_dir();
        sweep(&tmp);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let aside = Scratch(tmp.join(format!("jheap-{}-{nanos}.hprof", std::process::id())));
        let started = Instant::now();
        if show {
            eprint!("\runzipping…");
        }
        let unzipped = File::open(path).and_then(|f| {
            let mut gz = flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 20, f));
            let mut out = io::BufWriter::with_capacity(1 << 20, File::create(&aside.0)?);
            io::copy(&mut gz, &mut out)?;
            out.flush()
        });
        if show {
            eprint!("\r{:20}\r", "");
        }
        unzipped.map_err(|e| {
            format!("{} cannot be unzipped to {}: {e}", path.display(), aside.0.display())
        })?;
        unzip_ms = started.elapsed().as_secs_f64() * 1000.0;
        scratch = Some(aside);
    }
    let source = scratch.as_ref().map_or(path, |s| s.0.as_path());
    let file = File::open(source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let options = Options {
        pointers: o.oops,
        name: path.display().to_string(),
        ..Options::default()
    };
    let mut last = (Phase::Metadata, u32::MAX);
    let mut progress = |phase: Phase, percent: u32| {
        // every 5%, where it can be seen: a redirected stderr would keep every line
        if !show || (phase, percent / 5) == (last.0, last.1 / 5) {
            return;
        }
        last = (phase, percent);
        eprint!("\r{}… {percent}%   ", phase.label());
    };
    let parsed = jheap::parse(BufReader::new(file), &options, &mut progress);
    if show {
        eprint!("\r{:40}\r", "");
    }
    let parsed = parsed.map_err(|e| match e {
        jheap::Error::Io(e) => format!("cannot read {}: {e}", path.display()),
        jheap::Error::Format(m) => format!("{}: {m}", path.display()),
    })?;
    let profile = Profile::from_parse(&parsed);
    Ok((profile, parsed.report, unzip_ms))
}

/// How the dump was read, in a line: parsed, or read from its kept parse.
pub fn how_read(l: &Loaded) -> String {
    if l.is_profile {
        return "a profile: no dump to parse".into();
    }
    if let Some(from) = &l.read_from {
        return format!("read from {}, parsed earlier: --fresh parses again", from.display());
    }
    let mut line = match &l.report {
        Some(r) => format!("parsed in {}", style::duration(r.parse_time_ms)),
        None => "parsed".into(),
    };
    if l.unzip_ms > 0.0 {
        line += &format!(" after {} unzipping", style::duration(l.unzip_ms));
    }
    if let Some(kept) = &l.kept_at {
        line += &format!(", kept as {}", kept.file_name().unwrap_or_default().to_string_lossy());
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_beside_the_dump_without_its_suffix() {
        assert_eq!(kept_path_of(Path::new("d/heap.hprof")), Path::new("d/heap.jheap.json"));
        assert_eq!(kept_path_of(Path::new("d/heap.HPROF.gz")), Path::new("d/heap.jheap.json"));
        assert_eq!(kept_path_of(Path::new("heap.bin")), Path::new("heap.bin.jheap.json"));
    }
}
