//! What probably leaks, from one dump: the objects of the application that
//! hold a large share of the heap, where under them the memory gathers, and
//! what kind of memory that is, which is what tells a leak from a footprint.
//!
//! One suspect per piece of memory: a holder, the objects it holds and their
//! classes are one suspect, not one each, as MAT's report has them. The
//! classes whose many objects together hold much are folded into the suspect
//! most of them hang from, or are suspects of their own when none does.
//!
//! Found in the tree of paths rather than a dominator tree: what an object
//! holds is what the walk reached through it first, so a shared object counts
//! under one holder only.

use crate::hash::{FxHashMap, FxHashSet};
use crate::model::diff::nodes_by_path;
use crate::model::retainers::{Made, Step, is_application, is_class, made_of, steps_of};
use crate::profile::{Profile, Sums};

/// One object holding much, or many objects of one class that together do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One object, or one class's static fields.
    One,
    /// Many objects of one class, from no one place.
    Many,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::One => "one",
            Kind::Many => "many",
        }
    }
}

/// What kind of memory a suspect holds, which is what tells a leak from a
/// footprint in a single dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Many objects of one class in a few collections. It grows with what it
    /// holds, and that is how a leak looks: a second dump tells, if they are more.
    Collection,
    /// A few large objects. Bounded, unless their number grows: footprint.
    Large,
    /// Many objects of one class, held from many places: what the application
    /// is made of, a leak only if their number grows.
    Instances,
    /// One object holding much over many branches, none of them most: a root
    /// of the application rather than a container.
    Spread,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Collection => "collection",
            Verdict::Large => "large",
            Verdict::Instances => "instances",
            Verdict::Spread => "spread",
        }
    }
}

/// Where under a suspect its memory gathers, on its heaviest branch: the
/// first class of the application with many objects there.
#[derive(Debug, Clone)]
pub struct Gathered {
    pub node: usize,
    /// Of the class.
    pub name: String,
    /// Of that class there; of a growth, those it gained.
    pub objects: f64,
    /// Of that class there in the dump itself: the same as `objects`, but for
    /// a growth. None when the dump does not count objects.
    pub there: Option<f64>,
    /// Bytes they hold.
    pub total: f64,
    /// The classes from the suspect to them, both left out.
    pub through: Vec<String>,
    /// The nearest array above them, the backing of a collection: how many
    /// collections they are in. None when no array is on the way.
    pub containers: Option<Containers>,
}

#[derive(Debug, Clone)]
pub struct Containers {
    pub name: String,
    pub objects: f64,
}

/// A class whose objects are mostly under a suspect.
#[derive(Debug, Clone)]
pub struct Included {
    pub name: String,
    /// Under the suspect.
    pub objects: f64,
    /// Of the class in the heap.
    pub all: f64,
    /// Bytes they hold under the suspect.
    pub total: f64,
}

#[derive(Debug, Clone)]
pub struct Suspect {
    pub kind: Kind,
    /// Of the class.
    pub name: String,
    /// One: the object; many: the heaviest place of the class.
    pub node: usize,
    /// Bytes held, the objects themselves included.
    pub total: f64,
    /// Of the heap, 0 to 1.
    pub share: f64,
    /// One: held; many: of the class. None when the profile does not count them.
    pub objects: Option<f64>,
    /// One: the objects at its place, 1 but for a few large objects of the
    /// JDK held at one place, as two buffers in threads' local variables.
    pub at: f64,
    /// One: the static fields of the class, not an instance.
    pub statics: bool,
    /// One: where its memory gathers, None when it spreads.
    pub gathered: Option<Gathered>,
    /// The classes with many objects folded into it, the heaviest first.
    pub includes: Vec<Included>,
    /// What it holds, by class, the heaviest first.
    pub made_of: Vec<Made>,
    /// Who holds it, the nearest first, in steps.
    pub holders: Vec<Step>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone)]
pub struct Leaks {
    /// The heaviest first.
    pub suspects: Vec<Suspect>,
    /// The bytes the shares are of.
    pub heap: f64,
}

/// How much of a holder a more specific suspect under it must hold for the
/// holder to be only the way to it; and how much of a class must be under a
/// suspect to be folded into it.
const COVERED: f64 = 0.7;

/// How much of a suspect the branch followed to where it gathers must hold:
/// half, as a map's entries and its values share what it holds.
const BRANCH: f64 = 0.5;

/// How much of what an object holds must be its own bytes for it to be one
/// large object.
const OWN: f64 = 0.8;

/// How many objects of one class under a suspect make a collection rather
/// than a few large objects.
pub const MANY: f64 = 100.0;

/// The suspects of a profile, the heaviest first.
///
/// `share` is the part of the heap, 0 to 1, a suspect holds at least; MAT's
/// is 10%. `like` is the dump whose places these are, when the profile is
/// what grew between two: whether a place is one object, a class's static
/// fields or a few collections is said by the dump, not by what it gained.
pub fn leaks_of(p: &Profile, share: f64, like: Option<&Profile>) -> Leaks {
    Finder::new(p, share, like).find()
}

/// The suspect a class may be folded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Into {
    /// An index in `Finder::suspects`.
    One(usize),
    /// An index in the suspects of many objects.
    Many(usize),
}

struct Finder<'a> {
    p: &'a Profile,
    s: &'a Sums,
    heap: f64,
    least: f64,
    /// The sums of the dump the places are of: `p`'s own, or `like`'s.
    shape: &'a Sums,
    /// Each node's place in that dump, -1 for none; None when it is `p`.
    place: Option<Vec<i64>>,
    /// Reached from a root, not under `(unreachable)`.
    reached: Vec<bool>,
    /// The suspects of one object.
    suspects: Vec<Suspect>,
    /// A node in the region of a suspect of one object: itself, and the
    /// holders above it that it is most of.
    region: FxHashMap<usize, usize>,
}

impl<'a> Finder<'a> {
    fn new(p: &'a Profile, share: f64, like: Option<&'a Profile>) -> Self {
        let s = p.sums();
        let heap = p.heap();
        let place = like.map(|like| {
            let places = nodes_by_path(like);
            let mut place = vec![-1i64; p.len()];
            for (key, n) in nodes_by_path(p) {
                place[n] = places.get(&key).map_or(-1, |&m| m as i64);
            }
            place
        });
        // what nothing reaches is garbage, not a leak
        let mut reached = vec![false; p.len()];
        for &r in &s.roots {
            if p.name(r) == "(unreachable)" {
                continue;
            }
            let mut stack = vec![r];
            while let Some(n) = stack.pop() {
                reached[n] = true;
                stack.extend_from_slice(&s.children[n]);
            }
        }
        Finder {
            p,
            s,
            heap,
            least: share * heap,
            shape: like.map_or(s, |l| l.sums()),
            place,
            reached,
            suspects: Vec::new(),
            region: FxHashMap::default(),
        }
    }

    fn name(&self, n: usize) -> &'a str {
        self.p.name(n)
    }

    fn objects_at(&self, n: usize) -> Option<f64> {
        self.s.objects.as_ref().map(|o| o[n])
    }

    fn place_of(&self, n: usize) -> i64 {
        match &self.place {
            Some(place) => place[n],
            None => n as i64,
        }
    }

    /// The objects at a place in the dump, None when it does not count them.
    fn objects_there(&self, n: usize) -> Option<f64> {
        let m = self.place_of(n);
        match &self.shape.objects {
            Some(o) if m >= 0 => Some(o[m as usize]),
            _ => None,
        }
    }

    /// One object at a path, or none but the class object, whose own size is
    /// 0 and which holds the class's static fields. A profile without counts
    /// cannot tell one from many: every place counts as one.
    fn single(&self, n: usize) -> bool {
        self.objects_there(n).is_none_or(|o| o <= 1.0)
    }

    fn statics(&self, n: usize) -> bool {
        let m = self.place_of(n);
        match &self.shape.objects {
            Some(o) if m >= 0 => o[m as usize] == 0.0 && self.shape.own[m as usize] == 0.0,
            _ => false,
        }
    }

    fn find(mut self) -> Leaks {
        let p = self.p;
        // --- One object holding much: the deepest of those that hold about as much ---
        // The application's objects first; then what no suspect of the
        // application covers, a JDK object or an array held straight from a
        // root, as a buffer in a thread's local variable. A JDK object whose
        // memory goes through an object of the application, as a class
        // loader's to a class's static fields, is the application's: judged
        // there, under the share or over it.
        let first = self.candidates(|f, n| f.single(n) && is_application(f.name(n)));
        self.add_ones(first);
        let second = self.candidates(|f, n| {
            f.objects_there(n).unwrap_or(0.0) < MANY
                && is_class(f.name(n))
                && !is_application(f.name(n))
                && !f.covered(n)
                && !f.branch_of(n).iter().any(|&b| f.single(b) && is_application(f.name(b)))
        });
        self.add_ones(second);
        for i in 0..self.suspects.len() {
            self.suspects[i].verdict = self.verdict_of(&self.suspects[i]);
        }

        // --- Many objects of one class holding much ---
        struct Entry {
            name: String,
            total: f64,
            objects: f64,
            nodes: Vec<usize>,
        }
        let mut by_class: Vec<Entry> = Vec::new();
        let mut class_index: FxHashMap<&str, usize> = FxHashMap::default();
        for n in 0..p.len() {
            let c = self.name(n);
            if !self.reached[n] || !is_application(c) {
                continue;
            }
            let at = *class_index.entry(c).or_insert_with(|| {
                by_class.push(Entry { name: c.to_string(), total: 0.0, objects: 0.0, nodes: Vec::new() });
                by_class.len() - 1
            });
            let entry = &mut by_class[at];
            entry.objects += self.objects_at(n).unwrap_or(0.0);
            // the outermost of the class only: what one holds of another is in its total already
            let mut nested = false;
            let mut a = p.parent_of(n);
            while let Some(i) = a {
                if p.name(i) == c {
                    nested = true;
                    break;
                }
                a = p.parent_of(i);
            }
            if !nested {
                entry.total += self.s.total[n];
                entry.nodes.push(n);
            }
        }
        let mut classes: Vec<&Entry> = by_class
            .iter()
            .filter(|e| e.total >= self.least && self.s.objects.is_some() && e.objects >= 2.0)
            .collect();
        classes.sort_by(|a, b| b.total.total_cmp(&a.total));

        let mut many: Vec<Suspect> = Vec::new();
        // the outermost places of each, to fold a class nested in them
        let mut many_nodes: Vec<FxHashSet<usize>> = Vec::new();
        for entry in classes {
            let c = entry.name.as_str();
            // folded into the suspect most of it hangs from
            let mut under: Vec<(Into, f64, f64)> = Vec::new();
            for &n in &entry.nodes {
                // part of a suspect when on its way, the place itself in its
                // region, or under its object; under a holder on its way only,
                // it is beside the suspect, not in it
                let mut into = None;
                let mut above = false;
                let mut a = Some(n);
                while let Some(i) = a {
                    if let Some(&r) = self.region.get(&i) {
                        let node = self.suspects[r].node;
                        above = i == n && i != node;
                        into = (i == n || i == node).then_some(Into::One(r));
                        break;
                    }
                    if let Some(m) = many_nodes.iter().position(|nodes| nodes.contains(&i)) {
                        into = Some(Into::Many(m));
                        break;
                    }
                    a = p.parent_of(i);
                }
                let Some(into) = into else { continue };
                // a holder on the way is the objects at that place, not what
                // it holds beside the suspect
                let objects = if above { self.objects_at(n).unwrap_or(0.0) } else { self.count_of(n, c) };
                match under.iter_mut().find(|u| u.0 == into) {
                    Some(u) => {
                        u.1 += self.s.total[n];
                        u.2 += objects;
                    }
                    None => under.push((into, self.s.total[n], objects)),
                }
            }
            let mut most: Option<(Into, f64, f64)> = None;
            for &u in &under {
                if most.is_none_or(|m| u.1 > m.1) {
                    most = Some(u);
                }
            }
            if let Some((into, total, objects)) = most
                && total >= COVERED * entry.total
            {
                let target = match into {
                    Into::One(i) => &mut self.suspects[i],
                    Into::Many(i) => &mut many[i],
                };
                // one object of the class on the way to the memory is the
                // chain, not a class of many
                if target.name != c && objects >= 2.0 {
                    target.includes.push(Included { name: c.to_string(), objects, all: entry.objects, total });
                }
                continue;
            }
            let mut heaviest = entry.nodes[0];
            for &b in &entry.nodes[1..] {
                if self.s.total[b] > self.s.total[heaviest] {
                    heaviest = b;
                }
            }
            many.push(Suspect {
                kind: Kind::Many,
                name: c.to_string(),
                node: heaviest,
                total: entry.total,
                share: if self.heap > 0.0 { entry.total / self.heap } else { 0.0 },
                objects: Some(entry.objects),
                at: self.objects_at(heaviest).unwrap_or(1.0),
                statics: false,
                gathered: None,
                includes: Vec::new(),
                made_of: Vec::new(),
                holders: steps_of(p, heaviest, 0.1),
                verdict: Verdict::Instances,
            });
            many_nodes.push(entry.nodes.iter().copied().collect());
        }

        let mut suspects = self.suspects;
        suspects.extend(many);
        for x in &mut suspects {
            x.includes.sort_by(|a, b| b.total.total_cmp(&a.total));
        }
        suspects.sort_by(|a, b| b.total.total_cmp(&a.total));
        Leaks { suspects, heap: self.heap }
    }

    /// The nodes over the share that `take` takes, the deepest of those that
    /// hold about as much: a holder with one of them on its heaviest branch,
    /// half of it, is only the way to it.
    fn candidates(&self, take: impl Fn(&Self, usize) -> bool) -> Vec<usize> {
        let p = self.p;
        let deepest: Vec<usize> = (0..p.len())
            .filter(|&n| self.reached[n] && p.parent[n] >= 0 && self.s.total[n] >= self.least && take(self, n))
            .collect();
        let mut kept = vec![false; p.len()];
        for &n in &deepest {
            kept[n] = true;
        }
        for &n in &deepest {
            let mut a = p.parent_of(n);
            while let Some(i) = a {
                if kept[i] && self.s.total[n] >= BRANCH * self.s.total[i] {
                    kept[i] = false;
                }
                a = p.parent_of(i);
            }
        }
        deepest.into_iter().filter(|&n| kept[n]).collect()
    }

    /// Makes suspects of one object of the nodes, each named by itself, the
    /// most specific. The holders above it that hold about as much, the
    /// application's class that keeps a library's or the server that keeps a
    /// cache, are its first step of holders, and its region.
    fn add_ones(&mut self, nodes: Vec<usize>) {
        let p = self.p;
        let mut added: Vec<Suspect> = nodes
            .into_iter()
            .map(|n| Suspect {
                kind: Kind::One,
                name: self.name(n).to_string(),
                node: n,
                total: self.s.total[n],
                share: if self.heap > 0.0 { self.s.total[n] / self.heap } else { 0.0 },
                objects: self.s.held.as_ref().map(|h| h[n]),
                at: self.objects_there(n).unwrap_or(1.0).max(1.0),
                statics: self.statics(n),
                gathered: self.gathered_under(n),
                includes: Vec::new(),
                made_of: made_of(p, n),
                holders: steps_of(p, n, 0.1),
                verdict: Verdict::Spread,
            })
            .collect();
        // A suspect's region: itself, and the holders above it that it is
        // most of, the ones that were only the way to it. A class whose
        // objects are there is part of the suspect, the library's Git above
        // its pack files as much as the entries below a map.
        added.sort_by(|a, b| b.total.total_cmp(&a.total));
        for x in added {
            let index = self.suspects.len();
            let mut a = Some(x.node);
            while let Some(i) = a {
                if self.s.total[x.node] < COVERED * self.s.total[i] {
                    break;
                }
                self.region.entry(i).or_insert(index);
                a = p.parent_of(i);
            }
            self.suspects.push(x);
        }
    }

    /// The heaviest branch under a node, as long as it holds half of it: the
    /// nearest first, the node itself left out.
    fn branch_of(&self, top: usize) -> Vec<usize> {
        let s = self.s;
        let mut path = Vec::new();
        let mut cur = top;
        loop {
            let mut heaviest: Option<usize> = None;
            for &c in &s.children[cur] {
                if heaviest.is_none_or(|h| s.total[c] > s.total[h]) {
                    heaviest = Some(c);
                }
            }
            match heaviest {
                Some(h) if s.total[h] >= BRANCH * s.total[top] => {
                    path.push(h);
                    cur = h;
                }
                _ => return path,
            }
        }
    }

    /// Whether a node is a suspect's already: in its region, or under it.
    fn covered(&self, n: usize) -> bool {
        let mut a = Some(n);
        while let Some(i) = a {
            if let Some(&r) = self.region.get(&i) {
                return i == n || i == self.suspects[r].node;
            }
            a = self.p.parent_of(i);
        }
        false
    }

    fn verdict_of(&self, x: &Suspect) -> Verdict {
        match &x.gathered {
            Some(g) if g.objects >= MANY => Verdict::Collection,
            Some(_) => Verdict::Large,
            // an array, or an object whose own bytes are most of what it holds
            None if self.s.own[x.node] >= OWN * self.s.total[x.node] => Verdict::Large,
            None => Verdict::Spread,
        }
    }

    /// Where the memory under one object gathers: down its heaviest branch,
    /// as long as the branch holds half of it, to the first class of the
    /// application with many objects there, the element of a collection; else
    /// to the deepest class of the application with several, else to the
    /// deepest place with several, a collection's entries.
    fn gathered_under(&self, top: usize) -> Option<Gathered> {
        let path = self.branch_of(top);
        let at = path
            .iter()
            .position(|&n| self.objects_there(n).unwrap_or(0.0) >= MANY && is_application(self.name(n)))
            .or_else(|| path.iter().rposition(|&n| !self.single(n) && is_application(self.name(n))))
            .or_else(|| path.iter().rposition(|&n| !self.single(n)))?;
        let found = path[at];
        // the nearest array on the way: the backing of the collection they are in
        let array = path[..at].iter().rev().copied().find(|&n| self.name(n).ends_with("[]"));
        Some(Gathered {
            node: found,
            name: self.name(found).to_string(),
            objects: self.objects_at(found).unwrap_or(0.0),
            there: self.objects_there(found),
            total: self.s.total[found],
            through: path[..at].iter().map(|&n| self.name(n).to_string()).collect(),
            containers: array.map(|a| Containers {
                name: self.name(a).to_string(),
                objects: self.objects_there(a).unwrap_or(0.0),
            }),
        })
    }

    /// The objects of a class at a place and under it.
    fn count_of(&self, top: usize, c: &str) -> f64 {
        let mut count = 0.0;
        let mut stack = vec![top];
        while let Some(n) = stack.pop() {
            if self.name(n) == c {
                count += self.objects_at(n).unwrap_or(0.0);
            }
            stack.extend_from_slice(&self.s.children[n]);
        }
        count
    }
}
