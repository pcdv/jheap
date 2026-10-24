//! The models give what heap-landscape's gave on the fixture dumps: the
//! numbers of `hl`'s JSON output, shaped as `tools/report.mjs` shapes them.
//! What only the command adds, the dump's header and what a root says of how
//! long it lives, is left out of the comparison.

mod support;

use support::hl::{diff, leaks, peaks, profile, report, same};

const DUMPS: [&str; 3] = ["static-holder", "inherited-fields", "server-after"];

#[test]
fn owners_and_chains_as_hl_reported_them() {
    for dump in DUMPS {
        same(report(&profile(dump)), &format!("{dump}.out.json"));
    }
}

#[test]
fn peaks_and_their_holders_as_hl_reported_them() {
    for dump in DUMPS {
        same(peaks(&profile(dump)), &format!("{dump}.peaks.out.json"));
    }
}

#[test]
fn leak_suspects_as_hl_reported_them() {
    for dump in DUMPS {
        same(leaks(&profile(dump)), &format!("{dump}.leaks.out.json"));
    }
}

#[test]
fn what_grew_as_hl_reported_it() {
    same(diff(&profile("server-before"), &profile("server-after")), "diff.out.json");
}
