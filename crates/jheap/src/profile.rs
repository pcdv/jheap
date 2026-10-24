//! A heap profile: the paths of a parse as a tree of nodes, as jfr-web's
//! profiles hold stacks. The models read it, and it is what is kept beside a
//! dump and what the web app draws.

use std::sync::OnceLock;

use crate::hash::FxHashMap;
use crate::hprof::Parse;

/// The type of every frame of a heap profile, in a frame's low 3 bits.
pub const COMPILED: u32 = 0;

/// The paths as a tree: a node per distinct path, its frame the class of the
/// objects that end there, and a sample per path with its bytes and objects.
///
/// Weights and counts are f64, as in heap-landscape: the models compare
/// shares of them, and the same arithmetic gives the same suspects.
#[derive(Debug)]
pub struct Profile {
    pub strings: Vec<String>,
    /// The parent of each node, -1 for a root.
    pub parent: Vec<i32>,
    /// The frame of each node: its string << 3 | its type.
    pub frame: Vec<u32>,
    /// The node of each sample, -1 for none.
    pub stack: Vec<i32>,
    /// The bytes of each sample.
    pub weight: Vec<f64>,
    /// The objects of each sample; None for a profile saved before they were
    /// counted.
    pub objects: Option<Vec<f64>>,
    sums: OnceLock<Sums>,
}

/// What each node holds, summed once per profile: every model asks for it.
#[derive(Debug)]
pub struct Sums {
    /// The bytes each node holds, its contents included.
    pub total: Vec<f64>,
    /// The bytes of the samples that end at each node.
    pub own: Vec<f64>,
    /// The objects that end at each node, when the profile counts them.
    pub objects: Option<Vec<f64>>,
    /// The objects each node holds, itself and its contents.
    pub held: Option<Vec<f64>>,
    pub children: Vec<Vec<usize>>,
    pub roots: Vec<usize>,
}

impl Profile {
    pub fn new(
        strings: Vec<String>,
        parent: Vec<i32>,
        frame: Vec<u32>,
        stack: Vec<i32>,
        weight: Vec<f64>,
        objects: Option<Vec<f64>>,
    ) -> Profile {
        Profile { strings, parent, frame, stack, weight, objects, sums: OnceLock::new() }
    }

    /// The profile of paths, each a key of its frames joined by `;` with its
    /// bytes and objects, as heap-landscape's `stacksToProfile`. `counted`
    /// says whether the objects are counts or nothing.
    ///
    /// The paths are sorted by key first, so the same paths give the same
    /// profile whatever order they came in.
    pub fn from_stacks(mut stacks: Vec<(String, f64, f64)>, counted: bool) -> Profile {
        stacks.sort_by(|a, b| a.0.cmp(&b.0));
        let mut strings = vec![String::new()];
        let mut string_index: FxHashMap<String, u32> = FxHashMap::default();
        let (mut parent, mut frame) = (Vec::new(), Vec::new());
        let mut nodes: FxHashMap<(i32, u32), i32> = FxHashMap::default();
        let (mut stack, mut weight, mut count) = (Vec::new(), Vec::new(), Vec::new());
        for (path, size, objects) in stacks {
            if size <= 0.0 {
                continue;
            }
            let mut node = -1;
            for name in path.split(';') {
                let s = match string_index.get(name) {
                    Some(&i) => i,
                    None => {
                        strings.push(name.to_string());
                        let i = (strings.len() - 1) as u32;
                        string_index.insert(name.to_string(), i);
                        i
                    }
                };
                let f = s << 3 | COMPILED;
                node = *nodes.entry((node, f)).or_insert_with(|| {
                    parent.push(node);
                    frame.push(f);
                    (parent.len() - 1) as i32
                });
            }
            stack.push(node);
            weight.push(size);
            count.push(objects);
        }
        Profile::new(strings, parent, frame, stack, weight, counted.then_some(count))
    }

    /// The profile of a parse.
    pub fn from_parse(p: &Parse) -> Profile {
        let stacks = p
            .paths
            .iter()
            .map(|path| (p.frames(path).collect::<Vec<_>>().join(";"), path.bytes as f64, path.objects as f64))
            .collect();
        Profile::from_stacks(stacks, true)
    }

    /// The number of nodes.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// The name of a node's frame: a class, a root type.
    #[inline]
    pub fn name(&self, node: usize) -> &str {
        &self.strings[(self.frame[node] >> 3) as usize]
    }

    /// The node's parent, if it has one.
    #[inline]
    pub fn parent_of(&self, node: usize) -> Option<usize> {
        let p = self.parent[node];
        (p >= 0).then_some(p as usize)
    }

    /// The names from the root to the node.
    pub fn path_of(&self, node: usize) -> Vec<String> {
        let mut frames = Vec::new();
        let mut n = Some(node);
        while let Some(i) = n {
            frames.push(self.name(i).to_string());
            n = self.parent_of(i);
        }
        frames.reverse();
        frames
    }

    /// The bytes of the whole tree.
    pub fn heap(&self) -> f64 {
        let s = self.sums();
        s.roots.iter().map(|&r| s.total[r]).sum()
    }

    /// The sums, made on first use and kept with the profile.
    pub fn sums(&self) -> &Sums {
        self.sums.get_or_init(|| self.sum())
    }

    fn sum(&self) -> Sums {
        let n = self.len();
        let (mut total, mut own) = (vec![0.0; n], vec![0.0; n]);
        let counted = self.objects.as_ref();
        let mut objects = counted.map(|_| vec![0.0; n]);
        let mut held = counted.map(|_| vec![0.0; n]);
        for (i, &node) in self.stack.iter().enumerate() {
            if node < 0 {
                continue;
            }
            let node = node as usize;
            let w = self.weight[i];
            own[node] += w;
            total[node] += w;
            if let (Some(c), Some(o), Some(h)) = (counted, objects.as_mut(), held.as_mut()) {
                o[node] += c[i];
                h[node] += c[i];
            }
        }
        let mut children = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for i in 0..n {
            match self.parent_of(i) {
                Some(p) => children[p].push(i),
                None => roots.push(i),
            }
        }
        // depth first, then the reverse: every child before its parent
        let mut order = Vec::with_capacity(n);
        let mut stack = roots.clone();
        while let Some(node) = stack.pop() {
            order.push(node);
            stack.extend_from_slice(&children[node]);
        }
        for &node in order.iter().rev() {
            if let Some(p) = self.parent_of(node) {
                total[p] += total[node];
                if let Some(h) = held.as_mut() {
                    h[p] += h[node];
                }
            }
        }
        Sums { total, own, objects, held, children, roots }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_a_tree() {
        let p = Profile::from_stacks(
            vec![
                ("Root;A;B".into(), 10.0, 1.0),
                ("Root;A".into(), 5.0, 2.0),
                ("Root;C".into(), 1.0, 1.0),
                ("Root;D".into(), 0.0, 0.0),
            ],
            true,
        );
        // Root, A, B, C: the empty path is dropped
        assert_eq!(p.len(), 4);
        let s = p.sums();
        assert_eq!(s.roots, vec![0]);
        assert_eq!(s.total[0], 16.0);
        assert_eq!(p.heap(), 16.0);
        let a = s.children[0].iter().copied().find(|&c| p.name(c) == "A").unwrap();
        assert_eq!((s.total[a], s.own[a]), (15.0, 5.0));
        assert_eq!(s.held.as_ref().unwrap()[a], 3.0);
        assert_eq!(p.path_of(s.children[a][0]), vec!["Root", "A", "B"]);
    }
}
