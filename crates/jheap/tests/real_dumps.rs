//! Real dumps, which never leave the machine: each dump under the directories
//! `JHEAP_REAL_DUMPS` lists (searched with their subdirectories) that has a
//! `.hl.json` beside it, written by heap-landscape's `hl --fresh`, is parsed and
//! compared with it path by path, and the time is printed beside the JS
//! parser's. The peak memory printed is the process's, which the JSON of the
//! comparison adds to: `cargo run --release --example measure -- parse <dump>`
//! measures one parse alone.
//!
//! ```sh
//! JHEAP_REAL_DUMPS=/path/to/dumps cargo test --release -p jheap --test real_dumps -- --ignored --nocapture
//! ```
//!
//! Nothing it reads or prints is committed: a parse names the application's
//! classes.

mod support;

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Instant;

use jheap::{Options, parse};
use serde_json::Value;

/// The report's fields that describe the run, not the dump.
const RUN: [&str; 5] = ["fileName", "parseTimeMs", "metadataTimeMs", "objectsTimeMs", "graphTimeMs"];

fn dumps_under(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dumps_under(&path, found);
        } else if path.to_string_lossy().to_lowercase().ends_with(".hprof")
            || path.to_string_lossy().to_lowercase().ends_with(".hprof.gz")
        {
            found.push(path);
        }
    }
}

fn kept_beside(dump: &Path) -> PathBuf {
    let name = dump.file_name().unwrap().to_string_lossy();
    let stem = name.strip_suffix(".gz").unwrap_or(&name);
    let stem = stem.strip_suffix(".hprof").unwrap_or(stem);
    dump.with_file_name(format!("{stem}.hl.json"))
}

/// The paths of a profile document, as jfr-web's version 4 writes them:
/// strings front-coded, nodes depth first, a sample per path.
fn paths_of(doc: &Value) -> HashMap<Vec<String>, (u64, u64)> {
    let mut strings: Vec<String> = Vec::new();
    let mut previous: Vec<u16> = Vec::new();
    for s in doc["strings"].as_array().unwrap() {
        // the first character is the length, in UTF-16 units, of the prefix
        // shared with the previous string, plus 32
        let units: Vec<u16> = s.as_str().unwrap().encode_utf16().collect();
        let shared = (units[0] - 32) as usize;
        previous.truncate(shared);
        previous.extend_from_slice(&units[1..]);
        strings.push(String::from_utf16_lossy(&previous));
    }
    let stacks = doc["stacks"].as_array().unwrap();
    let (mut parent, mut frame, mut last) = (Vec::new(), Vec::new(), Vec::<usize>::new());
    for (i, pair) in stacks.chunks(2).enumerate() {
        let depth = pair[0].as_u64().unwrap() as usize;
        parent.push(if depth == 0 { usize::MAX } else { last[depth - 1] });
        frame.push(pair[1].as_u64().unwrap() as usize >> 3);
        last.truncate(depth);
        last.push(i);
    }
    let samples = &doc["samples"];
    let column = |name: &str| -> Vec<u64> {
        samples[name].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as u64).collect()
    };
    let (stack, weight, objects) = (column("stack"), column("weight"), column("objects"));
    let mut paths = HashMap::new();
    for i in 0..stack.len() {
        let mut names = Vec::new();
        let mut node = stack[i] as usize;
        while node != usize::MAX {
            names.push(strings[frame[node]].clone());
            node = parent[node];
        }
        names.reverse();
        let entry = paths.entry(names).or_insert((0, 0));
        entry.0 += weight[i];
        entry.1 += objects[i];
    }
    paths
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 20) as f64
}

#[test]
#[ignore = "reads the dumps of JHEAP_REAL_DUMPS, which are local"]
fn parses_real_dumps_as_heap_landscape_did() {
    let Some(dirs) = std::env::var_os("JHEAP_REAL_DUMPS") else {
        eprintln!("JHEAP_REAL_DUMPS names no directory: nothing to compare");
        return;
    };
    let mut dumps = Vec::new();
    for dir in std::env::split_paths(&dirs) {
        dumps_under(&dir, &mut dumps);
    }
    // the smallest first: the peak is the process's, so each dump's is the
    // largest yet
    dumps.sort_by_key(|d| fs::metadata(d).map(|m| m.len()).unwrap_or(0));

    let mut failures = Vec::new();
    let mut compared = 0;
    for dump in &dumps {
        let kept = kept_beside(dump);
        if !kept.exists() {
            eprintln!("{}: no .hl.json beside it, skipped", dump.display());
            continue;
        }
        // a gzipped dump, whatever its name says, is unzipped aside first, as
        // hl does: its parse time is the parse's only
        let mut magic = [0u8; 2];
        std::io::Read::read_exact(&mut File::open(dump).unwrap(), &mut magic).unwrap();
        let unzipped = (magic == [0x1f, 0x8b]).then(|| {
            let aside = std::env::temp_dir().join(format!("jheap-test-{}.hprof", std::process::id()));
            let mut gz = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(File::open(dump).unwrap()));
            std::io::copy(&mut gz, &mut File::create(&aside).unwrap()).unwrap();
            aside
        });
        let source = unzipped.as_deref().unwrap_or(dump);
        let doc: Value = serde_json::from_str(&fs::read_to_string(&kept).unwrap()).unwrap();
        let theirs = &doc["heap"]["report"];
        let pointers = match doc["heap"]["key"]["oops"].as_str() {
            Some("compressed") => Some(jheap::Pointers::Compressed),
            Some("wide") => Some(jheap::Pointers::Wide),
            _ => None,
        };
        let options = Options { pointers, name: dump.display().to_string(), ..Options::default() };

        let start = Instant::now();
        let result = parse(File::open(source).unwrap(), &options, &mut |_, _| {});
        if let Some(aside) = &unzipped {
            fs::remove_file(aside).ok();
        }
        let parsed = match result {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: {e}", dump.display()));
                continue;
            }
        };
        let seconds = start.elapsed().as_secs_f64();
        let (_, peak) = support::memory::peak();
        compared += 1;

        let mut ours = serde_json::to_value(&parsed.report).unwrap();
        let mut theirs = theirs.clone();
        for key in RUN {
            ours.as_object_mut().unwrap().remove(key);
            theirs.as_object_mut().unwrap().remove(key);
        }
        if ours != theirs {
            for (key, value) in theirs.as_object().unwrap() {
                if ours.get(key) != Some(value) {
                    failures.push(format!("{}: {key} is {:?}, {value} in the .hl.json", dump.display(), ours.get(key)));
                }
            }
        }

        let want = paths_of(&doc);
        let mut got: HashMap<Vec<String>, (u64, u64)> = HashMap::new();
        for p in &parsed.paths {
            got.insert(parsed.frames(p).map(str::to_string).collect(), (p.bytes, p.objects));
        }
        let differ = want.iter().filter(|(path, value)| got.get(*path) != Some(value)).count()
            + got.keys().filter(|path| !want.contains_key(*path)).count();
        if differ > 0 {
            failures.push(format!("{}: {differ} of {} paths differ", dump.display(), want.len()));
        }

        let objects = parsed.object_count;
        println!(
            "{}: {} objects, {:.0} MiB of dump\n  jheap {:.2} s, the process's peak so far {:.0} MiB, {:.1} bytes an object\n  hl    {:.2} s{}",
            dump.file_name().unwrap().to_string_lossy(),
            objects,
            mib(parsed.report.file_size),
            seconds,
            mib(peak),
            peak as f64 / objects as f64,
            doc["heap"]["report"]["parseTimeMs"].as_f64().unwrap_or(0.0) / 1000.0,
            if differ == 0 && ours == theirs { ", same parse" } else { ", parses DIFFER" },
        );
    }
    assert!(failures.is_empty(), "{compared} compared:\n{}", failures.join("\n"));
}
