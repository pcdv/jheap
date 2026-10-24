//! jheap's parser for the browser, as WebAssembly.
//!
//! The page hands a dropped `File` to a Web Worker, which reads it
//! synchronously a slice at a time (`FileReaderSync`): the parser reads it
//! through `Read + Seek` like any file, and the dump is never held whole. What
//! comes back is the profile document and the parse's report, as JSON: 1.8 MB
//! for a dump of 4.6 GB. The object graph never crosses into JavaScript.
//!
//! No wasm-bindgen: heap-landscape has no build step, so the module exports a
//! few plain functions, imports three, and a page module of its own wraps them.
//!
//! Imports, from the module `jheap`:
//!
//! - `read_at(offset: f64, ptr: u32, len: u32) -> u32`: copies up to `len`
//!   bytes of the file from `offset` into memory at `ptr`, says how many;
//! - `progress(phase: u32, percent: u32)`: 0 reading strings and classes, 1
//!   reading objects, 2 aggregating paths;
//! - `now() -> f64`: milliseconds, for the report's timings.
//!
//! Exports: `memory`, `jheap_alloc(len)` and `jheap_free(ptr, len)` for the
//! file's name, `jheap_parse(size, oops, chunk, name, name_len)` which returns
//! the length of its answer (`chunk` the bytes read at a time, 0 for the
//! default), and `jheap_result()` where the answer is: a JSON
//! object, `{doc, report}` or `{error}`.

use std::io::{self, Read, Seek, SeekFrom};

use jheap::profile::document::Document;
use jheap::{Options, Phase, Pointers, Profile, Report};
use serde::Serialize;

#[derive(Serialize)]
struct Parsed<'a> {
    doc: &'a Document,
    report: &'a Report,
}

/// Parses a dump and answers in JSON: the profile document and the report,
/// or the error.
pub fn parse_to_json<R: Read + Seek>(
    src: R,
    options: &Options,
    progress: &mut dyn FnMut(Phase, u32),
) -> String {
    match jheap::parse(src, options, progress) {
        Ok(parsed) => {
            let doc = Document::of(&Profile::from_parse(&parsed));
            serde_json::to_string(&Parsed { doc: &doc, report: &parsed.report }).expect("a document serializes")
        }
        Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
    }
}

/// The width of references, as the page asks: 0 to guess, 1 compressed, 2 wide.
pub fn pointers(oops: u32) -> Option<Pointers> {
    match oops {
        1 => Some(Pointers::Compressed),
        2 => Some(Pointers::Wide),
        _ => None,
    }
}

/// A file the page reads for the parser, a slice at a time.
pub struct Source<F: FnMut(u64, &mut [u8]) -> usize> {
    read_at: F,
    size: u64,
    at: u64,
}

impl<F: FnMut(u64, &mut [u8]) -> usize> Source<F> {
    pub fn new(size: u64, read_at: F) -> Self {
        Source { read_at, size, at: 0 }
    }
}

impl<F: FnMut(u64, &mut [u8]) -> usize> Read for Source<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.size.saturating_sub(self.at);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        let read = (self.read_at)(self.at, &mut buf[..n]);
        self.at += read as u64;
        Ok(read)
    }
}

impl<F: FnMut(u64, &mut [u8]) -> usize> Seek for Source<F> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
            SeekFrom::Current(d) => self.at.checked_add_signed(d),
        };
        self.at = at.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"))?;
        Ok(self.at)
    }
}

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::sync::Mutex;

    use super::*;

    #[link(wasm_import_module = "jheap")]
    unsafe extern "C" {
        fn read_at(offset: f64, ptr: *mut u8, len: u32) -> u32;
        fn progress(phase: u32, percent: u32);
        fn now() -> f64;
    }

    /// The last answer, kept until the next parse for the page to read.
    static RESULT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    fn clock() -> f64 {
        // SAFETY: an import of the page, which takes nothing
        unsafe { now() }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn jheap_alloc(len: u32) -> *mut u8 {
        let mut v = Vec::<u8>::with_capacity(len as usize);
        let ptr = v.as_mut_ptr();
        std::mem::forget(v);
        ptr
    }

    /// # Safety
    /// `ptr` and `len` are those of a `jheap_alloc`, freed once.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn jheap_free(ptr: *mut u8, len: u32) {
        // SAFETY: the caller gives back what jheap_alloc gave
        drop(unsafe { Vec::from_raw_parts(ptr, 0, len as usize) });
    }

    /// Parses the file the page reads, of `size` bytes, named by the UTF-8 at
    /// `name`, read `chunk` bytes at a time (0 for the default). Returns the
    /// length of the answer, at `jheap_result()`.
    ///
    /// # Safety
    /// `name` points to `name_len` bytes of memory.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn jheap_parse(size: f64, oops: u32, chunk: u32, name: *const u8, name_len: u32) -> u32 {
        // SAFETY: the caller wrote the name there
        let name = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, name_len as usize) }).into_owned();
        let mut options = Options { pointers: pointers(oops), name, clock, ..Options::default() };
        if chunk > 0 {
            options.chunk = chunk as usize;
        }
        let source = Source::new(size as u64, |offset, buf: &mut [u8]| {
            // SAFETY: the page copies at most len bytes to ptr, which buf owns
            unsafe { read_at(offset as f64, buf.as_mut_ptr(), buf.len() as u32) as usize }
        });
        let answer = parse_to_json(source, &options, &mut |phase, percent| {
            let phase = match phase {
                Phase::Metadata => 0,
                Phase::Objects => 1,
                Phase::Paths => 2,
            };
            // SAFETY: an import of the page, which takes two numbers
            unsafe { progress(phase, percent) }
        });
        let mut result = RESULT.lock().unwrap();
        *result = answer.into_bytes();
        result.len() as u32
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn jheap_result() -> *const u8 {
        RESULT.lock().unwrap().as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(name: &str) -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dumps").join(name)).unwrap()
    }

    #[test]
    fn answers_the_document_and_the_report() {
        let bytes = dump("server-after.hprof");
        let mut reads = 0;
        let source = Source::new(bytes.len() as u64, |offset, buf: &mut [u8]| {
            reads += 1;
            let start = offset as usize;
            buf.copy_from_slice(&bytes[start..start + buf.len()]);
            buf.len()
        });
        let options = Options { chunk: 4096, ..Options::default() };
        let answer: serde_json::Value = serde_json::from_str(&parse_to_json(source, &options, &mut |_, _| {})).unwrap();
        assert_eq!(answer["doc"]["version"], 4);
        assert_eq!(answer["report"]["totalSize"], 633536);
        assert_eq!(answer["report"]["pointers"], "compressed");
        // read in slices, never whole
        assert!(reads > 10, "{reads} reads");
    }

    #[test]
    fn answers_an_error_as_one() {
        let bytes = dump("truncated.hprof");
        let source = Source::new(bytes.len() as u64, |offset, buf: &mut [u8]| {
            let start = offset as usize;
            buf.copy_from_slice(&bytes[start..start + buf.len()]);
            buf.len()
        });
        let answer: serde_json::Value =
            serde_json::from_str(&parse_to_json(source, &Options::default(), &mut |_, _| {})).unwrap();
        assert!(answer["error"].as_str().unwrap().starts_with("Truncated file"), "{answer}");
    }
}
