//! What grew between two dumps of the same process, and where: the leak
//! suspects of the growth, a collection that gained many objects the surest
//! sign; then the classes that gained the most objects.

use std::collections::HashMap;
use std::io;

use jheap::model::diff::{Amount, diff_of, nodes_by_path};
use jheap::model::leaks::{Kind, Suspect, Verdict, leaks_of};
use jheap::model::retainers::label_of;
use jheap::Profile;
use serde_json::{Value, json};

use super::leaks::suspect_json;
use super::{Ctx, Found, emit, num, opt, share};
use crate::load::{Loaded, how_read};
use crate::style;

fn signed(n: f64, f: impl Fn(f64) -> String) -> String {
    format!("{}{}", if n < 0.0 { "-" } else { "+" }, f(n.abs()))
}

fn amount(a: &Amount) -> String {
    match a.objects {
        Some(n) => format!("{} in {}", style::bytes(a.bytes), style::counted(n, "object")),
        None => style::bytes(a.bytes),
    }
}

pub fn run(c: &mut Ctx, first: &Loaded, second: &Loaded) -> io::Result<Found> {
    let top = c.args.rows.unwrap_or(10);
    let d = diff_of(&first.profile, &second.profile);
    let found = if d.grew > 0.0 { leaks_of(&d.growth, c.args.share, Some(&second.profile)).suspects } else { Vec::new() };
    let shown: Vec<&Suspect> = found.iter().filter(|x| x.verdict != Verdict::Spread).take(top).collect();
    let spread: Vec<&Suspect> = found.iter().filter(|x| x.verdict == Verdict::Spread).collect();
    let counted = d.before.objects.is_some() && d.after.objects.is_some();
    let gained: Vec<_> = d
        .classes
        .iter()
        .filter(|k| k.after.bytes > k.before.bytes || k.after.objects.unwrap_or(0.0) > k.before.objects.unwrap_or(0.0))
        .take(top)
        .collect();

    // the place of a node of the growth in each dump, to say how big it was and is
    let places = [nodes_by_path(&first.profile), nodes_by_path(&second.profile)];
    let profiles: [&Profile; 2] = [&first.profile, &second.profile];
    let growth_places: HashMap<usize, String> = nodes_by_path(&d.growth).into_iter().map(|(k, n)| (n, k)).collect();
    let size_at = |node: usize, i: usize| -> Option<Amount> {
        let n = *places[i].get(growth_places.get(&node)?)?;
        let s = profiles[i].sums();
        Some(Amount { bytes: s.total[n], objects: s.held.as_ref().map(|h| h[n]) })
    };
    let result = if d.grew > 0.0 { Found::Something } else { Found::Nothing };

    if c.args.json {
        let at = |a: Option<Amount>| a.map_or(Value::Null, |a| json!({"bytes": num(a.bytes), "objects": opt(a.objects)}));
        let verdict = if d.grew == 0.0 { "nothing-grew" }
            else if shown.iter().any(|x| x.verdict == Verdict::Collection) { "leak-sign" } else { "no-sign" };
        let side = |l: &Loaded, a: &Amount| {
            let mut v = super::dump_json(l, None);
            v["reachable"] = json!({"bytes": num(a.bytes), "objects": opt(a.objects)});
            v
        };
        emit(c, json!({
            "before": side(first, &d.before),
            "after": side(second, &d.after),
            "grew": num(d.grew),
            "shrank": num(d.shrank),
            "share": c.args.share,
            "verdict": verdict,
            "suspects": shown.iter().enumerate().map(|(i, x)| {
                let mut v = suspect_json(x, i + 1, d.grew);
                v["before"] = at(size_at(x.node, 0));
                v["after"] = at(size_at(x.node, 1));
                v
            }).collect::<Vec<_>>(),
            "spread": spread.iter().map(|x| json!({"name": x.name, "total": num(x.total), "share": share(x.total, d.grew)}))
                .collect::<Vec<_>>(),
            "classes": gained.iter().map(|k| json!({
                "name": k.name,
                "before": {"bytes": num(k.before.bytes), "objects": opt(k.before.objects)},
                "after": {"bytes": num(k.after.bytes), "objects": opt(k.after.objects)},
            })).collect::<Vec<_>>(),
        }))?;
        return Ok(result);
    }

    let s = c.style;
    let was = |node: usize| -> String {
        let say = |a: Option<Amount>| a.map_or("none".to_string(), |a| amount(&a));
        format!("{} before, {} after", say(size_at(node, 0)), say(size_at(node, 1)))
    };
    let of_growth = |v: f64| style::percent(if d.grew > 0.0 { v / d.grew } else { 0.0 });
    let pad = |label: &str| format!("   {}", s.dim(&format!("{label:11}")));

    writeln!(c.out, "{}", s.bold(&format!("before: {}: {}", first.path.display(), amount(&d.before))))?;
    writeln!(c.out, "{}", s.dim(&format!("        {}", how_read(first))))?;
    writeln!(c.out, "{}", s.bold(&format!("after:  {}: {}", second.path.display(), amount(&d.after))))?;
    writeln!(c.out, "{}", s.dim(&format!("        {}", how_read(second))))?;
    let objects_change = match (d.before.objects, d.after.objects) {
        (Some(b), Some(a)) => format!(", {} objects", signed(a - b, style::count)),
        _ => String::new(),
    };
    writeln!(c.out, "change: {}{objects_change}, what nothing reaches left out", signed(d.after.bytes - d.before.bytes, style::bytes))?;
    writeln!(c.out, "moved:  {} in the paths that grew, {} in those that shrank or went", style::bytes(d.grew), style::bytes(d.shrank))?;
    if !counted {
        writeln!(c.out, "{}", s.dim("One of the profiles counts no objects: the growth is in bytes only."))?;
    }
    if d.grew == 0.0 {
        writeln!(c.out, "\n{}", s.good("Nothing grew."))?;
        return Ok(result);
    }

    writeln!(c.out)?;
    let signs: Vec<String> =
        shown.iter().enumerate().filter(|(_, x)| x.verdict == Verdict::Collection).map(|(i, _)| (i + 1).to_string()).collect();
    if !signs.is_empty() {
        let (them, what) = if signs.len() == 1 { ("suspect", "a collection that gained") } else { ("suspects", "collections that gained") };
        writeln!(c.out, "{} {them} {}, {what} many objects between the dumps.", s.warn("Signs of a leak:"), signs.join(", "))?;
    } else if !shown.is_empty() {
        writeln!(c.out, "{} what grew is large objects, or objects of a class from no one place.", s.good("No collection gained many objects:"))?;
    } else {
        writeln!(c.out, "{} the growth is spread over the heap, none of it over {} in one place.",
            s.good("No one place gained much:"), style::percent(c.args.share))?;
    }

    for (i, x) in shown.iter().enumerate() {
        writeln!(c.out)?;
        match x.kind {
            Kind::One => {
                let what = if x.statics { format!("The static fields of {}", x.name) }
                    else if x.at > 1.0 { format!("{} {}", style::count(x.at), x.name) }
                    else { format!("One {}", x.name) };
                let objects = x.objects.map_or(String::new(), |n| format!(", {}", style::counted(n, "object")));
                writeln!(c.out, "{}", s.bold(&format!("{}. {what} gained {}, {} of the growth{objects}.",
                    i + 1, style::bytes(x.total), of_growth(x.total))))?;
                writeln!(c.out, "{}{}", pad("size"), was(x.node))?;
                if let Some(g) = &x.gathered {
                    let label = if x.verdict == Verdict::Collection { "collection" } else { "large" };
                    let there = g.there.map_or(String::new(), |n| format!(" ({} there now)", style::count(n)));
                    let each = if g.objects > 0.0 { format!(", {} each", style::bytes(g.total / g.objects)) }
                        else { format!(", which grew by {}", style::bytes(g.total)) };
                    let containers = g.containers.as_ref().map_or(String::new(), |k| format!(", in {} {}", style::count(k.objects), k.name));
                    writeln!(c.out, "{}+{} {}{there}{each}{containers}", pad(label), style::count(g.objects), g.name)?;
                    let through: Vec<&str> = std::iter::once(x.name.as_str())
                        .chain(g.through.iter().map(String::as_str)).chain([g.name.as_str()]).map(style::simple).collect();
                    writeln!(c.out, "{}through {}", pad(""), through.join(" → "))?;
                }
            }
            Kind::Many => {
                writeln!(c.out, "{}", s.bold(&format!("{}. {} more {} hold {}, {} of the growth, from no one place.",
                    i + 1, style::count(x.objects.unwrap_or(0.0)), x.name, style::bytes(x.total), of_growth(x.total))))?;
                writeln!(c.out, "{}{}, at the place that gained most", pad("size"), was(x.node))?;
            }
        }
        let gathered = x.gathered.as_ref().map(|g| g.name.as_str());
        let includes: Vec<String> = x.includes.iter().filter(|k| Some(k.name.as_str()) != gathered).take(3)
            .map(|k| format!("+{} {}", style::count(k.objects), k.name)).collect();
        if !includes.is_empty() {
            writeln!(c.out, "{}{}", pad("includes"), includes.join(", "))?;
        }
        for (j, step) in x.holders.iter().enumerate() {
            let label = if step.root { format!("{}, {}", step.classes[0], style::held_from(&step.classes[0])) } else { label_of(step) };
            writeln!(c.out, "{}{label}", pad(if j == 0 { "held by" } else { "" }))?;
        }
    }
    if !spread.is_empty() {
        writeln!(c.out, "\nAlso over {} of the growth, but over many branches, none of them most:", style::percent(c.args.share))?;
        for x in &spread {
            writeln!(c.out, "{:>9}  {:>5}  {}  {}", style::bytes(x.total), of_growth(x.total), x.name, s.dim(&format!("({})", was(x.node))))?;
        }
    }

    if !gained.is_empty() {
        writeln!(c.out)?;
        writeln!(c.out, "{}", s.bold("Classes, by the objects they gained"))?;
        for k in &gained {
            let objects = match (k.before.objects, k.after.objects) {
                (Some(b), Some(a)) => (signed(a - b, style::count), format!("  ({} → {})", style::count(b), style::count(a))),
                _ => (String::new(), String::new()),
            };
            writeln!(c.out, "{:>12}  {:>11}  {}{}", objects.0, signed(k.after.bytes - k.before.bytes, style::bytes), k.name, s.dim(&objects.1))?;
        }
    }
    writeln!(c.out)?;
    writeln!(c.out, "{}", s.dim("An object reached another way in the later dump moves from one path to another and\n\
        shows in both: what grew and did not move is the change above."))?;
    Ok(result)
}
