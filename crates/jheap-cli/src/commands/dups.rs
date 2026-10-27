//! The Strings held in several copies: which values, how many copies, what
//! keeping one of each would save, and whose they are.

use std::collections::HashMap;
use std::io;

use jheap::Duplicates;
use jheap::model::retainers::owner_of;
use serde_json::json;

use super::{Ctx, Found, emit, header, row, share};
use crate::load::Loaded;
use crate::style;

/// The classes that hold a value's copies, the most saving first, and what
/// each holds of it.
type Holders = Vec<(String, u64)>;

/// Each named value's holders: the innermost class of the application on the
/// path of each copy, as `peaks` names who holds a peak. The String and its
/// value, the last two frames, say nothing of who holds it.
fn holders_of(d: &Duplicates) -> HashMap<u32, Holders> {
    let mut by: HashMap<u32, HashMap<String, u64>> = HashMap::new();
    for path in &d.paths {
        let [held @ .., _string, value] = path.frames.as_slice() else { continue };
        let names: Vec<&str> = held.iter().map(|&f| d.names[f as usize].as_str()).collect();
        let owner = owner_of(&names).to_string();
        *by.entry(*value).or_default().entry(owner).or_default() += path.bytes;
    }
    by.into_iter()
        .map(|(value, owners)| {
            let mut owners: Holders = owners.into_iter().collect();
            owners.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            (value, owners)
        })
        .collect()
}

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let heap = l.profile.heap();
    // always there: the command asked the parse for them
    let Some(d) = l.duplicates.as_ref() else {
        return Err(io::Error::other("the parse made no duplicates"));
    };
    let top = c.args.rows.unwrap_or(20);
    let shown = &d.values[..d.values.len().min(top)];
    let holders = holders_of(d);
    let result = if d.count == 0 { Found::Nothing } else { Found::Something };
    let unshown = d.saving - shown.iter().map(|v| v.saving).sum::<u64>();
    let more = d.count as usize - shown.len();

    if c.args.json {
        let doc = json!({
            "dump": super::dump_json(l, c.peak_memory),
            "duplicates": {
                "values": d.count,
                "copies": d.copies,
                "bytes": d.bytes,
                "saving": d.saving,
                "share": share(d.saving as f64, heap),
            },
            "top": shown.iter().map(|v| json!({
                "value": v.text,
                "length": v.length,
                "copies": v.copies,
                "arrays": v.arrays,
                "bytes": v.bytes,
                "saving": v.saving,
                "share": share(v.saving as f64, heap),
                "heldBy": holders.get(&v.frame).map(|owners| owners.iter().take(3)
                    .map(|(name, bytes)| json!({"class": name, "saving": bytes}))
                    .collect::<Vec<_>>()),
            })).collect::<Vec<_>>(),
            "more": more,
            "moreSaving": unshown,
        });
        emit(c, doc)?;
        return Ok(result);
    }

    header(c, l)?;
    writeln!(c.out)?;
    if d.count == 0 {
        writeln!(c.out, "No String is held in several copies.")?;
        return Ok(result);
    }
    writeln!(
        c.out,
        "{} {} values in {} copies take {}; keeping one copy of each would save {}, {} of the heap.",
        c.style.warn("Duplicate Strings:"),
        style::count(d.count as f64),
        style::count(d.copies as f64),
        style::bytes(d.bytes as f64),
        style::bytes(d.saving as f64),
        style::percent(share(d.saving as f64, heap))
    )?;
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold(&format!("{:>9}  {:>5}  value, to save", "saves", "share")))?;
    let indent = " ".repeat(9 + 2 + 5 + 2);
    for v in shown {
        writeln!(c.out, "{}", row(v.saving as f64, share(v.saving as f64, heap), &d.names[v.frame as usize]))?;
        let arrays = if v.arrays == v.copies {
            "each with its own array".to_string()
        } else {
            format!("in {}", style::counted(v.arrays as f64, "array"))
        };
        let mut line = format!("{} copies, {arrays}", style::count(v.copies as f64));
        if let Some(owners) = holders.get(&v.frame) {
            let total: u64 = owners.iter().map(|o| o.1).sum();
            let named: Vec<String> = owners
                .iter()
                .take(3)
                .map(|(name, bytes)| format!("{name} {}", style::percent(share(*bytes as f64, total as f64))))
                .collect();
            line += &format!("; held by {}", named.join(", "));
            if owners.len() > 3 {
                line += &format!(", and {} more", owners.len() - 3);
            }
        }
        writeln!(c.out, "{indent}{}", c.style.dim(&line))?;
    }
    if more > 0 {
        writeln!(
            c.out,
            "{}",
            c.style.dim(&format!(
                "{:>9}  {:>5}  and {} more values: {}",
                style::bytes(unshown as f64),
                style::percent(share(unshown as f64, heap)),
                style::count(more as f64),
                c.suggest(&format!("dups {}", (top * 5).min(d.count as usize)))
            ))
        )?;
    }
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.dim(
        "A copy is a String whose value equals another's. What keeping one would save is every String\n\
         but one and every array but one: where copies share an array, only the Strings are repeated.\n\
         A value is shown as the dump holds it, cut at 60 characters. Held by names the innermost\n\
         class of the application on the path of each copy."))?;
    writeln!(c.out, "{}", c.style.dim(&format!(
        "Drawn as mountains by mountain-graph, a peak a value: {}", c.suggest("dups --save dups.fpk"))))?;
    Ok(result)
}
