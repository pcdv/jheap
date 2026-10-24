//! Chains deeper than the paths are kept: heap-landscape's depth tests.

mod support;

use std::io::Cursor;

use jheap::{Options, Parse, parse};
use support::hprof::{Builder, ROOT_UNKNOWN, TYPE_OBJECT};

/// A chain of instances, each holding the next: longer than the path cap, so
/// the tail is only reached if the walk keeps going past it.
fn chain(length: u64) -> Parse {
    let mut b = Builder::new(Vec::new(), 4).unwrap();
    b.string(0x101, "Link").unwrap();
    b.string(0x102, "next").unwrap();
    b.load_class(1, 0x201, 0x101).unwrap();
    b.class_dump(0x201, 0, &[(0x102, TYPE_OBJECT)], &[]).unwrap();
    for i in 0..length {
        let next = if i + 1 < length { 0x1000 + i + 1 } else { 0 };
        let values = b.id(next);
        b.instance(0x1000 + i, 0x201, &values).unwrap();
    }
    b.root(ROOT_UNKNOWN, 0x1000).unwrap();
    let bytes = b.finish().unwrap();
    parse(Cursor::new(bytes), &Options::default(), &mut |_, _| {}).unwrap()
}

#[test]
fn reaches_every_link_past_the_path_cap() {
    let p = chain(50);
    assert_eq!(p.report.unreachable_size, 0);
    // 8 of header, 4 of field, aligned
    assert_eq!(p.report.reachable_size, 50 * 16);
    assert!(p.report.max_depth_reached > 30);
    assert!(p.report.depth_limit_hits > 0);
    // every path stays at the cap, so the paths do not grow with the chain
    assert!(p.paths.iter().all(|path| path.frames.len() <= 31));
}

#[test]
fn reads_a_chain_longer_than_the_stack_is_deep() {
    let p = chain(200_000);
    assert_eq!(p.report.unreachable_size, 0);
    assert_eq!(p.report.reachable_size, 200_000 * 16);
    assert_eq!(p.report.max_depth_reached, 200_001);
    // a root object is at depth 1: the links from the 31st on are past the cap
    assert_eq!(p.report.depth_limit_hits, 200_000 - 30);
}

#[test]
fn files_the_links_past_the_cap_under_one_path() {
    let p = chain(200_000);
    let full: Vec<&str> = std::iter::once("Unknown").chain(std::iter::repeat_n("Link", 30)).collect();
    let window: Vec<&str> = std::iter::repeat_n("Link", 31).collect();
    let longest: Vec<_> = p.paths.iter().filter(|path| path.frames.len() == 31).collect();
    assert_eq!(longest.len(), 2);
    let named = |path: &jheap::Path| p.frames(path).collect::<Vec<_>>();
    let window_path = longest.iter().find(|path| named(path) == window).expect("the window");
    assert!(longest.iter().any(|path| named(path) == full));
    assert_eq!(window_path.bytes, (200_000 - 30) * 16);
    assert_eq!(window_path.objects, 200_000 - 30);
}
