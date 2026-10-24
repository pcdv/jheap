//! The commands, each as text for a person and as JSON for a script.

pub mod classes;
pub mod diff;
pub mod info;
pub mod leaks;
pub mod peaks;
pub mod summary;
pub mod trace;

use std::io::{self, Write};

use jheap::model::retainers::{Step, is_application, label_of, steps_of};
use jheap::Profile;
use serde_json::{Value, json};

use crate::args::Args;
use crate::load::{Loaded, how_read};
use crate::style::{self, Style};

/// The version of the JSON documents: raised when a field changes meaning or
/// goes, not when one is added.
pub const JSON_VERSION: u32 = 1;

/// What a command found, as its exit code says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Something = 0,
    Nothing = 3,
    Several = 4,
}

pub struct Ctx<'a> {
    pub args: &'a Args,
    pub style: Style,
    pub out: &'a mut dyn Write,
    /// How to name the dump in a command line suggested: None when it was
    /// found, so the suggestion finds it again.
    pub named: Vec<Option<String>>,
    /// The peak memory of the process, for --stats, once the parse is done.
    pub peak_memory: Option<u64>,
}

impl Ctx<'_> {
    /// A suggested command line, the dump named as it was.
    pub fn suggest(&self, command: &str) -> String {
        match self.named.first().cloned().flatten() {
            Some(dump) => format!("jheap {} {command}", quote(&dump)),
            None => format!("jheap {command}"),
        }
    }
}

/// A path as a shell takes it.
pub fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:\\".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Bytes, their share and a text, aligned as every table here is.
pub fn row(bytes: f64, share: f64, text: &str) -> String {
    format!("{:>9}  {:>5}  {text}", style::bytes(bytes), style::percent(share))
}

/// The objects a dump holds: its records, or the profile's count.
pub fn objects_of(l: &Loaded) -> Option<f64> {
    match &l.report {
        Some(r) => Some((r.instance_count + r.object_array_count + r.primitive_array_count) as f64),
        None => l.profile.sums().held.as_ref().map(|h| l.profile.sums().roots.iter().map(|&r| h[r]).sum()),
    }
}

/// The two lines every text output starts with: the heap, and how it was read.
pub fn header(c: &mut Ctx, l: &Loaded) -> io::Result<()> {
    let heap = l.profile.heap();
    let mut first = format!("{}: {}", l.path.display(), style::bytes(heap));
    if let Some(n) = objects_of(l) {
        first += &format!(" in {}", style::counted(n, "object"));
    }
    if let Some(r) = &l.report
        && r.unreachable_size > 0
    {
        first += &format!(", {} unreachable", style::percent(r.unreachable_size as f64 / heap.max(1.0)));
    }
    writeln!(c.out, "{}", c.style.bold(&first))?;
    writeln!(c.out, "{}", c.style.dim(&how_read(l)))?;
    if let Some(peak) = c.peak_memory {
        let per = objects_of(l).map_or(String::new(), |n| format!(", {:.0} bytes an object", peak as f64 / n.max(1.0)));
        writeln!(c.out, "{}", c.style.dim(&format!("peak memory {}{per}", style::bytes(peak as f64))))?;
    }
    Ok(())
}

/// What the JSON says of a dump: what the text's header does.
pub fn dump_json(l: &Loaded, peak_memory: Option<u64>) -> Value {
    let mut d = json!({
        "file": l.path.display().to_string(),
        "profile": l.is_profile,
        "readFrom": l.read_from.as_ref().map(|p| p.display().to_string()),
        "keptAt": l.kept_at.as_ref().map(|p| p.display().to_string()),
        "bytes": num(l.profile.heap()),
        "objects": objects_of(l).map(num),
        "counted": l.profile.objects.is_some(),
    });
    if let Some(r) = &l.report {
        let parse = json!({
            "format": r.version,
            "idSize": r.id_size,
            "fileSize": r.file_size,
            "pointers": r.pointers.name(),
            "pointersGuessed": r.pointers_guessed,
            "unreachable": r.unreachable_size,
            "classes": r.class_count,
            "parseMs": num(r.parse_time_ms.round()),
            "unzipMs": num(l.unzip_ms.round()),
        });
        d.as_object_mut().unwrap().extend(parse.as_object().unwrap().clone());
    }
    if let Some(peak) = peak_memory {
        d["peakMemory"] = json!(peak);
    }
    d
}

/// Prints a JSON document, with its version and command.
pub fn emit(c: &mut Ctx, mut doc: Value) -> io::Result<()> {
    let mut head = json!({"jheap": JSON_VERSION, "command": c.args.command.name()});
    head.as_object_mut().unwrap().append(doc.as_object_mut().unwrap());
    serde_json::to_writer_pretty(&mut *c.out, &head).map_err(io::Error::other)?;
    writeln!(c.out)
}

/// A number as JSON: an integer when it is one, as bytes and counts are.
pub fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9e15 { json!(v as i64) } else { json!(v) }
}

pub fn opt(v: Option<f64>) -> Value {
    v.map_or(Value::Null, num)
}

/// A share, 0 to 1, rounded to what a reader needs: a hundredth of a percent.
pub fn share(v: f64, of: f64) -> f64 {
    if of > 0.0 { (v / of * 1e4).round() / 1e4 } else { 0.0 }
}

/// A step of holders as JSON.
pub fn step_json(step: &Step, of: f64) -> Value {
    let mut v = json!({
        "total": num(step.total),
        "share": share(step.total, of),
        "classes": step.classes,
        "holders": step.holders,
        "root": step.root,
    });
    if step.root {
        v["lives"] = json!(style::lives(&step.classes[0]));
    }
    v
}

/// Who holds a node, in a few words: the nearest step with a class of the
/// application, else the nearest, then the root it is held from.
pub fn held_by(p: &Profile, node: usize) -> String {
    let steps = steps_of(p, node, 0.1);
    let root = steps.iter().find(|s| s.root).map(|s| s.classes[0].as_str());
    if root == Some("(unreachable)") || (steps.is_empty() && p.name(node) == "(unreachable)") {
        return style::held_from("(unreachable)").to_string();
    }
    let holders: Vec<&Step> = steps.iter().filter(|s| !s.root).collect();
    let step = holders.iter().find(|s| s.classes.iter().any(|c| is_application(c))).or(holders.first());
    let from = root.map_or("", style::held_from);
    match step {
        Some(s) => format!("held by {}, {from}", label_of(s)),
        None => format!("held {from}"),
    }
}
