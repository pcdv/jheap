//! Generates a large synthetic dump, or parses one and says what it cost.
//!
//! ```sh
//! cargo run --release --example measure -- generate big.hprof 20000000 [--shuffled]
//! cargo run --release --example measure -- parse big.hprof
//! ```
//!
//! The heap is shaped like a server's: groups of map entries, each with a
//! string key and a byte array value, some chained, held by an array that a
//! frame roots. `--shuffled` writes the groups out of address order, as a dump
//! written by several threads is.

#[path = "../tests/support/hprof.rs"]
mod hprof;
#[path = "../tests/support/memory.rs"]
mod memory;

use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use hprof::{Builder, ROOT_JAVA_FRAME, TYPE_BYTE, TYPE_INT, TYPE_OBJECT};
use jheap::{Options, parse};

/// Map entries in a group.
const GROUP: u64 = 1000;
/// Objects in a group: a holder, an array, and four per entry.
const PER_GROUP: u64 = 2 + 4 * GROUP;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("generate") => {
            let objects: u64 = args[2].parse().expect("an object count");
            generate(&args[1], objects, args.iter().any(|a| a == "--shuffled")).unwrap();
        }
        Some("parse") => measure(&args[1]),
        _ => {
            eprintln!("measure generate <dump> <objects> [--shuffled] | measure parse <dump>");
            std::process::exit(2);
        }
    }
}

fn generate(path: &str, objects: u64, shuffled: bool) -> std::io::Result<()> {
    let mut b = Builder::new(BufWriter::with_capacity(1 << 20, File::create(path)?), 8)?;
    // strings, then the classes
    let names = ["java.lang.String", "java.util.HashMap$Node", "[Ljava/lang/Object;", "com.example.Holder"];
    for (i, name) in names.iter().enumerate() {
        b.string(0x10 + i as u64, name)?;
    }
    for (i, field) in ["value", "hash", "key", "next", "table"].iter().enumerate() {
        b.string(0x20 + i as u64, field)?;
    }
    let (string, node, array, holder) = (0x1_0000, 0x1_0100, 0x1_0200, 0x1_0300);
    for (i, class) in [string, node, array, holder].iter().enumerate() {
        b.load_class(i as u32 + 1, *class, 0x10 + i as u64)?;
    }
    b.class_dump(string, 0, &[(0x20, TYPE_OBJECT), (0x21, TYPE_INT)], &[])?;
    b.class_dump(node, 0, &[(0x22, TYPE_OBJECT), (0x20, TYPE_OBJECT), (0x23, TYPE_OBJECT), (0x21, TYPE_INT)], &[])?;
    b.class_dump(array, 0, &[], &[])?;
    b.class_dump(holder, 0, &[(0x24, TYPE_OBJECT)], &[])?;

    let groups = objects.div_ceil(PER_GROUP);
    let base = 0x7_0000_0000u64;
    for k in 0..groups {
        let g = if shuffled { (k * 7919) % groups } else { k };
        // objects 32 bytes apart, a group's in a range of its own
        let mut next = base + g * PER_GROUP * 32;
        let mut alloc = || {
            let id = next;
            next += 32;
            id
        };
        let holder_id = alloc();
        let array_id = alloc();
        let mut entries = Vec::with_capacity(GROUP as usize);
        let mut previous = 0;
        for e in 0..GROUP {
            let (node_id, key, key_bytes, value) = (alloc(), alloc(), alloc(), alloc());
            let chained = if e % 10 == 1 { previous } else { 0 };
            let mut fields = b.id(key);
            fields.extend(b.id(value));
            fields.extend(b.id(chained));
            fields.extend((e as u32).to_be_bytes());
            b.instance(node_id, node, &fields)?;
            let mut fields = b.id(key_bytes);
            fields.extend(16u32.to_be_bytes());
            b.instance(key, string, &fields)?;
            b.primitive_array(key_bytes, TYPE_BYTE, 16, 1)?;
            b.primitive_array(value, TYPE_BYTE, 32 + (e % 64) as u32, 1)?;
            if e % 10 != 0 {
                entries.push(node_id);
            }
            previous = node_id;
        }
        b.object_array(array_id, array, &entries)?;
        b.instance(holder_id, holder, &b.id(array_id))?;
        b.root(ROOT_JAVA_FRAME, holder_id)?;
    }
    b.finish()?;
    println!("{} objects in {groups} groups, in {path}", groups * PER_GROUP);
    Ok(())
}

fn measure(path: &str) {
    let start = Instant::now();
    let options = Options { name: path.to_string(), ..Options::default() };
    let p = parse(File::open(path).unwrap(), &options, &mut |_, _| {}).unwrap();
    let elapsed = start.elapsed();
    let r = &p.report;
    let (peak_set, peak_private) = memory::peak();
    let objects = p.object_count as f64;
    println!("{} objects, {} paths, {:.1} MiB in the heap", p.object_count, p.paths.len(),
             r.total_size as f64 / (1 << 20) as f64);
    println!("parsed in {:.2} s: metadata {:.0} ms, objects {:.0} ms, walk {:.0} ms",
             elapsed.as_secs_f64(), r.metadata_time_ms, r.objects_time_ms, r.graph_time_ms);
    println!("peak working set {:.0} MiB, peak private {:.0} MiB: {:.1} bytes an object",
             peak_set as f64 / (1 << 20) as f64, peak_private as f64 / (1 << 20) as f64,
             peak_private as f64 / objects);
}
