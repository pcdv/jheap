//! The command as an agent or a person calls it: on copies of the fixture
//! dumps, in a directory of their own, since a parse is kept beside its dump.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

// the library's builder of synthetic dumps, for a dump with Strings in it
#[path = "../../jheap/tests/support/hprof.rs"]
mod hprof;

/// A directory with copies of the fixture dumps, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("jheap-cli-{test}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/dumps");
        for name in ["server-before.hprof", "server-after.hprof", "static-holder.hprof", "truncated.hprof"] {
            fs::copy(fixtures.join(name), dir.join(name)).unwrap();
        }
        Dir(dir)
    }

    fn jheap(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_jheap")).args(args).current_dir(&self.0).output().unwrap()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Makes `name` the most recent file of the directory.
    fn touch(&self, name: &str, seconds_ago: u64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds_ago);
        fs::File::options().write(true).open(self.path(name)).unwrap().set_modified(when).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("not JSON ({e}):\n{}", stdout(o)))
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

#[test]
fn the_summary_answers_first_and_keeps_the_parse() {
    let d = Dir::new("summary");
    let o = d.jheap(&["server-after.hprof"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let text = stdout(&o);
    assert!(text.starts_with("server-after.hprof: 618.7 KiB in 949 objects"), "{text}");
    assert!(text.contains("byte[] ← com.example.Session"), "{text}");
    assert!(text.contains("Could grow: one com.example.Registry holds 85% of the heap in a collection of 400 com.example.Session"), "{text}");
    assert!(text.contains("jheap server-after.hprof trace com.example.Registry"), "{text}");
    // plain text in a pipe
    assert!(!text.contains('\x1b'), "{text}");
    assert!(d.path("server-after.jheap.json").exists());

    // the next command reads the kept parse
    let o = d.jheap(&["server-after.hprof", "info"]);
    assert!(stdout(&o).contains("read from server-after.jheap.json"), "{}", stdout(&o));
    // a fresh one parses again
    let o = d.jheap(&["server-after.hprof", "info", "--fresh"]);
    assert!(stdout(&o).contains("parsed in"), "{}", stdout(&o));
}

#[test]
fn no_keep_writes_nothing_beside_the_dump() {
    let d = Dir::new("nokeep");
    let o = d.jheap(&["server-after.hprof", "peaks", "--no-keep"]);
    assert_eq!(code(&o), 0);
    assert!(!d.path("server-after.jheap.json").exists());
}

#[test]
fn every_command_has_its_json() {
    let d = Dir::new("json");
    for args in [
        vec!["server-after.hprof"],
        vec!["server-after.hprof", "peaks"],
        vec!["server-after.hprof", "leaks"],
        vec!["server-after.hprof", "classes"],
        vec!["server-after.hprof", "trace", "Session"],
        vec!["server-after.hprof", "info"],
        vec!["diff", "server-before.hprof", "server-after.hprof"],
    ] {
        let mut with = args.clone();
        with.push("--json");
        let o = d.jheap(&with);
        assert_eq!(code(&o), 0, "{args:?}: {}", stderr(&o));
        let doc = json(&o);
        assert_eq!(doc["jheap"], 1, "{args:?}");
        assert!(doc["command"].is_string(), "{args:?}");
        // a version first, then what the command is: a reader can tell before it reads on
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys[..2], ["jheap", "command"], "{args:?}");
    }

    let leaks = json(&d.jheap(&["server-after.hprof", "leaks", "--json"]));
    assert_eq!(leaks["verdict"], "could-grow");
    assert_eq!(leaks["suspects"][0]["name"], "com.example.Registry");
    assert_eq!(leaks["suspects"][0]["gathered"]["objects"], 400);
    assert_eq!(leaks["dump"]["bytes"], 633536);

    let diff = json(&d.jheap(&["diff", "server-before.hprof", "server-after.hprof", "--json"]));
    assert_eq!(diff["verdict"], "leak-sign");
    assert_eq!(diff["grew"], 471000);
    assert_eq!(diff["suspects"][0]["gathered"]["objects"], 350);
    assert_eq!(diff["suspects"][0]["before"]["objects"], 112);

    let classes = json(&d.jheap(&["server-after.hprof", "classes", "3", "--json"]));
    assert_eq!(classes["classes"][0]["name"], "byte[]");
    assert_eq!(classes["classes"].as_array().unwrap().len(), 3);
    assert_eq!(classes["more"], 9);
}

#[test]
fn exit_codes_say_what_was_found() {
    let d = Dir::new("codes");
    // found nothing: nothing grew, no class of the name
    assert_eq!(code(&d.jheap(&["diff", "server-after.hprof", "server-after.hprof"])), 3);
    assert_eq!(code(&d.jheap(&["server-after.hprof", "trace", "Nothing"])), 3);
    // a name of several classes, listed
    let o = d.jheap(&["server-after.hprof", "trace", "example"]);
    assert_eq!(code(&o), 4);
    assert!(stdout(&o).contains("5 classes match example"));
    // called wrong
    let o = d.jheap(&["server-after.hprof", "peek"]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("no command peek"));
    assert_eq!(code(&d.jheap(&["diff", "server-after.hprof"])), 2);
    // it broke: no such file, a dump cut short, not a dump
    assert_eq!(code(&d.jheap(&["nothere.hprof"])), 1);
    let o = d.jheap(&["truncated.hprof"]);
    assert_eq!(code(&o), 1);
    assert!(stderr(&o).starts_with("jheap: truncated.hprof: Truncated file"), "{}", stderr(&o));
    fs::write(d.path("notes.txt"), "hello").unwrap();
    let o = d.jheap(&["notes.txt"]);
    assert_eq!(code(&o), 1);
    assert!(stderr(&o).contains("neither a heap dump"), "{}", stderr(&o));
}

#[test]
fn without_a_dump_the_most_recent_is_taken_and_named() {
    let d = Dir::new("latest");
    d.touch("server-before.hprof", 3600);
    d.touch("static-holder.hprof", 1800);
    d.touch("truncated.hprof", 7200);
    d.touch("server-after.hprof", 60);
    let o = d.jheap(&["leaks"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stderr(&o).starts_with("jheap: using server-after.hprof (1 minute ago)"), "{}", stderr(&o));
    assert!(stdout(&o).contains("com.example.Registry"));
    // the suggestions find it again
    let o = d.jheap(&[]);
    assert!(stdout(&o).contains("  jheap leaks"), "{}", stdout(&o));

    // its kept parse, newer, does not make another dump the most recent
    d.touch("static-holder.hprof", 10);
    let o = d.jheap(&["--no-keep", "info"]);
    assert!(stderr(&o).contains("using static-holder.hprof"), "{}", stderr(&o));

    // diff takes the two most recent, the earlier first
    let o = d.jheap(&["diff", "--json"]);
    let doc = json(&o);
    assert_eq!(doc["before"]["file"], "server-after.hprof", "{}", stderr(&o));
    assert_eq!(doc["after"]["file"], "static-holder.hprof");

    // nothing to take: called wrong
    let empty = Dir::new("latest-empty");
    for name in ["server-before.hprof", "server-after.hprof", "static-holder.hprof", "truncated.hprof"] {
        fs::remove_file(empty.path(name)).unwrap();
    }
    let o = empty.jheap(&["peaks"]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("no dump in"), "{}", stderr(&o));
}

#[test]
fn a_gzipped_dump_is_read_as_the_dump() {
    let d = Dir::new("gzip");
    let bytes = fs::read(d.path("server-after.hprof")).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&bytes).unwrap();
    // named as a dump, gzipped all the same, as one was found
    fs::write(d.path("zipped.hprof"), gz.finish().unwrap()).unwrap();
    let plain = json(&d.jheap(&["server-after.hprof", "leaks", "--json", "--no-keep"]));
    let zipped = json(&d.jheap(&["zipped.hprof", "leaks", "--json", "--no-keep"]));
    assert_eq!(plain["suspects"], zipped["suspects"]);
    assert!(zipped["dump"]["unzipMs"].as_f64().is_some());
}

#[test]
fn a_saved_profile_is_read_as_the_dump_was() {
    let d = Dir::new("save");
    let o = d.jheap(&["server-after.hprof", "--save", "server.profile.json", "--no-keep", "--json"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let from_dump = json(&d.jheap(&["server-after.hprof", "leaks", "--json", "--no-keep"]));
    let from_profile = json(&d.jheap(&["server.profile.json", "leaks", "--json"]));
    assert_eq!(from_dump["suspects"], from_profile["suspects"]);
    assert_eq!(from_profile["dump"]["profile"], true);
    // the saved profile is Flamepack's version 5, which mountain-graph and jfr-web read
    let doc: Value = serde_json::from_str(&fs::read_to_string(d.path("server.profile.json")).unwrap()).unwrap();
    assert_eq!(doc["version"], 5);
    assert!(doc["samples"]["objects"].is_array());
}

/// A heap of a cache holding six Strings: "hello" three times and "x;y"
/// twice, each in an array of its own, and "once".
fn strings_dump() -> Vec<u8> {
    use hprof::{Builder, ROOT_JAVA_FRAME, TYPE_BYTE, TYPE_INT, TYPE_OBJECT};
    let mut b = Builder::new(Vec::new(), 8).unwrap();
    for (i, name) in ["java.lang.String", "[Ljava/lang/Object;", "com.example.Cache"].iter().enumerate() {
        b.string(0x10 + i as u64, name).unwrap();
        b.load_class(i as u32 + 1, 0x1000 + i as u64, 0x10 + i as u64).unwrap();
    }
    for (i, field) in ["value", "coder", "hash", "items"].iter().enumerate() {
        b.string(0x20 + i as u64, field).unwrap();
    }
    b.class_dump(0x1000, 0, &[(0x20, TYPE_OBJECT), (0x21, TYPE_BYTE), (0x22, TYPE_INT)], &[]).unwrap();
    b.class_dump(0x1001, 0, &[], &[]).unwrap();
    b.class_dump(0x1002, 0, &[(0x23, TYPE_OBJECT)], &[]).unwrap();
    let mut held = Vec::new();
    for (i, text) in ["hello", "hello", "hello", "x;y", "x;y", "once"].iter().enumerate() {
        let (string, value) = (0x2000 + 2 * i as u64, 0x2001 + 2 * i as u64);
        let mut fields = b.id(value);
        fields.push(0);
        fields.extend(0u32.to_be_bytes());
        b.instance(string, 0x1000, &fields).unwrap();
        b.primitive_array_of(value, TYPE_BYTE, 1, text.as_bytes()).unwrap();
        held.push(string);
    }
    b.object_array(0x3000, 0x1001, &held).unwrap();
    b.instance(0x3001, 0x1002, &b.id(0x3000)).unwrap();
    b.root(ROOT_JAVA_FRAME, 0x3001).unwrap();
    b.finish().unwrap()
}

#[test]
fn dups_names_the_duplicated_values_and_who_holds_them() {
    let d = Dir::new("dups");
    fs::write(d.path("strings.hprof"), strings_dump()).unwrap();
    let o = d.jheap(&["strings.hprof", "dups"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("Duplicate Strings: 2 values in 5 copies"), "{text}");
    assert!(text.contains("keeping one copy of each would save 144 B"), "{text}");
    assert!(text.contains("\"hello\""), "{text}");
    assert!(text.contains("3 copies, each with its own array; held by com.example.Cache 100%"), "{text}");
    // the frame escapes the ; that separates the frames of a path
    assert!(text.contains("\"x\\u003by\""), "{text}");
    assert!(!text.contains("once"), "{text}");

    let doc = json(&d.jheap(&["strings.hprof", "dups", "--json"]));
    let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
    assert_eq!(keys[..2], ["jheap", "command"]);
    assert_eq!(doc["command"], "dups");
    assert_eq!(doc["duplicates"]["values"], 2);
    assert_eq!(doc["duplicates"]["copies"], 5);
    assert_eq!(doc["duplicates"]["saving"], 144);
    assert_eq!(doc["top"][0]["value"], "hello");
    assert_eq!(doc["top"][0]["saving"], 96);
    assert_eq!(doc["top"][0]["heldBy"][0]["class"], "com.example.Cache");
    assert_eq!(doc["top"][1]["value"], "x;y");
    assert_eq!(doc["more"], 0);

    // a number of rows
    let one = json(&d.jheap(&["strings.hprof", "dups", "1", "--json"]));
    assert_eq!(one["top"].as_array().unwrap().len(), 1);
    assert_eq!(one["more"], 1);
    assert_eq!(one["moreSaving"], 48);
}

#[test]
fn dups_keeps_no_text_beside_the_dump() {
    let d = Dir::new("dups-keep");
    fs::write(d.path("strings.hprof"), strings_dump()).unwrap();
    let o = d.jheap(&["strings.hprof", "dups"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    // the usual parse is kept, which names classes and never a String's text
    let kept = fs::read_to_string(d.path("strings.jheap.json")).expect("the parse is kept");
    assert!(!kept.contains("hello"), "{kept}");
    // and it is not read in place of the dump: the next dups still finds them
    let o = d.jheap(&["strings.hprof", "dups", "--json"]);
    assert_eq!(json(&o)["duplicates"]["values"], 2);
    assert_eq!(json(&o)["dump"]["readFrom"], Value::Null);
}

#[test]
fn dups_saves_the_profile_of_the_duplicates() {
    let d = Dir::new("dups-save");
    fs::write(d.path("strings.hprof"), strings_dump()).unwrap();
    let o = d.jheap(&["strings.hprof", "dups", "--save", "dups.fpk", "--no-keep"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let doc: Value = serde_json::from_str(&fs::read_to_string(d.path("dups.fpk")).unwrap()).unwrap();
    assert_eq!(doc["version"], 5);
    assert_eq!(doc["duplicates"], serde_json::json!({"count": 2, "copies": 5, "bytes": 240, "saving": 144, "named": 2}));
    // the weights add up to what keeping one copy of each would save
    let weights: f64 = doc["samples"]["weight"].as_array().unwrap().iter().map(|w| w.as_f64().unwrap()).sum();
    assert_eq!(weights, 144.0);
    let objects: f64 = doc["samples"]["objects"].as_array().unwrap().iter().map(|w| w.as_f64().unwrap()).sum();
    assert_eq!(objects, 5.0);
}

#[test]
fn dups_finds_nothing_where_no_string_repeats_and_needs_a_dump() {
    let d = Dir::new("dups-none");
    let o = d.jheap(&["static-holder.hprof", "dups"]);
    assert_eq!(code(&o), 3, "{}", stderr(&o));
    assert!(stdout(&o).contains("No String is held in several copies."), "{}", stdout(&o));
    // no profile is saved of nothing
    let o = d.jheap(&["static-holder.hprof", "dups", "--save", "none.json", "--no-keep"]);
    assert!(stderr(&o).contains("no duplicate String"), "{}", stderr(&o));
    assert!(!d.path("none.json").exists());

    // a profile holds no value to compare
    let o = d.jheap(&["server-after.hprof", "--save", "server.profile.json", "--no-keep"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let o = d.jheap(&["server.profile.json", "dups"]);
    assert_eq!(code(&o), 1);
    assert!(stderr(&o).contains("is a profile: the duplicate Strings are found in the dump"), "{}", stderr(&o));
}

#[test]
fn help_and_version() {
    let d = Dir::new("help");
    let o = d.jheap(&["--help"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("Usage: jheap [<dump>] [<command> [<args>]] [options]"));
    let o = d.jheap(&["-V"]);
    assert_eq!(stdout(&o).trim(), format!("jheap {}", env!("CARGO_PKG_VERSION")));
}
