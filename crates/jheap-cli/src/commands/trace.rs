//! One class, both directions: the heaviest object of the class, what it
//! holds and who holds it.
//!
//! The class is the one named exactly, else the ones whose simple name it is,
//! else those whose name contains it; when several do, they are listed
//! instead, for the caller to name one.

use std::io;

use jheap::model::ownership::{holders_of, holds_of};
use jheap::model::retainers::is_class;
use serde_json::json;

use super::{Ctx, Found, emit, header, num, opt, row};
use crate::load::Loaded;
use crate::style;

/// The classes a name picks, by how closely it names them.
pub fn pick<'a>(names: &[&'a str], wanted: &str) -> Vec<&'a str> {
    let simple = |n: &str| -> usize { n.rfind(['.', '$']).map_or(0, |i| i + 1) };
    let exact: Vec<&str> = names.iter().copied().filter(|n| *n == wanted).collect();
    if !exact.is_empty() {
        return exact;
    }
    let suffix: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| &n[simple(n)..] == wanted || n.ends_with(&format!(".{wanted}")) || n.ends_with(&format!("${wanted}")))
        .collect();
    if !suffix.is_empty() {
        return suffix;
    }
    names.iter().copied().filter(|n| n.contains(wanted)).collect()
}

pub fn run(c: &mut Ctx, l: &Loaded, wanted: &str) -> io::Result<Found> {
    let p = &l.profile;
    let s = p.sums();
    let heap = p.heap();
    let top = c.args.rows.unwrap_or(20);

    // the nodes of each class, by name
    let mut by_name: Vec<(&str, Vec<usize>)> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for n in 0..p.len() {
        let name = p.name(n);
        if !is_class(name) {
            continue;
        }
        let at = *index.entry(name).or_insert_with(|| {
            by_name.push((name, Vec::new()));
            by_name.len() - 1
        });
        by_name[at].1.push(n);
    }
    let names: Vec<&str> = by_name.iter().map(|(n, _)| *n).collect();
    let mut picked = pick(&names, wanted);
    let nodes_of = |name: &str| &by_name[index[name]].1;
    let heaviest = |name: &str| -> usize {
        let nodes = nodes_of(name);
        let mut best = nodes[0];
        for &n in &nodes[1..] {
            if s.total[n] > s.total[best] {
                best = n;
            }
        }
        best
    };

    if picked.is_empty() {
        if c.args.json {
            emit(c, json!({"dump": super::dump_json(l, c.peak_memory), "query": wanted, "found": "none"}))?;
        } else {
            header(c, l)?;
            writeln!(c.out, "\nNo class is named {wanted}, nor ends or contains it. {} lists the heaviest.", c.suggest("classes"))?;
        }
        return Ok(Found::Nothing);
    }
    if picked.len() > 1 {
        picked.sort_by(|a, b| s.total[heaviest(b)].total_cmp(&s.total[heaviest(a)]));
        if c.args.json {
            emit(c, json!({
                "dump": super::dump_json(l, c.peak_memory),
                "query": wanted,
                "found": "several",
                "candidates": picked.iter().take(top).map(|n| json!({"name": n, "total": num(s.total[heaviest(n)])})).collect::<Vec<_>>(),
                "more": picked.len().saturating_sub(top),
            }))?;
        } else {
            header(c, l)?;
            writeln!(c.out, "\n{} classes match {wanted}: name one. Each with the most one of its objects holds:", picked.len())?;
            for n in picked.iter().take(top) {
                writeln!(c.out, "{}", row(s.total[heaviest(n)], s.total[heaviest(n)] / heap.max(1.0), n))?;
            }
            if picked.len() > top {
                writeln!(c.out, "  and {} more", style::count((picked.len() - top) as f64))?;
            }
        }
        return Ok(Found::Several);
    }

    let name = picked[0];
    let node = heaviest(name);
    let nodes = nodes_of(name);
    let objects = s.objects.as_ref().map(|o| nodes.iter().map(|&n| o[n]).sum::<f64>());
    let held = |n: usize| s.held.as_ref().map(|h| h[n]);
    let holds = holds_of(p, node, 0.0).map(|f| f.children).unwrap_or_default();
    let chain = holders_of(p, node, 0.0);
    let mut holders = Vec::new();
    let mut f = chain.as_ref().and_then(|c| c.children.first());
    while let Some(h) = f {
        holders.push(h);
        f = h.children.first();
    }

    if c.args.json {
        emit(c, json!({
            "dump": super::dump_json(l, c.peak_memory),
            "query": wanted,
            "found": "one",
            "name": name,
            "objects": opt(objects),
            "paths": nodes.len(),
            "heaviest": {"total": num(s.total[node]), "own": num(s.own[node]), "objects": opt(held(node))},
            "holds": holds.iter().take(top).map(|h| json!({
                "name": h.name, "total": num(h.total), "own": num(h.own), "objects": opt(held(h.node)),
            })).collect::<Vec<_>>(),
            "holdsMore": holds.len().saturating_sub(top),
            "holders": holders.iter().map(|h| json!({"name": h.name, "total": num(h.total)})).collect::<Vec<_>>(),
        }))?;
        return Ok(Found::Something);
    }

    header(c, l)?;
    writeln!(c.out)?;
    let paths = style::counted(nodes.len() as f64, "path");
    let first = match objects {
        Some(n) => format!("{name}: {} on {paths}", style::counted(n, "object")),
        None => format!("{name}: on {paths}"),
    };
    let heaviest_holds = held(node).map_or(String::new(), |n| format!(" in {}", style::counted(n, "object")));
    writeln!(c.out, "{}", c.style.bold(&first))?;
    writeln!(c.out, "the heaviest path holds {}{heaviest_holds}, {} of its own", style::bytes(s.total[node]), style::bytes(s.own[node]))?;

    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold("What it holds, the heaviest first"))?;
    if holds.is_empty() {
        writeln!(c.out, "  nothing: its own bytes only")?;
    }
    for h in holds.iter().take(top) {
        let n = held(h.node).map_or(String::new(), |n| format!(", {}", style::counted(n, "object")));
        writeln!(c.out, "{}", row(h.total, h.total / heap.max(1.0), &format!("{}  {}", h.name, c.style.dim(&format!("(own {}{n})", style::bytes(h.own))))))?;
    }
    if holds.len() > top {
        writeln!(c.out, "  and {} more", holds.len() - top)?;
    }

    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold("Who holds it, the nearest first"))?;
    for h in &holders {
        writeln!(c.out, "{}", row(h.total, h.total / heap.max(1.0), &h.name))?;
    }
    if holders.is_empty() {
        writeln!(c.out, "  nothing: it is a root")?;
    }
    if let Some(root) = holders.last() {
        let lives = style::lives(&root.name).unwrap_or(style::held_from(&root.name));
        writeln!(c.out, "{}", c.style.dim(&format!("{:18}{lives}", "")))?;
    }
    Ok(Found::Something)
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn picks_the_closest_names() {
        let names = ["com.example.Config", "com.example.Config$Value", "com.example.store.Store", "com.example.StoreCache"];
        assert_eq!(pick(&names, "com.example.Config"), ["com.example.Config"]);
        assert_eq!(pick(&names, "Config"), ["com.example.Config"]);
        assert_eq!(pick(&names, "Value"), ["com.example.Config$Value"]);
        assert_eq!(pick(&names, "store.Store"), ["com.example.store.Store"]);
        assert_eq!(pick(&names, "Store"), ["com.example.store.Store"]);
        assert_eq!(pick(&names, "Stor"), ["com.example.store.Store", "com.example.StoreCache"]);
        assert!(pick(&names, "Nothing").is_empty());
    }
}
