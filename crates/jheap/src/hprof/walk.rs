//! The walk from the roots, gathering objects into paths.

use super::graph::Graph;
use crate::hash::FxHashMap;

/// The longest path kept, in frames. A deeper object is filed under its last
/// frames, so the paths stay bounded however deep the heap runs.
const PATH_CAP: u32 = 31;

/// No node: the parent of a root's.
const NONE: u32 = u32::MAX;

/// A path's frames, root first, with the bytes and objects that end there.
pub(crate) type Aggregated = Vec<(Box<[u32]>, u64, u64)>;

pub(crate) struct Walked {
    pub(crate) paths: Aggregated,
    pub(crate) reachable_size: u64,
    pub(crate) unreachable_size: u64,
    pub(crate) max_depth_reached: u64,
    pub(crate) depth_limit_hits: u64,
}

/// The tree of paths: a node per (parent, frame), so that objects which share
/// a path share a node, and the tree stays small (15k nodes for 2.5M objects).
/// No per-object array or string is ever built.
///
/// Past the cap a node is its window, the last PATH_CAP frames, not its whole
/// path: chains that end the same way share it, so a linked list of a million
/// is a few dozen nodes rather than a million.
#[derive(Default)]
struct Tree {
    parent: Vec<u32>,
    frame: Vec<u32>,
    /// In frames from the root type, PATH_CAP for a window.
    depth: Vec<u8>,
    /// The bytes of the objects whose path ends here, not of those under it.
    own: Vec<u64>,
    objects: Vec<u64>,
    children: FxHashMap<(u32, u32), u32>,
    /// The frames of a window, the oldest first.
    window_of: FxHashMap<u32, Box<[u32]>>,
    /// The node of each window, by its frames.
    windows: FxHashMap<Box<[u32]>, u32>,
}

impl Tree {
    fn make(&mut self, parent: u32, frame: u32, depth: u32) -> u32 {
        self.parent.push(parent);
        self.frame.push(frame);
        self.depth.push(depth as u8);
        self.own.push(0);
        self.objects.push(0);
        (self.parent.len() - 1) as u32
    }

    /// Its frames from the root type, the root first.
    fn path_of(&self, node: u32) -> Vec<u32> {
        let mut frames = Vec::new();
        let mut n = node;
        while n != NONE {
            frames.push(self.frame[n as usize]);
            n = self.parent[n as usize];
        }
        frames.reverse();
        frames
    }

    fn frames_of(&self, node: u32) -> Box<[u32]> {
        match self.window_of.get(&node) {
            Some(window) => window.clone(),
            None => self.path_of(node).into_boxed_slice(),
        }
    }

    /// The node of an object of that frame held from `parent`, made when first seen.
    #[inline]
    fn node_of(&mut self, parent: u32, frame: u32) -> u32 {
        if let Some(&node) = self.children.get(&(parent, frame)) {
            return node;
        }
        let depth = if parent == NONE { 0 } else { self.depth[parent as usize] as u32 + 1 };
        let node = if depth < PATH_CAP {
            self.make(parent, frame, depth)
        } else {
            // past the cap: the holder's window slides one frame on
            let held = self.frames_of(parent);
            let mut frames = held[1..].to_vec();
            frames.push(frame);
            match self.windows.get(frames.as_slice()) {
                Some(&node) => node,
                None => {
                    let node = self.make(parent, frame, PATH_CAP);
                    let frames = frames.into_boxed_slice();
                    self.windows.insert(frames.clone(), node);
                    self.window_of.insert(node, frames);
                    node
                }
            }
        };
        self.children.insert((parent, frame), node);
        node
    }
}

/// Queues an object reached from `parent`, its node written over its frame.
#[inline]
fn enqueue(tree: &mut Tree, queue: &mut Vec<u32>, frame: &mut [u32], defined: usize, index: u32, parent: u32) {
    let i = index as usize;
    // An id no record defines has no frame: it weighs nothing and holds
    // nothing, so its node would hold nothing either
    if i < defined {
        frame[i] = tree.node_of(parent, frame[i]);
    }
    queue.push(index);
}

/// Breadth first from the roots, each object reached once, by the first
/// holder the walk finds: a spanning tree, whose paths are gathered as it goes.
///
/// An object's node is worked out when it is queued and written over its
/// frame, which nothing needs after: the queue then holds the object alone.
pub(crate) fn walk(g: &mut Graph, unreachable: u32, progress: &mut dyn FnMut(u32)) -> Walked {
    let defined = g.defined as usize;
    let total = g.total as usize;
    let Graph { frame, size, offsets, refs, roots, .. } = g;
    let mut tree = Tree::default();
    let mut seen = vec![0u64; total.div_ceil(64)];
    let mut queue: Vec<u32> = Vec::with_capacity(total);

    for &(root_frame, index) in roots.iter() {
        let (word, bit) = (index as usize / 64, 1u64 << (index % 64));
        if seen[word] & bit != 0 {
            continue;
        }
        seen[word] |= bit;
        let node = tree.node_of(NONE, root_frame);
        enqueue(&mut tree, &mut queue, frame, defined, index, node);
    }

    let (mut reachable_size, mut max_depth_reached, mut depth_limit_hits) = (0u64, 0u64, 0u64);
    // An object's depth is its level in the walk, which a window no longer
    // says: the objects queued before the level's end are the ones a step nearer
    let (mut level, mut level_end) = (0u64, queue.len());
    let mut head = 0;
    while head < queue.len() {
        if head == level_end {
            level += 1;
            level_end = queue.len();
        }
        let index = queue[head] as usize;
        head += 1;
        // in frames from the root type, which is 0: a root object is 1
        let depth = level + 1;
        if depth >= PATH_CAP as u64 {
            depth_limit_hits += 1;
        }
        max_depth_reached = max_depth_reached.max(depth + 1);
        if index >= defined {
            continue;
        }

        // a class object is metadata outside the heap: it weighs nothing, but
        // the walk goes through it
        let node = frame[index];
        let bytes = size[index] as u64 * 8;
        tree.own[node as usize] += bytes;
        if bytes > 0 {
            tree.objects[node as usize] += 1;
        }
        reachable_size += bytes;

        // Every reference, once: the flag is what stops it coming back
        let (start, end) = offsets.range(index);
        for slice in refs.slices(start, end) {
            for &held in slice {
                let (word, bit) = (held as usize / 64, 1u64 << (held % 64));
                if seen[word] & bit != 0 {
                    continue;
                }
                seen[word] |= bit;
                enqueue(&mut tree, &mut queue, frame, defined, held, node);
            }
        }

        if head & 0xfffff == 0 {
            progress((100 * head / total.max(1)) as u32);
        }
    }
    drop(queue);

    // The paths, once: a node's path is up its parents, at most PATH_CAP of
    // them, or its window past the cap
    let mut paths: FxHashMap<Box<[u32]>, (u64, u64)> = FxHashMap::default();
    for node in 0..tree.parent.len() {
        if tree.own[node] == 0 {
            continue;
        }
        let entry = paths.entry(tree.frames_of(node as u32)).or_default();
        entry.0 += tree.own[node];
        entry.1 += tree.objects[node];
    }

    // What the roots could not reach: a class of its own, so the report says
    // how much it is without it being taken for a holder
    let mut unreachable_size = 0;
    for index in 0..defined {
        let bytes = size[index] as u64 * 8;
        if seen[index / 64] & (1 << (index % 64)) != 0 || bytes == 0 {
            continue;
        }
        unreachable_size += bytes;
        let entry = paths.entry(Box::new([unreachable, frame[index]])).or_default();
        entry.0 += bytes;
        entry.1 += 1;
    }

    Walked {
        paths: paths.into_iter().map(|(frames, (bytes, objects))| (frames, bytes, objects)).collect(),
        reachable_size,
        unreachable_size,
        max_depth_reached,
        depth_limit_hits,
    }
}
