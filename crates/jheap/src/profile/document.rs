//! A profile as a Flamepack document (specified in
//! https://github.com/pcdv/flamepack), which jfr-web's and mountain-graph's
//! pages read: strings sorted and front-coded, stack nodes depth first, a
//! sample per path. Written as version 5, which leaves out what a heap has
//! not (time, threads, operations, the weights a column of bytes replaces);
//! version 4, which jheap wrote until then, is read as well.
//!
//! A heap profile's samples are written sorted by stack, each stack as the
//! difference with the last, which costs almost nothing once compressed. The
//! column `samples.objects`, the objects of each sample, is a heap's own.

use serde::{Deserialize, Serialize};

use super::Profile;

/// The version of the format that is written.
pub const VERSION: u32 = 5;

/// The version before it, which is read.
const PREVIOUS: u32 = 4;

#[derive(Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub unit: String,
    /// Version 4 always had them: the weight of a sample is `weights[kind]`
    /// when it has no `weight`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    pub strings: Vec<String>,
    pub stacks: Stacks,
    pub samples: Samples,
}

/// What the document says about the profile: the kind of it, which names files.
#[derive(Serialize, Deserialize)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The nodes of the stack tree, depth first.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Stacks {
    /// Version 5: a column each. The type of a frame, in its low 3 bits, is
    /// left out when all are 0.
    Columns {
        depth: Vec<i64>,
        name: Vec<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#type: Option<Vec<u8>>,
    },
    /// Version 4: the depth and the frame of each node, one after the other.
    Pairs(Vec<i64>),
}

#[derive(Serialize, Deserialize)]
pub struct Samples {
    /// Version 5: the node of each sample plus 1, 0 for none, as the
    /// difference with the last. Version 4: the node, -1 for none.
    pub stack: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objects: Option<Vec<f64>>,
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
        let mut stack_depth = Vec::with_capacity(n);
        let mut stack_name = Vec::with_capacity(n);
        let mut stack_type = Vec::with_capacity(n);
        for &node in &order {
            let frame = p.frame[node];
            stack_depth.push(depth[node]);
            stack_name.push(rank[p.strings[(frame >> 3) as usize].as_str()] as i64);
            stack_type.push((frame & 7) as u8);
        }

        // by stack, the samples with none first: the node plus 1, as a difference
        let at = |i: usize| if p.stack[i] < 0 { 0 } else { position[p.stack[i] as usize] + 1 };
        let mut sample: Vec<usize> = (0..p.stack.len()).collect();
        sample.sort_by_key(|&i| at(i));
        let mut last = 0;
        let differences = sample
            .iter()
            .map(|&i| {
                let next = at(i);
                let d = next - last;
                last = next;
                d
            })
            .collect();

        Document {
            version: VERSION,
            unit: "bytes".into(),
            weights: None,
            meta: Some(Meta { kind: Some("heap".into()) }),
            strings,
            stacks: Stacks::Columns {
                depth: stack_depth,
                name: stack_name,
                r#type: stack_type.iter().any(|&t| t != 0).then_some(stack_type),
            },
            samples: Samples {
                stack: differences,
                kind: None,
                weight: Some(sample.iter().map(|&i| p.weight[i]).collect()),
                objects: p.objects.as_ref().map(|o| sample.iter().map(|&i| o[i]).collect()),
            },
        }
    }

    /// The profile of a document, of version 5 or 4.
    pub fn profile(&self) -> Result<Profile, String> {
        if self.version != VERSION && self.version != PREVIOUS {
            return Err(format!("a profile of version {}, where versions {PREVIOUS} and {VERSION} are read", self.version));
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
        let (depths, frames): (Vec<i64>, Vec<i64>) = match (&self.stacks, self.version) {
            (Stacks::Columns { depth, name, r#type }, VERSION) => {
                if depth.len() != name.len() || r#type.as_ref().is_some_and(|t| t.len() != depth.len()) {
                    return Err("the stacks' columns are not of one length".into());
                }
                let frames = name.iter().enumerate().map(|(i, &n)| n << 3 | r#type.as_ref().map_or(0, |t| t[i] as i64));
                (depth.clone(), frames.collect())
            }
            (Stacks::Pairs(pairs), PREVIOUS) => {
                (pairs.chunks_exact(2).map(|c| c[0]).collect(), pairs.chunks_exact(2).map(|c| c[1]).collect())
            }
            _ => return Err(format!("the stacks are not written as version {} writes them", self.version)),
        };
        let n = depths.len();
        let (mut parent, mut frame) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut last: Vec<i32> = Vec::new();
        for i in 0..n {
            let depth = depths[i] as usize;
            if depths[i] < 0 || depth > last.len() {
                return Err(format!("node {i} is {} deep, under no node {} deep", depths[i], depths[i] - 1));
            }
            let f = frames[i];
            if f < 0 || (f >> 3) as usize >= strings.len() {
                return Err(format!("node {i} names no string of the profile"));
            }
            parent.push(if depth == 0 { -1 } else { last[depth - 1] });
            frame.push(f as u32);
            last.truncate(depth);
            last.push(i as i32);
        }

        let samples = &self.samples;
        let mut sum = 0;
        let stack: Vec<i32> = samples
            .stack
            .iter()
            .map(|&s| {
                let node = if self.version == VERSION {
                    sum += s;
                    sum - 1
                } else {
                    s
                };
                if node < 0 || node as usize >= n { -1 } else { node as i32 }
            })
            .collect();
        let weight = match &samples.weight {
            Some(w) => w.clone(),
            None => {
                let weights = self.weights.clone().unwrap_or_else(|| vec![1.0]);
                let weight = |k: u8| weights.get(k as usize).copied().unwrap_or(0.0);
                match &samples.kind {
                    Some(kind) => kind.iter().map(|&k| weight(k)).collect(),
                    None => vec![weight(0); stack.len()],
                }
            }
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

    /// The type of a frame that stands for no class.
    const SYNTHETIC: u32 = 4;

    fn profile() -> Profile {
        Profile::from_stacks(
            vec![
                ("Java Frame;com.example.Server;byte[]".into(), 100.0, 1.0),
                ("Java Frame;com.example.Server".into(), 16.0, 1.0),
                ("System Class;com.example.Café;😀[]".into(), 24.0, 2.0),
            ],
            true,
        )
    }

    fn paths(p: &Profile) -> Vec<(Vec<String>, f64, f64)> {
        let mut v: Vec<(Vec<String>, f64, f64)> = p
            .stack
            .iter()
            .enumerate()
            .map(|(i, &n)| (p.path_of(n as usize), p.weight[i], p.objects.as_ref().unwrap()[i]))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn a_profile_survives_its_document() {
        let p = profile();
        let text = serde_json::to_string(&Document::of(&p)).unwrap();
        let q = serde_json::from_str::<Document>(&text).unwrap().profile().unwrap();

        assert_eq!(paths(&p), paths(&q));
        assert_eq!(q.heap(), 140.0);
    }

    #[test]
    fn version_5_leaves_out_what_a_heap_has_not() {
        let value = serde_json::to_value(Document::of(&profile())).unwrap();
        let members: Vec<&String> = value.as_object().unwrap().keys().collect();

        assert_eq!(value["version"], 5);
        assert_eq!(value["unit"], "bytes");
        assert_eq!(members.len(), 6, "{members:?}");
        assert_eq!(value["meta"]["kind"], "heap");
        assert_eq!(value["stacks"].as_object().unwrap().keys().collect::<Vec<_>>(), ["depth", "name"]);
        let samples = value["samples"].as_object().unwrap();
        let mut keys: Vec<&String> = samples.keys().collect();
        keys.sort();
        assert_eq!(keys, ["objects", "stack", "weight"]);
    }

    #[test]
    fn the_samples_are_sorted_by_stack_and_written_as_differences() {
        let value = serde_json::to_value(Document::of(&profile())).unwrap();
        let stack: Vec<i64> = value["samples"]["stack"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();

        assert_eq!(stack.len(), 3);
        assert!(stack.iter().all(|&d| d >= 0), "{stack:?}");
    }

    #[test]
    fn a_frame_that_is_not_compiled_has_its_type_written() {
        let mut p = profile();
        p.frame[0] |= SYNTHETIC;
        let value = serde_json::to_value(Document::of(&p)).unwrap();

        assert!(value["stacks"]["type"].as_array().unwrap().iter().any(|t| t == SYNTHETIC));
    }

    #[test]
    fn version_4_is_read_as_well() {
        let text = r#"{"version": 4, "unit": "bytes", "weights": [1, 1], "start": "", "end": 0,
            "strings": [" Java Frame", " Server", " byte[]"],
            "stacks": [0, 0, 1, 8, 2, 16],
            "samples": {"time": [0, 0], "stack": [1, 2], "thread": [0, 0], "operation": [-1, -1],
                        "kind": [0, 0], "weight": [16, 100]},
            "threads": [[0, 0]], "operations": {"name": []}}"#;
        let p = serde_json::from_str::<Document>(text).unwrap().profile().unwrap();

        assert_eq!(p.stack.len(), 2);
        assert_eq!(p.path_of(p.stack[1] as usize), ["Java Frame", "Server", "byte[]"]);
        assert_eq!(p.weight, [16.0, 100.0]);
    }

    #[test]
    fn another_version_is_refused() {
        let text = r#"{"version": 6, "unit": "bytes", "strings": [], "stacks": {"depth": [], "name": []},
            "samples": {"stack": []}}"#;
        let error = serde_json::from_str::<Document>(text).unwrap().profile().err().unwrap();

        assert!(error.contains("version 6"), "{error}");
    }

    #[test]
    fn a_column_of_another_length_is_refused() {
        let text = r#"{"version": 5, "unit": "bytes", "strings": [" a"], "stacks": {"depth": [0], "name": [0]},
            "samples": {"stack": [1, 0], "weight": [3]}}"#;
        let error = serde_json::from_str::<Document>(text).unwrap().profile().err().unwrap();

        assert!(error.contains("one length"), "{error}");
    }
}
