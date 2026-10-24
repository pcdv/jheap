//! A profile as the JSON document jfr-web's pages read (version 4), which
//! heap-landscape's web app reads too: strings sorted and front-coded, stack
//! nodes depth first, a sample per path. A heap profile has no time: its
//! samples are all at 0 and its weights are bytes. It has one column
//! jfr-web's have not, `samples.objects`, the objects of each sample, which
//! jfr-web ignores.

use serde::{Deserialize, Serialize};

use super::Profile;

/// The version of jfr-web's format.
pub const VERSION: u32 = 4;

/// The name of the one thread of a heap profile.
const THREAD: &str = "all";

#[derive(Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub unit: String,
    pub weights: Vec<f64>,
    pub start: String,
    pub end: f64,
    pub strings: Vec<String>,
    /// Depth and frame of each node, depth first.
    pub stacks: Vec<i64>,
    pub samples: Samples,
    /// Name and id of each thread.
    pub threads: Vec<(i64, i64)>,
    pub operations: Operations,
}

#[derive(Serialize, Deserialize)]
pub struct Samples {
    pub time: Vec<f64>,
    pub stack: Vec<i64>,
    pub thread: Vec<i64>,
    pub operation: Vec<i64>,
    pub kind: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<Vec<f64>>,
}

/// A heap profile has none: the fields are there for jfr-web's pages.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Operations {
    #[serde(default)]
    pub name: Vec<i64>,
    #[serde(default)]
    pub start: Vec<f64>,
    #[serde(default)]
    pub duration: Vec<f64>,
    #[serde(default)]
    pub thread: Vec<i64>,
    #[serde(default)]
    pub detail: Vec<i64>,
    #[serde(default)]
    pub status: Vec<i64>,
    #[serde(default)]
    pub failed: Vec<u8>,
    #[serde(default)]
    pub error: Vec<i64>,
    #[serde(default)]
    pub r#async: Vec<u8>,
    #[serde(default)]
    pub bytes_in: Vec<f64>,
    #[serde(default)]
    pub bytes_out: Vec<f64>,
}

/// The UTF-16 units of a string: front coding counts them, as JavaScript does.
fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Document {
    /// The document of a profile.
    pub fn of(p: &Profile) -> Document {
        // names sorted, for their front coding, as JavaScript sorts: by UTF-16 unit
        let mut names: Vec<&str> = p.frame.iter().map(|&f| p.strings[(f >> 3) as usize].as_str()).collect();
        names.push(THREAD);
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        names.dedup();
        let rank: std::collections::HashMap<&str, usize> = names.iter().enumerate().map(|(i, &s)| (s, i)).collect();

        let mut strings = Vec::with_capacity(names.len());
        let mut previous: Vec<u16> = Vec::new();
        for name in &names {
            let name = units(name);
            // 95 at most: the character stays printable ASCII
            let max = previous.len().min(name.len()).min(95);
            let mut shared = 0;
            while shared < max && previous[shared] == name[shared] {
                shared += 1;
            }
            // never between the halves of a surrogate pair, which a Rust
            // string cannot hold apart
            if shared > 0 && shared < name.len() && (0xDC00..0xE000).contains(&name[shared]) {
                shared -= 1;
            }
            let mut coded = vec![(shared + 32) as u16];
            coded.extend_from_slice(&name[shared..]);
            strings.push(String::from_utf16(&coded).expect("whole surrogate pairs"));
            previous = name;
        }

        // nodes depth first, each renumbered by its position
        let n = p.len();
        let s = p.sums();
        let mut position = vec![0i64; n];
        let mut depth = vec![0i64; n];
        let mut order = Vec::with_capacity(n);
        let mut stack: Vec<usize> = s.roots.iter().rev().copied().collect();
        while let Some(node) = stack.pop() {
            depth[node] = p.parent_of(node).map_or(0, |parent| depth[parent] + 1);
            position[node] = order.len() as i64;
            order.push(node);
            stack.extend(s.children[node].iter().rev());
        }
        let mut stacks = Vec::with_capacity(2 * n);
        for &node in &order {
            let frame = p.frame[node];
            let name = rank[p.strings[(frame >> 3) as usize].as_str()] as i64;
            stacks.push(depth[node]);
            stacks.push(name << 3 | (frame & 7) as i64);
        }

        let samples = p.stack.len();
        Document {
            version: VERSION,
            unit: "bytes".into(),
            weights: vec![1.0, 1.0],
            start: String::new(),
            end: 0.0,
            strings,
            stacks,
            samples: Samples {
                time: vec![0.0; samples],
                stack: p.stack.iter().map(|&node| if node < 0 { -1 } else { position[node as usize] }).collect(),
                thread: vec![0; samples],
                operation: vec![-1; samples],
                kind: vec![0; samples],
                weight: Some(p.weight.clone()),
                objects: p.objects.clone(),
            },
            threads: vec![(rank[THREAD] as i64, 0)],
            operations: Operations::default(),
        }
    }

    /// The profile of a document.
    pub fn profile(&self) -> Result<Profile, String> {
        if self.version != VERSION {
            return Err(format!("a profile of version {}, where version {VERSION} is read", self.version));
        }
        // front-coded: the first character tells the length, in UTF-16 units,
        // of the prefix shared with the previous string
        let mut strings = Vec::with_capacity(self.strings.len());
        let mut previous: Vec<u16> = Vec::new();
        for s in &self.strings {
            let coded = units(s);
            let Some(&first) = coded.first() else {
                return Err("a string of the profile is empty, without its prefix length".into());
            };
            previous.truncate((first as usize).saturating_sub(32));
            previous.extend_from_slice(&coded[1..]);
            strings.push(String::from_utf16_lossy(&previous));
        }

        // depth first: a node's parent is the last node one level up
        let n = self.stacks.len() / 2;
        let (mut parent, mut frame) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut last: Vec<i32> = Vec::new();
        for i in 0..n {
            let depth = self.stacks[2 * i] as usize;
            if depth > last.len() {
                return Err(format!("node {i} is {depth} deep, under no node {} deep", depth - 1));
            }
            let f = self.stacks[2 * i + 1];
            if f < 0 || (f >> 3) as usize >= strings.len() {
                return Err(format!("node {i} names no string of the profile"));
            }
            parent.push(if depth == 0 { -1 } else { last[depth - 1] });
            frame.push(f as u32);
            last.truncate(depth);
            last.push(i as i32);
        }

        let samples = &self.samples;
        let stack: Vec<i32> = samples.stack.iter().map(|&s| if s < 0 || s as usize >= n { -1 } else { s as i32 }).collect();
        let weight = match &samples.weight {
            Some(w) => w.clone(),
            None => samples.kind.iter().map(|&k| self.weights.get(k as usize).copied().unwrap_or(0.0)).collect(),
        };
        if weight.len() != stack.len() || samples.objects.as_ref().is_some_and(|o| o.len() != stack.len()) {
            return Err("the samples' columns are not of one length".into());
        }
        Ok(Profile::new(strings, parent, frame, stack, weight, samples.objects.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_survives_its_document() {
        let p = Profile::from_stacks(
            vec![
                ("Java Frame;com.example.Server;byte[]".into(), 100.0, 1.0),
                ("Java Frame;com.example.Server".into(), 16.0, 1.0),
                ("System Class;com.example.Café;😀[]".into(), 24.0, 2.0),
            ],
            true,
        );
        let doc = Document::of(&p);
        let text = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&text).unwrap();
        let q = back.profile().unwrap();
        let paths = |p: &Profile| {
            let mut v: Vec<(Vec<String>, f64, f64)> = p
                .stack
                .iter()
                .enumerate()
                .map(|(i, &n)| (p.path_of(n as usize), p.weight[i], p.objects.as_ref().unwrap()[i]))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        assert_eq!(paths(&p), paths(&q));
        assert_eq!(q.heap(), 140.0);
    }
}
