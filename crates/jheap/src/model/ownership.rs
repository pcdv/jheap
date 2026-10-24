//! What owns the memory: the branches of the tree that hold most of the
//! weight. A container and its contents are one chain, not two consumers, so
//! a chain is reported at the deepest node that still holds most of what is
//! above it.

use crate::profile::{Profile, Sums};

/// A chain of ownership, at the object that owns it.
#[derive(Debug, Clone)]
pub struct Owned {
    pub node: usize,
    pub name: String,
    /// Bytes it holds, its contents and the chain's own objects included.
    pub total: f64,
    /// Bytes of the chain's own objects.
    pub own: f64,
    /// From the root to it.
    pub path: Vec<String>,
}

/// The heaviest child, the first of equals.
fn heaviest_child(s: &Sums, node: usize) -> Option<usize> {
    let mut heaviest: Option<usize> = None;
    for &c in &s.children[node] {
        if heaviest.is_none_or(|h| s.total[c] > s.total[h]) {
            heaviest = Some(c);
        }
    }
    heaviest
}

/// The node that owns the memory under one: follow the heaviest child while
/// it holds `share` of its parent, and stop where the weight spreads over
/// several children.
pub fn anchor_of(node: usize, s: &Sums, share: f64) -> usize {
    *chain_of(node, s, share).last().unwrap()
}

/// The chain from a node down its heaviest branch, as long as each child
/// holds `share` of its parent: the node first, the owner last.
pub fn chain_of(node: usize, s: &Sums, share: f64) -> Vec<usize> {
    let mut chain = vec![node];
    let mut cur = node;
    loop {
        match heaviest_child(s, cur) {
            Some(h) if s.total[h] >= share * s.total[cur] => {
                cur = h;
                chain.push(cur);
            }
            _ => return chain,
        }
    }
}

/// How little an object may own of what it holds and still be called an
/// owner. A handler or a manager holds the server and owns a few hundred
/// bytes: it is where the walk entered the graph, not a consumer of memory.
pub const THIN: f64 = 0.01;

/// The branches of the tree that hold the memory, the heaviest first. Each
/// chain is reported at the object that owns it, and everything beside the
/// chain is a chain of its own, so the anchors add up to the whole tree.
///
/// `least` is the smallest anchor worth reporting, in bytes. Below `thin` of
/// its weight, an object that passes the weight on to several children is not
/// an owner and its children are taken instead; 0 keeps every object.
pub fn ownership(p: &Profile, least: f64, thin: f64) -> Vec<Owned> {
    let s = p.sums();
    let mut anchors: Vec<(usize, f64, f64)> = Vec::new();

    fn walk(p: &Profile, s: &Sums, node: usize, least: f64, thin: f64, anchors: &mut Vec<(usize, f64, f64)>) {
        let chain = chain_of(node, s, 0.5);
        let anchor = *chain.last().unwrap();
        // What hangs beside the chain is held by something else: a chain of
        // its own. The anchor's own children are not, they are in its total.
        for pair in chain.windows(2) {
            for &c in &s.children[pair[0]] {
                if c != pair[1] {
                    walk(p, s, c, least, thin, anchors);
                }
            }
        }
        // A root is where the walk started, not an object holding anything:
        // when the weight spreads right there, its children are the owners
        if p.parent[anchor] < 0 {
            for &c in &s.children[anchor] {
                walk(p, s, c, least, thin, anchors);
            }
            return;
        }
        // The chain holds its own objects as well as what the anchor holds: a
        // cache and the maps inside it are one consumer, not the maps alone
        let mut total = s.total[anchor];
        let own: f64 = chain.iter().map(|&n| s.own[n]).sum();
        for &n in &chain[..chain.len() - 1] {
            total += s.own[n];
        }
        // A pass-through: it holds a lot and owns almost none of it, so what
        // it holds is not its own. Its children are the owners, as at a root.
        if thin > 0.0 && own < thin * total && s.children[anchor].len() > 1 {
            for &c in &s.children[anchor] {
                walk(p, s, c, least, thin, anchors);
            }
            return;
        }
        if total >= least {
            anchors.push((anchor, total, own));
        }
    }

    for &root in &s.roots {
        walk(p, s, root, least, thin, &mut anchors);
    }
    let mut owned: Vec<Owned> = anchors
        .into_iter()
        .map(|(node, total, own)| Owned { node, name: p.name(node).to_string(), total, own, path: p.path_of(node) })
        .collect();
    owned.sort_by(|a, b| b.total.total_cmp(&a.total));
    owned
}

/// An ownership chain as a peak, as the page ranks them.
#[derive(Debug, Clone)]
pub struct OwnedPeak {
    /// Of the anchor.
    pub node: usize,
    /// The anchor and the object holding it, numbered when the same names
    /// make several peaks.
    pub name: String,
    /// Bytes of the chain's own objects.
    pub own: f64,
    /// Bytes it holds, its contents included.
    pub total: f64,
    /// Its total over the heap's, 0 to 1.
    pub share: f64,
    /// What holds the chain, the nearest first, out to the root.
    pub holders: Vec<String>,
    /// Bytes the anchor holds alone, without the chain's own objects.
    pub subtree: f64,
}

#[derive(Debug, Clone)]
pub struct OwnershipPeaks {
    pub peaks: Vec<OwnedPeak>,
    /// Bytes no peak accounts for.
    pub plain: f64,
    /// The share of the heap the peaks account for, 0 to 1.
    pub coverage: f64,
    /// Bytes of the heap.
    pub total: f64,
}

/// The ownership chains as peaks: at most `count`, until they account for
/// `coverage` of the heap.
pub fn ownership_peaks(p: &Profile, count: usize, coverage: f64, thin: f64) -> OwnershipPeaks {
    let s = p.sums();
    let heap = p.heap();
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut peaks = Vec::new();
    let mut covered = 0.0;
    for o in ownership(p, 0.0, thin) {
        if peaks.len() >= count || (heap > 0.0 && covered / heap >= coverage) {
            break;
        }
        // the anchor and the object holding it: the same class may own several chains
        let base = match p.parent_of(o.node) {
            Some(holder) if !p.name(holder).is_empty() => format!("{} ← {}", o.name, p.name(holder)),
            _ => o.name.clone(),
        };
        let n = match seen.iter_mut().find(|(b, _)| *b == base) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                seen.push((base.clone(), 1));
                1
            }
        };
        let mut holders = o.path[..o.path.len() - 1].to_vec();
        holders.reverse();
        peaks.push(OwnedPeak {
            node: o.node,
            name: if n > 1 { format!("{base} ({n})") } else { base },
            own: o.own,
            total: o.total,
            share: if heap > 0.0 { o.total / heap } else { 0.0 },
            holders,
            subtree: s.total[o.node],
        });
        covered += o.total;
    }
    OwnershipPeaks {
        peaks,
        plain: heap - covered,
        coverage: if heap > 0.0 { covered / heap } else { 0.0 },
        total: heap,
    }
}

/// A node as the panel lists it, with the nodes under it or, for holders,
/// the one holding it.
#[derive(Debug, Clone)]
pub struct Frame {
    pub node: usize,
    pub name: String,
    pub total: f64,
    /// Bytes of the samples that end there.
    pub own: f64,
    pub children: Vec<Frame>,
}

/// What one object holds: the object at the top, the objects under it, the
/// heaviest first. `extra` is added to the top's total: the bytes of the
/// chain of holders above it, when it was reached as a peak.
pub fn holds_of(p: &Profile, node: usize, extra: f64) -> Option<Frame> {
    if node >= p.len() {
        return None;
    }
    let s = p.sums();
    fn build(p: &Profile, s: &Sums, i: usize) -> Frame {
        let mut children: Vec<Frame> = s.children[i].iter().map(|&c| build(p, s, c)).collect();
        children.sort_by(|a, b| b.total.total_cmp(&a.total));
        Frame { node: i, name: p.name(i).to_string(), total: s.total[i], own: s.own[i], children }
    }
    let mut root = build(p, s, node);
    root.total += extra;
    Some(root)
}

/// Who holds an object: the object at the top, the one holding it under it,
/// and so on out to the root, each with what it holds, so the chain widens
/// outward. `extra` is added to every total, as `holds_of` adds it to its top.
pub fn holders_of(p: &Profile, node: usize, extra: f64) -> Option<Frame> {
    if node >= p.len() {
        return None;
    }
    let s = p.sums();
    let mut chain = Vec::new();
    let mut n = Some(node);
    while let Some(i) = n {
        chain.push(i);
        n = p.parent_of(i);
    }
    let mut frame: Option<Frame> = None;
    for &i in chain.iter().rev() {
        frame = Some(Frame {
            node: i,
            name: p.name(i).to_string(),
            total: s.total[i] + extra,
            own: s.own[i],
            children: frame.into_iter().collect(),
        });
    }
    frame
}

/// What the outermost holder of a chain holds: what the shares of the
/// holders are taken of, as a chain grows outward.
pub fn outermost_of(chain: &Frame) -> f64 {
    let mut total = chain.total;
    let mut f = chain;
    while let Some(next) = f.children.first() {
        total = total.max(next.total);
        f = next;
    }
    total
}
