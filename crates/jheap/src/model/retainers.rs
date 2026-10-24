//! Who retains a peak: the classes of the application among its holders, and
//! where along the chain of holders the memory gathers. A peak says what the
//! memory is; its holders say whose it is, and the class to look at is rarely
//! the one next to it, a map or an array, but the one of the application
//! further out, where what is held stops growing.

use crate::hash::FxHashMap;
use crate::hprof::ROOT_NAMES;
use crate::profile::Profile;

/// The frame names that are no class: where the walk started, and what it
/// never reached.
fn is_root_or_unreachable(name: &str) -> bool {
    name == "(unreachable)" || ROOT_NAMES.contains(&name)
}

/// Whether a frame is a class, not a root or the unreachable.
pub fn is_class(name: &str) -> bool {
    !is_root_or_unreachable(name)
}

/// The packages of the JDK: its classes hold the application's memory, they
/// are not whose it is.
pub fn is_jdk(name: &str) -> bool {
    ["java.", "javax.", "jdk.", "sun.", "com.sun."].iter().any(|p| name.starts_with(p))
}

/// Whether a frame is a class of the application or of a library it uses:
/// not the JDK's, not an array, not a root.
pub fn is_application(name: &str) -> bool {
    is_class(name) && !is_jdk(name) && !name.ends_with("[]")
}

/// The class a path is held through, for a table by owner: the innermost
/// class of the application on it, else the innermost class that is not an
/// array, as a zip file holds its buffers. A path of a root and arrays only is
/// named by its root, which says more than the array: a thread's frame, or
/// nothing at all.
pub fn owner_of<S: AsRef<str>>(path: &[S]) -> &str {
    if let Some(c) = path.iter().rev().find(|n| is_application(n.as_ref())) {
        return c.as_ref();
    }
    if let Some(c) = path.iter().rev().find(|n| is_class(n.as_ref()) && !n.as_ref().ends_with("[]")) {
        return c.as_ref();
    }
    path.first().map_or("", |n| n.as_ref())
}

/// A step of the holders of a node: holders that hold about the same.
#[derive(Debug, Clone)]
pub struct Step {
    /// The outermost holder of the step: the one that holds `total`.
    pub node: usize,
    /// Bytes the outermost holder holds, the peak included.
    pub total: f64,
    /// The classes of the application in the step, the nearest first, each
    /// once; when there is none, the step's first and last holders.
    pub classes: Vec<String>,
    /// How many holders the step spans.
    pub holders: usize,
    /// The step is the root, where the walk started.
    pub root: bool,
}

/// The holders of a node, outward, in steps: a step ends where what the
/// holders hold grows by more than `grow`. Within a step the holders hold
/// about the same, so they are one owner seen at several levels, a cache and
/// the maps inside it; where a step ends, more is gathered than this peak,
/// which is where several peaks meet. The root is a step of its own, the last.
pub fn steps_of(p: &Profile, node: usize, grow: f64) -> Vec<Step> {
    let s = p.sums();
    let mut steps = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let close = |run: &mut Vec<usize>, steps: &mut Vec<Step>| {
        if run.is_empty() {
            return;
        }
        let mut classes: Vec<String> = Vec::new();
        for &n in run.iter() {
            let c = p.name(n);
            if is_application(c) && classes.last().is_none_or(|last| last != c) {
                classes.push(c.to_string());
            }
        }
        if classes.is_empty() {
            classes.push(p.name(run[0]).to_string());
            let last = p.name(run[run.len() - 1]);
            if run.len() > 1 && last != classes[0] {
                classes.push(last.to_string());
            }
        }
        let last = run[run.len() - 1];
        steps.push(Step { node: last, total: s.total[last], classes, holders: run.len(), root: false });
        run.clear();
    };
    let mut n = p.parent_of(node);
    while let Some(i) = n {
        if p.parent[i] < 0 {
            close(&mut run, &mut steps);
            steps.push(Step { node: i, total: s.total[i], classes: vec![p.name(i).to_string()], holders: 1, root: true });
            break;
        }
        if !run.is_empty() && s.total[i] > (1.0 + grow) * s.total[run[0]] {
            close(&mut run, &mut steps);
        }
        run.push(i);
        n = p.parent_of(i);
    }
    steps
}

/// A step as a line names it: its classes, or the nearest and the outermost
/// when there are more than three, with how many are between.
pub fn label_of(step: &Step) -> String {
    let c = &step.classes;
    if c.len() > 3 {
        format!("{} ← ({} more) ← {}", c[0], c.len() - 2, c[c.len() - 1])
    } else {
        c.join(" ← ")
    }
}

/// The bytes and objects of one class under a node.
#[derive(Debug, Clone)]
pub struct Made {
    pub name: String,
    pub own: f64,
    /// None when the profile does not count objects.
    pub objects: Option<f64>,
}

/// What a node holds, by class: the bytes of the objects of each class under
/// it, itself included, and how many they are, the heaviest first.
pub fn made_of(p: &Profile, node: usize) -> Vec<Made> {
    let s = p.sums();
    let mut by: Vec<Made> = Vec::new();
    let mut index: FxHashMap<&str, usize> = FxHashMap::default();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if s.own[n] > 0.0 {
            let name = p.name(n);
            let at = *index.entry(name).or_insert_with(|| {
                by.push(Made { name: name.to_string(), own: 0.0, objects: s.objects.as_ref().map(|_| 0.0) });
                by.len() - 1
            });
            by[at].own += s.own[n];
            if let (Some(objects), Some(c)) = (s.objects.as_ref(), by[at].objects.as_mut()) {
                *c += objects[n];
            }
        }
        stack.extend_from_slice(&s.children[n]);
    }
    by.sort_by(|a, b| b.own.total_cmp(&a.own));
    by
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_and_applications() {
        assert!(is_application("com.example.Cache"));
        assert!(!is_application("java.util.HashMap"));
        assert!(!is_application("com.example.Cache[]"));
        assert!(!is_application("Java Frame"));
        assert!(!is_class("(unreachable)"));
        assert_eq!(owner_of(&["Java Frame", "com.example.Server", "java.util.HashMap", "byte[]"]), "com.example.Server");
        assert_eq!(owner_of(&["System Class", "java.util.zip.ZipFile$Source", "byte[]"]), "java.util.zip.ZipFile$Source");
        assert_eq!(owner_of(&["Java Frame", "byte[]"]), "Java Frame");
    }
}
