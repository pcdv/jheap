//! The duplicate Strings: which values are held in several copies, what
//! keeping one would save, and where the copies are held.

mod support;

use std::io::Cursor;

use jheap::{Duplicates, Options, Parse, parse};
use support::hprof::{Builder, ROOT_JAVA_FRAME, ROOT_STICKY_CLASS, TYPE_BYTE, TYPE_CHAR, TYPE_INT, TYPE_OBJECT};

const STRING: u64 = 0x1000;
const ARRAY: u64 = 0x1001;
const HOLDER_A: u64 = 0x1002;
const HOLDER_B: u64 = 0x1003;
const FILLER: u64 = 0x1004;

/// Builds a dump: the classes, then the objects `objects` writes, given the
/// builder and a source of fresh ids.
fn dump(modern: bool, objects: impl FnOnce(&mut Builder<Vec<u8>>, &mut dyn FnMut() -> u64)) -> Vec<u8> {
    let mut b = Builder::new(Vec::new(), 8).unwrap();
    let names = ["java.lang.String", "[Ljava/lang/Object;", "com.example.A", "com.example.B", "com.example.Filler"];
    for (i, name) in names.iter().enumerate() {
        b.string(0x10 + i as u64, name).unwrap();
        b.load_class(i as u32 + 1, STRING + i as u64, 0x10 + i as u64).unwrap();
    }
    for (i, field) in ["value", "coder", "hash", "items"].iter().enumerate() {
        b.string(0x20 + i as u64, field).unwrap();
    }
    // JDK 9 on: a byte[] and its coder; before, a char[]
    let fields: &[(u64, u8)] =
        if modern { &[(0x20, TYPE_OBJECT), (0x21, TYPE_BYTE), (0x22, TYPE_INT)] } else { &[(0x20, TYPE_OBJECT), (0x22, TYPE_INT)] };
    b.class_dump(STRING, 0, fields, &[]).unwrap();
    b.class_dump(ARRAY, 0, &[], &[]).unwrap();
    b.class_dump(HOLDER_A, 0, &[(0x23, TYPE_OBJECT)], &[]).unwrap();
    b.class_dump(HOLDER_B, 0, &[(0x23, TYPE_OBJECT)], &[]).unwrap();
    b.class_dump(FILLER, 0, &[], &[]).unwrap();
    let mut next = 0x7_0000_0000u64;
    let mut fresh = || {
        next += 16;
        next
    };
    // more records than a checkpoint's worth before the Strings, so that
    // their values are read from one
    for _ in 0..600 {
        let id = fresh();
        b.instance(id, FILLER, &[]).unwrap();
    }
    objects(&mut b, &mut fresh);
    b.finish().unwrap()
}

/// A String's field bytes.
fn string_fields(b: &Builder<Vec<u8>>, value: u64, coder: Option<u8>) -> Vec<u8> {
    let mut fields = b.id(value);
    fields.extend(coder);
    fields.extend(0u32.to_be_bytes());
    fields
}

/// A holder of the class, rooted by a frame, holding the Strings in an array.
fn holder(b: &mut Builder<Vec<u8>>, fresh: &mut dyn FnMut() -> u64, class: u64, strings: &[u64]) {
    let (h, items) = (fresh(), fresh());
    b.object_array(items, ARRAY, strings).unwrap();
    b.instance(h, class, &b.id(items)).unwrap();
    b.root(ROOT_JAVA_FRAME, h).unwrap();
}

/// A JDK 9 heap: "hello" three times, each in its own array; "dedup" four
/// times in one array, as G1's deduplication leaves it; "hé€" twice in UTF-16;
/// the bytes of a UTF-16 "ab" once as LATIN1, which is another value; "once";
/// and two copies of "garbage" nothing reaches.
fn modern() -> Vec<u8> {
    dump(true, |b, fresh| {
        let mut a = Vec::new();
        for _ in 0..3 {
            let (s, v) = (fresh(), fresh());
            b.instance(s, STRING, &string_fields(b, v, Some(0))).unwrap();
            b.primitive_array_of(v, TYPE_BYTE, 1, b"hello").unwrap();
            a.push(s);
        }
        // a String and its value in two segments, read across their edge
        let (s, v) = (fresh(), fresh());
        b.instance(s, STRING, &string_fields(b, v, Some(1))).unwrap();
        b.flush().unwrap();
        let utf16: Vec<u8> = "hé€".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b.primitive_array_of(v, TYPE_BYTE, 1, &utf16).unwrap();
        a.push(s);
        let (s, v) = (fresh(), fresh());
        b.primitive_array_of(v, TYPE_BYTE, 1, &utf16).unwrap();
        b.instance(s, STRING, &string_fields(b, v, Some(1))).unwrap();
        a.push(s);
        // a root record between the objects, which a read back steps over
        b.root(ROOT_STICKY_CLASS, STRING).unwrap();
        for (text, coder) in [("ab".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>(), 1), (b"a\0b\0".to_vec(), 0)] {
            let (s, v) = (fresh(), fresh());
            b.instance(s, STRING, &string_fields(b, v, Some(coder))).unwrap();
            b.primitive_array_of(v, TYPE_BYTE, 1, &text).unwrap();
            a.push(s);
        }
        holder(b, fresh, HOLDER_A, &a);

        let shared = fresh();
        b.primitive_array_of(shared, TYPE_BYTE, 1, b"dedup").unwrap();
        let mut held = Vec::new();
        for _ in 0..4 {
            let s = fresh();
            b.instance(s, STRING, &string_fields(b, shared, Some(0))).unwrap();
            held.push(s);
        }
        let (s, v) = (fresh(), fresh());
        b.instance(s, STRING, &string_fields(b, v, Some(0))).unwrap();
        b.primitive_array_of(v, TYPE_BYTE, 1, b"once").unwrap();
        held.push(s);
        holder(b, fresh, HOLDER_B, &held);

        for _ in 0..2 {
            let (s, v) = (fresh(), fresh());
            b.instance(s, STRING, &string_fields(b, v, Some(0))).unwrap();
            b.primitive_array_of(v, TYPE_BYTE, 1, b"garbage").unwrap();
        }
    })
}

/// A JDK 8 heap: "x;y" twice, in char arrays.
fn jdk8() -> Vec<u8> {
    dump(false, |b, fresh| {
        let chars: Vec<u8> = "x;y".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut a = Vec::new();
        for _ in 0..2 {
            let (s, v) = (fresh(), fresh());
            b.instance(s, STRING, &string_fields(b, v, None)).unwrap();
            b.primitive_array_of(v, TYPE_CHAR, 2, &chars).unwrap();
            a.push(s);
        }
        holder(b, fresh, HOLDER_A, &a);
    })
}

fn parsed(bytes: &[u8], chunk: usize, duplicates: bool) -> Parse {
    let options = Options { chunk, duplicates, ..Options::default() };
    parse(Cursor::new(bytes.to_vec()), &options, &mut |_, _| {}).unwrap()
}

/// What the duplicates say, timings left out.
fn found(d: &Duplicates) -> Vec<(String, u64, u64, u64, u64, u64)> {
    d.values.iter().map(|v| (v.text.clone(), v.length, v.copies, v.arrays, v.bytes, v.saving)).collect()
}

#[test]
fn finds_the_values_held_in_several_copies() {
    let p = parsed(&modern(), 1 << 20, true);
    let d = p.duplicates.as_ref().unwrap();
    // compressed references: a String is 12 of header and 9 of fields, 24
    // aligned; "hello" and "dedup" 16 and 5, 24; "hé€" 16 and 6, 24
    assert_eq!(
        found(d),
        vec![
            ("hello".into(), 5, 3, 3, 144, 96),
            ("dedup".into(), 5, 4, 1, 120, 72),
            ("hé€".into(), 3, 2, 2, 96, 48),
        ]
    );
    assert_eq!((d.count, d.copies, d.bytes, d.saving), (3, 9, 360, 216));
}

#[test]
fn files_each_copy_under_its_holder_then_its_value() {
    let p = parsed(&modern(), 1 << 20, true);
    let d = p.duplicates.as_ref().unwrap();
    let paths: Vec<(Vec<&str>, u64, u64)> = d
        .paths
        .iter()
        .map(|path| (path.frames.iter().map(|&f| d.names[f as usize].as_str()).collect(), path.bytes, path.objects))
        .collect();
    fn held<'a>(holder: &'a str, value: &'a str) -> Vec<&'a str> {
        vec!["Java Frame", holder, "java.lang.Object[]", "java.lang.String", value]
    }
    assert_eq!(
        paths,
        vec![
            (held("com.example.A", "\"hello\""), 96, 3),
            (held("com.example.A", "\"hé€\""), 48, 2),
            (held("com.example.B", "\"dedup\""), 72, 4),
        ]
    );
}

#[test]
fn reads_the_same_whatever_the_window() {
    let bytes = modern();
    let whole = parsed(&bytes, 1 << 20, true);
    for chunk in [64, 100, 4096] {
        let p = parsed(&bytes, chunk, true);
        let (a, b) = (p.duplicates.as_ref().unwrap(), whole.duplicates.as_ref().unwrap());
        assert_eq!(found(a), found(b), "a window of {chunk}");
        assert_eq!(a.paths, b.paths, "a window of {chunk}");
    }
}

#[test]
fn leaves_the_parse_as_it_is() {
    let bytes = modern();
    let (with, without) = (parsed(&bytes, 1 << 20, true), parsed(&bytes, 1 << 20, false));
    assert!(without.duplicates.is_none());
    assert_eq!(with.names, without.names);
    assert_eq!(with.paths, without.paths);
}

#[test]
fn reads_the_char_arrays_of_jdk_8() {
    let p = parsed(&jdk8(), 1 << 20, true);
    let d = p.duplicates.as_ref().unwrap();
    // a String is 12 and 8, 24; "x;y" 16 and 6, 24
    assert_eq!(found(d), vec![("x;y".into(), 3, 2, 2, 96, 48)]);
    // the frame escapes what separates the frames of a path
    assert!(d.names.contains(&"\"x\\u003by\"".to_string()));
}
