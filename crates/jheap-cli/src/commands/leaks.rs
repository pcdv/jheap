//! What probably leaks: the objects of the application that hold a large
//! share of the heap, and the classes whose many objects do, each with what
//! kind of memory it is, which is what tells a leak from a footprint in one
//! dump.

use std::io;

use jheap::model::leaks::{Kind, Suspect, Verdict, leaks_of};
use jheap::model::retainers::label_of;
use serde_json::{Value, json};

use super::{Ctx, Found, emit, header, num, opt, share, step_json};
use crate::load::Loaded;
use crate::style::{self, Style};

/// A suspect as JSON, its shares of `of`: the heap, or what grew.
pub fn suspect_json(x: &Suspect, rank: usize, of: f64) -> Value {
    json!({
        "rank": rank,
        "kind": x.kind.name(),
        "verdict": x.verdict.name(),
        "name": x.name,
        "total": num(x.total),
        "share": share(x.total, of),
        "objects": opt(x.objects),
        "at": num(x.at),
        "statics": x.statics,
        "gathered": x.gathered.as_ref().map(|g| json!({
            "name": g.name,
            "objects": num(g.objects),
            "there": opt(g.there),
            "total": num(g.total),
            "through": g.through,
            "containers": g.containers.as_ref().map(|k| json!({"name": k.name, "objects": num(k.objects)})),
        })),
        "includes": x.includes.iter().take(5).map(|i| json!({
            "name": i.name, "objects": num(i.objects), "all": num(i.all), "total": num(i.total),
        })).collect::<Vec<_>>(),
        "madeOf": x.made_of.iter().take(5)
            .map(|m| json!({"name": m.name, "bytes": num(m.own), "objects": opt(m.objects)}))
            .collect::<Vec<_>>(),
        "holders": x.holders.iter().map(|s| step_json(s, of)).collect::<Vec<_>>(),
    })
}

fn pad(label: &str, s: &Style) -> String {
    format!("   {}", s.dim(&format!("{label:11}")))
}

/// `, 1.3 KiB each`, when there are some.
fn each(total: f64, n: f64) -> String {
    if n > 0.0 { format!(", {} each", style::bytes(total / n)) } else { String::new() }
}

/// The lines of one suspect after its first: what it is, its verdict, what it
/// is made of, what it includes, who holds it.
fn body(c: &mut Ctx, x: &Suspect, heap: f64) -> io::Result<()> {
    let s = c.style;
    let simple = |names: Vec<&str>| names.into_iter().map(style::simple).collect::<Vec<_>>().join(" → ");
    match (x.kind, &x.gathered) {
        (Kind::One, Some(g)) => {
            let containers = g.containers.as_ref().map_or(String::new(), |k| format!(", in {} {}", style::count(k.objects), k.name));
            let through: Vec<&str> =
                std::iter::once(x.name.as_str()).chain(g.through.iter().map(String::as_str)).chain([g.name.as_str()]).collect();
            if x.verdict == Verdict::Collection {
                writeln!(c.out, "{}{} {}{}{containers}", pad("collection", &s), style::count(g.objects), g.name, each(g.total, g.objects))?;
                writeln!(c.out, "{}through {}", pad("", &s), simple(through))?;
                writeln!(c.out, "{}{}", pad("verdict", &s), s.warn("a collection grows with what it holds: a leak if a later dump has more of them"))?;
            } else {
                writeln!(c.out, "{}{} {}{}, {} in all", pad("large", &s), style::count(g.objects), g.name, each(g.total, g.objects), style::bytes(g.total))?;
                writeln!(c.out, "{}through {}", pad("", &s), simple(through))?;
                writeln!(c.out, "{}a few large objects: footprint rather than a leak, unless their number grows", pad("verdict", &s))?;
            }
        }
        (Kind::One, None) => {
            let own = x.made_of.first().map_or(x.total, |m| m.own);
            if x.at > 1.0 {
                writeln!(c.out, "{}the objects themselves, {} each", pad("large", &s), style::bytes(own / x.at))?;
                writeln!(c.out, "{}a few large objects: footprint rather than a leak, unless it keeps growing", pad("verdict", &s))?;
            } else {
                writeln!(c.out, "{}the object itself, {} of its own", pad("large", &s), style::bytes(own))?;
                writeln!(c.out, "{}one large object: footprint rather than a leak, unless it keeps growing", pad("verdict", &s))?;
            }
        }
        (Kind::Many, _) => {
            writeln!(c.out, "{}many objects of one class: what the application is made of,", pad("verdict", &s))?;
            writeln!(c.out, "{}a leak only if their number grows", pad("", &s))?;
        }
    }
    if !x.made_of.is_empty() {
        let under: f64 = x.made_of.iter().map(|m| m.own).sum();
        let parts: Vec<String> = x
            .made_of
            .iter()
            .take(3)
            .map(|m| {
                let objects = m.objects.map_or(String::new(), |n| format!(" in {}", style::counted(n, "object")));
                format!("{} {}{objects}", m.name, style::percent(if under > 0.0 { m.own / under } else { 0.0 }))
            })
            .collect();
        writeln!(c.out, "{}{}", pad("made of", &s), parts.join(", "))?;
    }
    // the classes folded in, but the one the collection line names already
    let gathered = x.gathered.as_ref().map(|g| g.name.as_str());
    let includes: Vec<String> = x
        .includes
        .iter()
        .filter(|i| Some(i.name.as_str()) != gathered || i.objects != i.all)
        .take(3)
        .map(|i| format!("{} of the {} {}", style::count(i.objects), style::count(i.all), i.name))
        .collect();
    if !includes.is_empty() {
        writeln!(c.out, "{}{}", pad("includes", &s), includes.join(", "))?;
    }
    for (j, step) in x.holders.iter().enumerate() {
        let label = if step.root { step.classes[0].clone() } else { label_of(step) };
        writeln!(c.out, "{}{}", pad(if j == 0 { "held by" } else { "" }, &s), super::row(step.total, step.total / heap.max(1.0), &label))?;
    }
    if let Some(lives) = x.holders.last().filter(|st| st.root).and_then(|st| style::lives(&st.classes[0])) {
        writeln!(c.out, "{}{}", pad("", &s), s.dim(lives))?;
    }
    Ok(())
}

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let p = &l.profile;
    let top = c.args.rows.unwrap_or(10);
    let found = leaks_of(p, c.args.share, None);
    let heap = found.heap;
    // an object holding much over many branches, none of them most, is a root
    // of the application, not a container: said in a line, not as a suspect
    let suspects: Vec<&Suspect> = found.suspects.iter().filter(|x| x.verdict != Verdict::Spread).collect();
    let spread: Vec<&Suspect> = found.suspects.iter().filter(|x| x.verdict == Verdict::Spread).collect();
    let shown = &suspects[..suspects.len().min(top)];
    let result = if suspects.is_empty() { Found::Nothing } else { Found::Something };

    if c.args.json {
        let doc = json!({
            "dump": super::dump_json(l, c.peak_memory),
            "share": c.args.share,
            "verdict": if shown.iter().any(|x| x.verdict == Verdict::Collection) { "could-grow" } else { "no-sign" },
            "suspects": shown.iter().enumerate().map(|(i, x)| suspect_json(x, i + 1, heap)).collect::<Vec<_>>(),
            "more": suspects.len() - shown.len(),
            "spread": spread.iter().map(|x| json!({"name": x.name, "total": num(x.total), "share": share(x.total, heap)}))
                .collect::<Vec<_>>(),
        });
        emit(c, doc)?;
        return Ok(result);
    }

    header(c, l)?;
    writeln!(c.out)?;
    let over = style::percent(c.args.share);
    let signs: Vec<usize> =
        shown.iter().enumerate().filter(|(_, x)| x.verdict == Verdict::Collection).map(|(i, _)| i + 1).collect();
    if suspects.is_empty() {
        writeln!(c.out, "{} no object nor class of the application holds {over} of the heap,", c.style.good("No sign of a leak:"))?;
        writeln!(c.out, "but over many branches. {} lists what is heaviest; --share 5 looks lower.", c.suggest("peaks"))?;
    } else if signs.is_empty() {
        writeln!(c.out, "{} what holds over {over} of the heap is large objects or many objects of a class:", c.style.good("No sign of a leak:"))?;
        writeln!(c.out, "footprint rather than a leak. A second dump would tell whether any of it grows.")?;
    } else {
        let which = signs.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
        let (them, grow) = if signs.len() == 1 { ("suspect", "a collection, which grows with what it holds") }
            else { ("suspects", "collections, which grow with what they hold") };
        writeln!(c.out, "{} {them} {which}, {grow}.", c.style.warn("Could grow:"))?;
        writeln!(c.out, "{}Whether it does, a full cache or a leak, a later dump tells: jheap diff <this> <later>.",
            if shown.len() > signs.len() { "The others are footprint. " } else { "" })?;
    }

    for (i, x) in shown.iter().enumerate() {
        writeln!(c.out)?;
        let first = match x.kind {
            Kind::One => {
                let what = if x.statics { format!("The static fields of {} hold", x.name) }
                    else if x.at > 1.0 { format!("{} {} hold", style::count(x.at), x.name) }
                    else { format!("One {} holds", x.name) };
                let objects = x.objects.map_or(String::new(), |n| format!(", in {}", style::counted(n, "object")));
                format!("{}. {what} {}, {} of the heap{objects}.", i + 1, style::bytes(x.total), style::percent(x.share))
            }
            Kind::Many => format!(
                "{}. {} {} hold {}, {} of the heap{}, from no one place.",
                i + 1, style::count(x.objects.unwrap_or(0.0)), x.name, style::bytes(x.total),
                style::percent(x.share), each(x.total, x.objects.unwrap_or(0.0))
            ),
        };
        writeln!(c.out, "{}", c.style.bold(&first))?;
        body(c, x, heap)?;
    }
    if suspects.len() > shown.len() {
        writeln!(c.out, "\nand {} more: {} lists them all", suspects.len() - shown.len(), c.suggest(&format!("leaks {}", suspects.len())))?;
    }
    if !spread.is_empty() {
        writeln!(c.out, "\nAlso over {over}, but spread over many branches, none of them most: roots of the")?;
        writeln!(c.out, "application rather than containers. trace on one shows its branches.")?;
        for x in &spread {
            writeln!(c.out, "{}", super::row(x.total, x.share, &x.name))?;
        }
    }
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.dim("What an object holds is what the walk reached through it first: something else may hold it too."))?;
    Ok(result)
}
