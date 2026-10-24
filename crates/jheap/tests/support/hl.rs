//! The models' output shaped as heap-landscape's `tools/report.mjs` shapes its
//! JSON, to compare with what `hl` printed: for the fixtures' baseline and for
//! local real dumps.

#![allow(dead_code)]

use std::fs::{self, File};
use std::path::PathBuf;

use jheap::model::diff::{diff_of, nodes_by_path};
use jheap::model::leaks::{Suspect, Verdict, leaks_of};
use jheap::model::ownership::{THIN, ownership, ownership_peaks};
use jheap::model::retainers::{Step, made_of, owner_of, steps_of};
use jheap::{Options, Profile, parse};
use serde_json::{Value, json};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn profile(dump: &str) -> Profile {
    let file = File::open(fixtures().join("dumps").join(format!("{dump}.hprof"))).unwrap();
    Profile::from_parse(&parse(file, &Options::default(), &mut |_, _| {}).unwrap())
}

/// The baseline, without what only the command adds.
pub fn baseline(name: &str) -> Value {
    let text = fs::read_to_string(fixtures().join("baseline").join(name)).unwrap();
    of_hl(&text)
}

/// What `hl --json` printed, without what only the command adds.
pub fn of_hl(text: &str) -> Value {
    let mut doc: Value = serde_json::from_str(text).unwrap();
    doc.as_object_mut().unwrap().remove("dump");
    for side in ["before", "after"] {
        if let Some(d) = doc.get_mut(side) {
            *d = json!({"bytes": d["bytes"], "objects": d["objects"]});
        }
    }
    normalize(doc)
}

/// Numbers as floats, and no `lives`: integers and floats compare equal.
pub fn normalize(v: Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.into_iter().map(normalize).collect()),
        Value::Object(o) => {
            Value::Object(o.into_iter().filter(|(k, _)| k != "lives").map(|(k, v)| (k, normalize(v))).collect())
        }
        v => v,
    }
}

/// A share rounded to a hundredth of a percent, as the command gives it.
pub fn share_of(v: f64, of: f64) -> f64 {
    if of > 0.0 { (v / of * 1e4).round() / 1e4 } else { 0.0 }
}

pub fn step_json(step: &Step, of: f64) -> Value {
    json!({"total": step.total, "share": share_of(step.total, of), "classes": step.classes, "root": step.root})
}

pub fn suspect_json(x: &Suspect, rank: usize, of: f64) -> Value {
    json!({
        "rank": rank,
        "kind": x.kind.name(),
        "verdict": x.verdict.name(),
        "name": x.name,
        "total": x.total,
        "share": share_of(x.total, of),
        "objects": x.objects,
        "at": x.at,
        "statics": x.statics,
        "gathered": x.gathered.as_ref().map(|g| json!({
            "name": g.name, "objects": g.objects, "there": g.there, "total": g.total, "through": g.through,
            "containers": g.containers.as_ref().map(|c| json!({"name": c.name, "objects": c.objects})),
        })),
        "includes": x.includes.iter().take(5)
            .map(|c| json!({"name": c.name, "objects": c.objects, "all": c.all, "total": c.total}))
            .collect::<Vec<_>>(),
        "madeOf": x.made_of.iter().take(5)
            .map(|c| json!({"name": c.name, "bytes": c.own, "objects": c.objects})).collect::<Vec<_>>(),
        "holders": x.holders.iter().map(|s| step_json(s, of)).collect::<Vec<_>>(),
    })
}

/// Sorted by value, the heaviest first, the first of equals first.
pub fn rows(entries: Vec<(String, f64)>, top: usize) -> Vec<(String, f64)> {
    let mut entries = entries;
    entries.sort_by(|a, b| b.1.total_cmp(&a.1));
    entries.truncate(top);
    entries
}

/// Adds to a list kept in the order its keys were first seen.
pub fn add(list: &mut Vec<(String, f64)>, key: &str, v: f64) {
    match list.iter_mut().find(|(k, _)| k == key) {
        Some((_, total)) => *total += v,
        None => list.push((key.to_string(), v)),
    }
}

pub fn report(p: &Profile) -> Value {
    let top = 20;
    let heap = p.heap();
    let (mut by_class, mut count_by_class, mut by_holder) = (Vec::new(), Vec::new(), Vec::new());
    for (i, &node) in p.stack.iter().enumerate() {
        if node < 0 {
            continue;
        }
        let node = node as usize;
        add(&mut by_class, p.name(node), p.weight[i]);
        if let Some(o) = &p.objects {
            add(&mut count_by_class, p.name(node), o[i]);
        }
        if let Some(parent) = p.parent_of(node) {
            add(&mut by_holder, p.name(parent), p.weight[i]);
        }
    }
    let mut by_owner = Vec::new();
    for o in ownership(p, 0.0, THIN) {
        add(&mut by_owner, owner_of(&o.path), o.total);
    }
    let named = |rows: Vec<(String, f64)>| -> Vec<Value> {
        rows.into_iter().map(|(name, total)| json!({"name": name, "total": total, "share": share_of(total, heap)})).collect()
    };
    json!({
        "owners": named(rows(by_owner, top)),
        "chains": ownership(p, 0.005 * heap, THIN).into_iter().take(top).map(|o| json!({
            "name": o.name, "total": o.total, "own": o.own, "share": share_of(o.total, heap), "path": o.path,
        })).collect::<Vec<_>>(),
        "classes": rows(by_class, top).into_iter().map(|(name, bytes)| json!({
            "name": name, "bytes": bytes, "share": share_of(bytes, heap),
            "objects": count_by_class.iter().find(|(k, _)| *k == name).map(|(_, v)| *v),
        })).collect::<Vec<_>>(),
        "holders": named(rows(by_holder, top)),
    })
}

pub fn peaks(p: &Profile) -> Value {
    let heap = p.heap();
    let found = ownership_peaks(p, 10, 1.0, THIN).peaks;
    // the peaks held through each node, by their rank: where peaks meet
    let mut through: Vec<Vec<usize>> = vec![Vec::new(); p.len()];
    for (i, peak) in found.iter().enumerate() {
        let mut n = p.parent_of(peak.node);
        while let Some(h) = n {
            through[h].push(i + 1);
            n = p.parent_of(h);
        }
    }
    json!({
        "peaks": found.iter().enumerate().map(|(i, peak)| json!({
            "rank": i + 1,
            "name": peak.name,
            "total": peak.total,
            "share": share_of(peak.total, heap),
            "madeOf": made_of(p, peak.node).iter().take(5)
                .map(|c| json!({"name": c.name, "bytes": c.own, "objects": c.objects})).collect::<Vec<_>>(),
            "holders": steps_of(p, peak.node, 0.1).iter().map(|step| {
                let mut v = step_json(step, heap);
                let also: Vec<usize> = if step.root { Vec::new() }
                    else { through[step.node].iter().copied().filter(|&k| k != i + 1).collect() };
                v["alsoPeaks"] = json!(also);
                v
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

pub fn leaks(p: &Profile) -> Value {
    let (top, share) = (10, 0.1);
    let found = leaks_of(p, share, None);
    let heap = found.heap;
    let suspects: Vec<&Suspect> = found.suspects.iter().filter(|x| x.verdict != Verdict::Spread).collect();
    let spread: Vec<&Suspect> = found.suspects.iter().filter(|x| x.verdict == Verdict::Spread).collect();
    let shown = &suspects[..suspects.len().min(top)];
    json!({
        "share": share,
        "verdict": if shown.iter().any(|x| x.verdict == Verdict::Collection) { "could-grow" } else { "no-sign" },
        "suspects": shown.iter().enumerate().map(|(i, x)| suspect_json(x, i + 1, heap)).collect::<Vec<_>>(),
        "more": suspects.len().saturating_sub(top),
        "spread": spread.iter().map(|x| json!({"name": x.name, "total": x.total, "share": share_of(x.total, heap)}))
            .collect::<Vec<_>>(),
    })
}

pub fn diff(first: &Profile, second: &Profile) -> Value {
    let (top, share) = (10, 0.1);
    let d = diff_of(first, second);
    let mut found = Vec::new();
    let mut suspects = Vec::new();
    if d.grew > 0.0 {
        found = leaks_of(&d.growth, share, Some(second)).suspects;
        let places = [nodes_by_path(first), nodes_by_path(second)];
        let profiles = [first, second];
        let growth_places: std::collections::HashMap<usize, String> =
            nodes_by_path(&d.growth).into_iter().map(|(k, n)| (n, k)).collect();
        let size_at = |node: usize, i: usize| -> Value {
            let at = growth_places.get(&node).and_then(|k| places[i].get(k));
            match at {
                None => Value::Null,
                Some(&n) => {
                    let s = profiles[i].sums();
                    json!({"bytes": s.total[n], "objects": s.held.as_ref().map(|h| h[n])})
                }
            }
        };
        suspects = found.iter().filter(|x| x.verdict != Verdict::Spread).take(top).enumerate().map(|(i, x)| {
            let mut v = suspect_json(x, i + 1, d.grew);
            v["before"] = size_at(x.node, 0);
            v["after"] = size_at(x.node, 1);
            v
        }).collect::<Vec<_>>();
    }
    let verdict = if d.grew == 0.0 { "nothing-grew" }
        else if suspects.iter().any(|x| x["verdict"] == "collection") { "leak-sign" } else { "no-sign" };
    json!({
        "before": {"bytes": d.before.bytes, "objects": d.before.objects},
        "after": {"bytes": d.after.bytes, "objects": d.after.objects},
        "grew": d.grew,
        "shrank": d.shrank,
        "share": share,
        "verdict": verdict,
        "suspects": suspects,
        "spread": found.iter().filter(|x| x.verdict == Verdict::Spread)
            .map(|x| json!({"name": x.name, "total": x.total, "share": share_of(x.total, d.grew)})).collect::<Vec<_>>(),
        "classes": d.classes.iter().take(top)
            .filter(|c| c.after.bytes > c.before.bytes || c.after.objects.unwrap_or(0.0) > c.before.objects.unwrap_or(0.0))
            .map(|c| json!({"name": c.name,
                "before": {"bytes": c.before.bytes, "objects": c.before.objects},
                "after": {"bytes": c.after.bytes, "objects": c.after.objects}}))
            .collect::<Vec<_>>(),
    })
}

pub fn same(got: Value, expected: &str) {
    let want = baseline(expected);
    let got = normalize(got);
    assert!(got == want, "{expected}:\n got  {}\n want {}", got, want);
}

