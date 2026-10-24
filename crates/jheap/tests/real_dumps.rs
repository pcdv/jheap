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
//! A second test runs the models on each dump and compares their output with
//! what `hl --json` prints for the report, `peaks` and `leaks`, and for a
//! `diff` of the two smallest dumps; it needs node, and `hl`, which only
//! heap-landscape's commit `75ac409` has: `HEAP_LANDSCAPE` names a checkout of
//! it (see fixtures/README.md). jheap's command has since moved on from `hl`'s
//! output, but the models still give what it gave.
//!
//! Nothing they read or print is committed: a parse names the application's
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

/// Parses a dump, a gzipped one (whatever its name says) unzipped aside
/// first, as hl does: the time given is the parse's only.
fn parse_dump(dump: &Path, pointers: Option<jheap::Pointers>) -> (Result<jheap::Parse, jheap::Error>, f64) {
    let mut magic = [0u8; 2];
    std::io::Read::read_exact(&mut File::open(dump).unwrap(), &mut magic).unwrap();
    let unzipped = (magic == [0x1f, 0x8b]).then(|| {
        let aside = std::env::temp_dir().join(format!("jheap-test-{}.hprof", std::process::id()));
        let mut gz = flate2::read::MultiGzDecoder::new(std::io::BufReader::new(File::open(dump).unwrap()));
        std::io::copy(&mut gz, &mut File::create(&aside).unwrap()).unwrap();
        aside
    });
    let source = unzipped.as_deref().unwrap_or(dump);
    let options = Options { pointers, name: dump.display().to_string(), ..Options::default() };
    let start = Instant::now();
    let result = parse(File::open(source).unwrap(), &options, &mut |_, _| {});
    let seconds = start.elapsed().as_secs_f64();
    if let Some(aside) = &unzipped {
        fs::remove_file(aside).ok();
    }
    (result, seconds)
}

/// The dumps under `JHEAP_REAL_DUMPS`, the smallest first.
fn real_dumps() -> Vec<PathBuf> {
    let mut dumps = Vec::new();
    if let Some(dirs) = std::env::var_os("JHEAP_REAL_DUMPS") {
        for dir in std::env::split_paths(&dirs) {
            dumps_under(&dir, &mut dumps);
        }
    }
    dumps.sort_by_key(|d| fs::metadata(d).map(|m| m.len()).unwrap_or(0));
    dumps
}

/// What `hl` prints as JSON for these arguments, read fresh and kept nowhere.
fn hl(args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let landscape = std::env::var_os("HEAP_LANDSCAPE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../heap-landscape"));
    let out = std::process::Command::new("node")
        .arg(landscape.join("tools/report.mjs"))
        .args(args)
        .args(["--json", "--fresh", "--no-keep"])
        .output()
        .map_err(|e| format!("node: {e}"))?;
    if out.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Where two documents first differ, for a message that is not two pages long.
fn first_difference(path: &str, got: &Value, want: &Value) -> Option<String> {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                let at = format!("{path}.{k}");
                match a.get(k) {
                    Some(g) => {
                        if let Some(d) = first_difference(&at, g, v) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{at}: missing")),
                }
            }
            a.keys().find(|k| !b.contains_key(*k)).map(|k| format!("{path}.{k}: not expected"))
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (g, v)) in a.iter().zip(b).enumerate() {
                if let Some(d) = first_difference(&format!("{path}[{i}]"), g, v) {
                    return Some(d);
                }
            }
            (a.len() != b.len()).then(|| format!("{path}: {} items, {} expected", a.len(), b.len()))
        }
        _ => (got != want).then(|| format!("{path}: {got}, {want} expected")),
    }
}

#[test]
#[ignore = "reads the dumps of JHEAP_REAL_DUMPS, which are local, and runs hl"]
fn models_on_real_dumps_as_heap_landscape_did() {
    use support::hl::{diff, leaks, normalize, of_hl, peaks, report};

    let dumps = real_dumps();
    let mut failures = Vec::new();
    let mut profiles = Vec::new();
    for dump in &dumps {
        let (result, _) = parse_dump(dump, None);
        let p = match result {
            Ok(parsed) => jheap::Profile::from_parse(&parsed),
            Err(e) => {
                failures.push(format!("{}: {e}", dump.display()));
                continue;
            }
        };
        let name = dump.file_name().unwrap().to_string_lossy().into_owned();
        type Shape = fn(&jheap::Profile) -> Value;
        let shaped: [(&str, Shape); 3] = [("", report), ("peaks", peaks), ("leaks", leaks)];
        for (command, shape) in shaped {
            let mut args: Vec<&std::ffi::OsStr> = Vec::new();
            if !command.is_empty() {
                args.push(command.as_ref());
            }
            args.push(dump.as_os_str());
            let verdict = match hl(&args) {
                Err(e) => format!("hl failed: {e}"),
                Ok(text) => match first_difference("", &normalize(shape(&p)), &of_hl(&text)) {
                    None => "same".to_string(),
                    Some(d) => {
                        failures.push(format!("{name} {command}: {d}"));
                        "DIFFERS".to_string()
                    }
                },
            };
            println!("{name} {}: {verdict}", if command.is_empty() { "report" } else { command });
        }
        profiles.push((dump.clone(), p));
    }
    // two dumps, the smallest: of one process or not, the diff has to agree
    if let [(a, first), (b, second), ..] = profiles.as_slice() {
        let verdict = match hl(&["diff".as_ref(), a.as_os_str(), b.as_os_str()]) {
            Err(e) => format!("hl failed: {e}"),
            Ok(text) => match first_difference("", &normalize(diff(first, second)), &of_hl(&text)) {
                None => "same".to_string(),
                Some(d) => {
                    failures.push(format!("diff: {d}"));
                    "DIFFERS".to_string()
                }
            },
        };
        println!("diff of the two smallest: {verdict}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 20) as f64
}

#[test]
#[ignore = "reads the dumps of JHEAP_REAL_DUMPS, which are local"]
fn parses_real_dumps_as_heap_landscape_did() {
    // the smallest first: the peak is the process's, so each dump's is the
    // largest yet
    let dumps = real_dumps();
    if dumps.is_empty() {
        eprintln!("JHEAP_REAL_DUMPS names no directory with dumps: nothing to compare");
    }

    let mut failures = Vec::new();
    let mut compared = 0;
    for dump in &dumps {
        let kept = kept_beside(dump);
        if !kept.exists() {
            eprintln!("{}: no .hl.json beside it, skipped", dump.display());
            continue;
        }
        let doc: Value = serde_json::from_str(&fs::read_to_string(&kept).unwrap()).unwrap();
        let theirs = &doc["heap"]["report"];
        let pointers = match doc["heap"]["key"]["oops"].as_str() {
            Some("compressed") => Some(jheap::Pointers::Compressed),
            Some("wide") => Some(jheap::Pointers::Wide),
            _ => None,
        };

        let (result, seconds) = parse_dump(dump, pointers);
        let parsed = match result {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: {e}", dump.display()));
                continue;
            }
        };
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
