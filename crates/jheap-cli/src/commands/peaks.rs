//! The peaks, as heap-landscape draws them, and under each who retains it:
//! what it is made of, then its holders outward, a line where what they hold
//! grows. A line that other peaks share is where they meet, the likelier
//! culprit.

use std::io;

use jheap::model::ownership::{THIN, ownership_peaks};
use jheap::model::retainers::{label_of, made_of, steps_of};
use serde_json::json;

use super::{Ctx, Found, emit, header, num, opt, row, share, step_json};
use crate::load::Loaded;
use crate::style;

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let p = &l.profile;
    let heap = p.heap();
    let found = ownership_peaks(p, c.args.rows.unwrap_or(10), 1.0, THIN).peaks;
    // the peaks held through each node, by their rank: where peaks meet
    let mut through: Vec<Vec<usize>> = vec![Vec::new(); p.len()];
    for (i, peak) in found.iter().enumerate() {
        let mut n = p.parent_of(peak.node);
        while let Some(h) = n {
            through[h].push(i + 1);
            n = p.parent_of(h);
        }
    }
    // every peak meets every other at its root: not worth saying
    let also = |rank: usize, step: &jheap::model::retainers::Step| -> Vec<usize> {
        if step.root { Vec::new() } else { through[step.node].iter().copied().filter(|&k| k != rank).collect() }
    };
    let result = if found.is_empty() { Found::Nothing } else { Found::Something };

    if c.args.json {
        let doc = json!({
            "dump": super::dump_json(l, c.peak_memory),
            "peaks": found.iter().enumerate().map(|(i, peak)| json!({
                "rank": i + 1,
                "name": peak.name,
                "total": num(peak.total),
                "own": num(peak.own),
                "share": share(peak.total, heap),
                "madeOf": made_of(p, peak.node).iter().take(5)
                    .map(|m| json!({"name": m.name, "bytes": num(m.own), "objects": opt(m.objects)}))
                    .collect::<Vec<_>>(),
                "holders": steps_of(p, peak.node, 0.1).iter().map(|step| {
                    let mut v = step_json(step, heap);
                    v["alsoPeaks"] = json!(also(i + 1, step));
                    v
                }).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        emit(c, doc)?;
        return Ok(result);
    }

    header(c, l)?;
    if found.is_empty() {
        writeln!(c.out, "\nNo peak: the heap is empty.")?;
        return Ok(result);
    }
    let indent = " ".repeat(found.len().to_string().len() + 2);
    for (i, peak) in found.iter().enumerate() {
        writeln!(c.out)?;
        let rank = format!("{}.", i + 1);
        writeln!(c.out, "{rank:w$}{}", c.style.bold(&row(peak.total, peak.share, &peak.name)), w = indent.len())?;

        let made = made_of(p, peak.node);
        let under: f64 = made.iter().map(|m| m.own).sum();
        let mut parts: Vec<String> = made
            .iter()
            .take(3)
            .map(|m| {
                let objects = m.objects.map_or(String::new(), |n| format!(" in {}", style::counted(n, "object")));
                format!("{} {}{objects}", m.name, style::percent(if under > 0.0 { m.own / under } else { 0.0 }))
            })
            .collect();
        if made.len() > 3 {
            parts.push(format!("and {} more classes", made.len() - 3));
        }
        writeln!(c.out, "{indent}{}{}", c.style.dim(&format!("{:10}", "made of")), parts.join(", "))?;

        for (j, step) in steps_of(p, peak.node, 0.1).iter().enumerate() {
            let label = if step.root {
                let root = &step.classes[0];
                format!("{root}, {}", style::held_from(root))
            } else {
                label_of(step)
            };
            let others = also(i + 1, step);
            let shared = match others.len() {
                0 => String::new(),
                1 => format!("  (also peak {})", others[0]),
                _ => format!("  (also peaks {})", others.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(", ")),
            };
            let what = if j == 0 { "held by" } else { "" };
            writeln!(
                c.out,
                "{indent}{}{}{}",
                c.style.dim(&format!("{what:10}")),
                row(step.total, share(step.total, heap), &label),
                c.style.warn(&shared)
            )?;
        }
    }
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.dim(&format!(
        "Each holder line is where what is held grows by over 10%; a line other peaks share is\n\
         where they meet, the likelier culprit. More on one class: {}", c.suggest("trace <class>"))))?;
    Ok(result)
}
