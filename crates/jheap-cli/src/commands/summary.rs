//! The default: what holds the memory, whether anything could leak, and what
//! to ask next. One screen, the answer first.

use std::io;

use jheap::model::leaks::{Kind, Suspect, Verdict, leaks_of};
use jheap::model::ownership::{THIN, ownership_peaks};
use jheap::model::retainers::{is_application, steps_of};
use serde_json::json;

use super::{Ctx, Found, emit, header, held_by, num, row, share};
use crate::load::Loaded;
use crate::style;

/// A suspect in a sentence: who holds how much, in what.
pub fn sentence(x: &Suspect, of_what: &str) -> String {
    let who = if x.kind == Kind::Many {
        format!("{} {}", style::count(x.objects.unwrap_or(0.0)), x.name)
    } else if x.statics {
        format!("the static fields of {}", x.name)
    } else if x.at > 1.0 {
        format!("{} {}", style::count(x.at), x.name)
    } else {
        format!("one {}", x.name)
    };
    let verb = if x.kind == Kind::Many || (x.at > 1.0 && !x.statics) { "hold" } else { "holds" };
    let what = match (&x.gathered, x.verdict) {
        (Some(g), Verdict::Collection) => format!(" in a collection of {} {}", style::count(g.objects), g.name),
        (Some(g), _) => format!(", {} large {}", style::count(g.objects), g.name),
        (None, Verdict::Instances) => ", from no one place".into(),
        (None, _) => String::new(),
    };
    format!("{who} {verb} {} of {of_what}{what}", style::percent(x.share))
}

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let p = &l.profile;
    let heap = p.heap();
    let peaks = ownership_peaks(p, c.args.rows.unwrap_or(5), 1.0, THIN).peaks;
    let found = leaks_of(p, c.args.share, None);
    let suspects: Vec<&Suspect> = found.suspects.iter().filter(|x| x.verdict != Verdict::Spread).collect();
    let collection = suspects.iter().find(|x| x.verdict == Verdict::Collection);

    // the class to trace next: the likeliest culprit, else whose the biggest peak is
    let trace = collection
        .or(suspects.first())
        .map(|x| x.name.clone())
        .or_else(|| {
            peaks.first().and_then(|peak| {
                steps_of(p, peak.node, 0.1)
                    .into_iter()
                    .flat_map(|s| s.classes)
                    .find(|n| is_application(n))
            })
        });

    if c.args.json {
        let verdict = if collection.is_some() { "could-grow" } else { "no-sign" };
        let doc = json!({
            "dump": super::dump_json(l, c.peak_memory),
            "peaks": peaks.iter().map(|peak| json!({
                "name": peak.name,
                "total": num(peak.total),
                "share": share(peak.total, heap),
                "heldBy": held_by(p, peak.node),
            })).collect::<Vec<_>>(),
            "leaks": {
                "share": c.args.share,
                "verdict": verdict,
                "suspects": suspects.iter().take(3).map(|x| json!({
                    "kind": x.kind.name(),
                    "verdict": x.verdict.name(),
                    "name": x.name,
                    "total": num(x.total),
                    "share": share(x.total, heap),
                    "says": sentence(x, "the heap"),
                })).collect::<Vec<_>>(),
                "more": suspects.len().saturating_sub(3),
            },
        });
        emit(c, doc)?;
        return Ok(Found::Something);
    }

    header(c, l)?;
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold("Where the memory is"))?;
    let width = peaks.len().to_string().len() + 2;
    for (i, peak) in peaks.iter().enumerate() {
        writeln!(c.out, "{:width$}{}", format!("{}.", i + 1), row(peak.total, peak.share, &peak.name))?;
        writeln!(c.out, "{:width$}{:18}{}", "", "", c.style.dim(&held_by(p, peak.node)))?;
    }

    writeln!(c.out)?;
    match (collection, suspects.first()) {
        (Some(x), _) => {
            writeln!(c.out, "{} {}.", c.style.warn("Could grow:"), sentence(x, "the heap"))?;
            if suspects.len() > 1 {
                let more = suspects.len() - 1;
                writeln!(c.out, "{}", c.style.dim(&format!(
                    "and {more} more suspect{}: {}", if more > 1 { "s" } else { "" }, c.suggest("leaks"))))?;
            }
        }
        (None, Some(x)) => writeln!(
            c.out,
            "{} the heaviest is footprint rather than a collection: {}.",
            c.style.good("No sign of a leak:"),
            sentence(x, "the heap")
        )?,
        (None, None) => writeln!(
            c.out,
            "{} no object nor class of the application holds {} of the heap.",
            c.style.good("No sign of a leak:"),
            style::percent(c.args.share)
        )?,
    }

    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold("Next"))?;
    let mut next = vec![
        (c.suggest("peaks"), "each peak, what it is made of and who holds it".to_string()),
        (c.suggest("leaks"), "the leak suspects, each with what it is made of".to_string()),
        (c.suggest("classes"), "the heaviest classes".to_string()),
    ];
    if let Some(class) = trace {
        next.push((c.suggest(&format!("trace {}", super::quote(&class))), format!("what holds a {} and what it holds", style::simple(&class))));
    }
    let dump = c.named.first().cloned().flatten().map_or("<later>".into(), |d| super::quote(&d));
    next.push((format!("jheap diff <earlier> {dump}"), "what grew since an earlier dump".to_string()));
    let width = next.iter().map(|(cmd, _)| cmd.chars().count()).max().unwrap_or(0);
    let longest = next.iter().map(|(_, what)| what.len()).max().unwrap_or(0);
    for (cmd, what) in next {
        // side by side while it fits a terminal; a long path puts each
        // description under its command
        if 2 + width + 2 + longest <= 120 {
            writeln!(c.out, "  {cmd:width$}  {}", c.style.dim(&what))?;
        } else {
            writeln!(c.out, "  {cmd}\n      {}", c.style.dim(&what))?;
        }
    }
    Ok(Found::Something)
}
