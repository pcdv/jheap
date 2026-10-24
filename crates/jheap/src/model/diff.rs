//! What changed between two dumps of the same process: the strongest sign of
//! a leak, which one dump can only suggest. The dumps are compared path by
//! path, a path being the same place in both: the chain of classes from a root.
//!
//! What grew is a profile of its own, so that the leak suspects of one dump
//! find where the growth is: a collection that gained many objects is a leak
//! far more surely than one that is merely large.
//!
//! An object the walk reached another way in the second dump moves from one
//! path to another: it shows as growth here and as shrinking there. The two
//! totals are given, so that what moved can be told from what was added.

use std::collections::HashMap;

use crate::hash::FxHashMap;
use crate::profile::Profile;

/// The root of what the walk never reached: garbage, which grows and shrinks
/// by itself.
const UNREACHABLE: &str = "(unreachable);";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Amount {
    pub bytes: f64,
    /// None when the profile does not count objects.
    pub objects: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ClassChange {
    pub name: String,
    pub before: Amount,
    pub after: Amount,
}

#[derive(Debug)]
pub struct Diff {
    /// The paths that grew, each by what it gained.
    pub growth: Profile,
    /// The heap of the first dump, what nothing reaches left out.
    pub before: Amount,
    /// The same of the second.
    pub after: Amount,
    /// Bytes gained by the paths that grew.
    pub grew: f64,
    /// Bytes lost by the paths that shrank or went.
    pub shrank: f64,
    /// Every class, by the objects it gained, the most first; by the bytes
    /// when the profiles do not count objects.
    pub classes: Vec<ClassChange>,
}

/// The key of each node: its path's names joined by `;`.
fn keys_of(p: &Profile) -> Vec<String> {
    let mut keys: Vec<Option<String>> = vec![None; p.len()];
    for node in 0..p.len() {
        // the nodes up to the first whose key is known, then down again: a
        // parent comes before its children in no order for sure
        let mut up = Vec::new();
        let mut n = Some(node);
        while let Some(i) = n {
            if keys[i].is_some() {
                break;
            }
            up.push(i);
            n = p.parent_of(i);
        }
        for &i in up.iter().rev() {
            let key = match p.parent_of(i) {
                Some(parent) => format!("{};{}", keys[parent].as_ref().unwrap(), p.name(i)),
                None => p.name(i).to_string(),
            };
            keys[i] = Some(key);
        }
    }
    keys.into_iter().map(Option::unwrap).collect()
}

/// The paths of a profile, each with its bytes and objects, in the order of
/// its samples.
pub fn paths_of(p: &Profile) -> Vec<(String, f64, f64)> {
    let keys = keys_of(p);
    let mut out: Vec<(String, f64, f64)> = Vec::new();
    let mut index: FxHashMap<&str, usize> = FxHashMap::default();
    for (i, &node) in p.stack.iter().enumerate() {
        if node < 0 {
            continue;
        }
        let key = keys[node as usize].as_str();
        let at = *index.entry(key).or_insert_with(|| {
            out.push((key.to_string(), 0.0, 0.0));
            out.len() - 1
        });
        out[at].1 += p.weight[i];
        out[at].2 += p.objects.as_ref().map_or(0.0, |o| o[i]);
    }
    out
}

/// The node of each path of a profile, its own and those it passes through:
/// to find a place of one dump in another.
pub fn nodes_by_path(p: &Profile) -> HashMap<String, usize> {
    keys_of(p).into_iter().enumerate().map(|(n, k)| (k, n)).collect()
}

/// What changed from `before`, the first dump, to `after`, a later one of the
/// same process.
pub fn diff_of(before: &Profile, after: &Profile) -> Diff {
    let counted = before.objects.is_some() && after.objects.is_some();
    let (a, b) = (paths_of(before), paths_of(after));
    let in_a: FxHashMap<&str, (f64, f64)> = a.iter().map(|(k, w, o)| (k.as_str(), (*w, *o))).collect();
    let in_b: FxHashMap<&str, ()> = b.iter().map(|(k, _, _)| (k.as_str(), ())).collect();
    let mut grown = Vec::new();
    let (mut grew, mut shrank) = (0.0, 0.0);
    for (k, bytes, objects) in &b {
        if k.starts_with(UNREACHABLE) {
            continue;
        }
        let was = in_a.get(k.as_str());
        let d = bytes - was.map_or(0.0, |w| w.0);
        if d > 0.0 {
            grown.push((k.clone(), d, (objects - was.map_or(0.0, |w| w.1)).max(0.0)));
            grew += d;
        } else {
            shrank -= d;
        }
    }
    for (k, bytes, _) in &a {
        if !in_b.contains_key(k.as_str()) && !k.starts_with(UNREACHABLE) {
            shrank += bytes;
        }
    }

    let none = Amount { bytes: 0.0, objects: counted.then_some(0.0) };
    let mut classes: Vec<ClassChange> = Vec::new();
    let mut index: FxHashMap<String, usize> = FxHashMap::default();
    for (paths, after_side) in [(&a, false), (&b, true)] {
        for (k, bytes, objects) in paths.iter() {
            if k.starts_with(UNREACHABLE) {
                continue;
            }
            let name = &k[k.rfind(';').map_or(0, |i| i + 1)..];
            let at = *index.entry(name.to_string()).or_insert_with(|| {
                classes.push(ClassChange { name: name.to_string(), before: none, after: none });
                classes.len() - 1
            });
            let side = if after_side { &mut classes[at].after } else { &mut classes[at].before };
            side.bytes += bytes;
            if let Some(o) = side.objects.as_mut() {
                *o += objects;
            }
        }
    }
    let total = |after_side: bool| {
        let (mut bytes, mut objects) = (0.0, 0.0);
        for c in &classes {
            let side = if after_side { &c.after } else { &c.before };
            bytes += side.bytes;
            objects += side.objects.unwrap_or(0.0);
        }
        Amount { bytes, objects: counted.then_some(objects) }
    };
    let (before_total, after_total) = (total(false), total(true));
    let change = |c: &ClassChange| match (c.after.objects, c.before.objects) {
        (Some(x), Some(y)) if counted => x - y,
        _ => c.after.bytes - c.before.bytes,
    };
    classes.sort_by(|x, y| {
        let by_objects = change(y) - change(x);
        let order = if by_objects != 0.0 && !by_objects.is_nan() {
            by_objects
        } else {
            (y.after.bytes - y.before.bytes) - (x.after.bytes - x.before.bytes)
        };
        order.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });

    Diff {
        growth: Profile::from_stacks(grown, counted),
        before: before_total,
        after: after_total,
        grew,
        shrank,
        classes,
    }
}
