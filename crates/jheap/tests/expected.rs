//! jheap's parse of each fixture dump is the one heap-landscape's parser made:
//! the same paths, bytes and objects, the same report.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::PathBuf;

use jheap::{Options, Pointers, parse};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// The report's fields that describe the run, not the dump.
const RUN: [&str; 5] = ["fileName", "parseTimeMs", "metadataTimeMs", "objectsTimeMs", "graphTimeMs"];

type Paths = BTreeMap<Vec<String>, (u64, u64)>;

fn expected_paths(doc: &Value) -> Paths {
    doc["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let path = p["path"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
            (path, (p["bytes"].as_u64().unwrap(), p["objects"].as_u64().unwrap()))
        })
        .collect()
}

fn check(expected: &PathBuf, chunk: usize) {
    let doc: Value = serde_json::from_str(&fs::read_to_string(expected).unwrap()).unwrap();
    let dump = doc["dump"].as_str().unwrap();
    let pointers = match doc["pointers"].as_str().unwrap() {
        "auto" => None,
        "compressed" => Some(Pointers::Compressed),
        "wide" => Some(Pointers::Wide),
        other => panic!("pointers {other}"),
    };
    let options = Options { pointers, chunk, name: dump.to_string(), ..Options::default() };
    let file = File::open(fixtures().join("dumps").join(dump)).unwrap();
    let result = parse(file, &options, &mut |_, _| {});
    let what = format!("{} at a chunk of {chunk}", expected.display());

    if let Some(error) = doc.get("error") {
        let got = result.err().map(|e| e.to_string());
        assert_eq!(got.as_deref(), error.as_str(), "{what}");
        return;
    }
    let parsed = result.unwrap_or_else(|e| panic!("{what}: {e}"));

    let mut report = serde_json::to_value(&parsed.report).unwrap();
    for key in RUN {
        report.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(report, doc["report"], "{what}: report");

    let got: Paths = parsed
        .paths
        .iter()
        .map(|p| (parsed.frames(p).map(str::to_string).collect(), (p.bytes, p.objects)))
        .collect();
    let want = expected_paths(&doc);
    for (path, value) in &want {
        assert_eq!(got.get(path), Some(value), "{what}: {}", path.join(";"));
    }
    for path in got.keys() {
        assert!(want.contains_key(path), "{what}: unexpected {}", path.join(";"));
    }
}

#[test]
fn parses_as_heap_landscape_did() {
    let mut files: Vec<PathBuf> = fs::read_dir(fixtures().join("expected"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".parse.json"))
        .collect();
    files.sort();
    assert!(files.len() >= 6, "fixtures missing: {files:?}");
    for file in &files {
        // the default window, then one so small every record falls across its edge
        check(file, 1 << 22);
        check(file, 7);
    }
}
