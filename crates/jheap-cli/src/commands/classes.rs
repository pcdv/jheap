//! The classes by the bytes of their objects, as `jmap -histo` lists them:
//! what the heap is made of, before who holds it.

use std::io;

use serde_json::json;

use super::{Ctx, Found, emit, header, num, opt, share};
use crate::load::Loaded;
use crate::style;

struct Class {
    name: String,
    bytes: f64,
    objects: Option<f64>,
}

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let p = &l.profile;
    let heap = p.heap();
    let top = c.args.rows.unwrap_or(20);

    // the class of a path is its last frame: the objects that end there
    let mut classes: Vec<Class> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for (i, &node) in p.stack.iter().enumerate() {
        if node < 0 {
            continue;
        }
        let name = p.name(node as usize);
        let at = *index.entry(name).or_insert_with(|| {
            classes.push(Class { name: name.to_string(), bytes: 0.0, objects: p.objects.as_ref().map(|_| 0.0) });
            classes.len() - 1
        });
        classes[at].bytes += p.weight[i];
        if let (Some(o), Some(n)) = (&p.objects, classes[at].objects.as_mut()) {
            *n += o[i];
        }
    }
    classes.sort_by(|a, b| b.bytes.total_cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    let shown = &classes[..classes.len().min(top)];
    let average = |k: &Class| k.objects.filter(|&n| n > 0.0).map(|n| k.bytes / n);
    let result = if classes.is_empty() { Found::Nothing } else { Found::Something };

    if c.args.json {
        let doc = json!({
            "dump": super::dump_json(l, c.peak_memory),
            "classes": shown.iter().map(|k| json!({
                "name": k.name,
                "bytes": num(k.bytes),
                "share": share(k.bytes, heap),
                "objects": opt(k.objects),
                "average": average(k).map(|a| num(a.round())),
            })).collect::<Vec<_>>(),
            "count": classes.len(),
            "more": classes.len() - shown.len(),
        });
        emit(c, doc)?;
        return Ok(result);
    }

    header(c, l)?;
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.bold(&format!("{:>9}  {:>5}  {:>12}  {:>9}  class", "bytes", "share", "objects", "average")))?;
    for k in shown {
        writeln!(
            c.out,
            "{:>9}  {:>5}  {:>12}  {:>9}  {}",
            style::bytes(k.bytes),
            style::percent(if heap > 0.0 { k.bytes / heap } else { 0.0 }),
            k.objects.map_or(String::new(), style::count),
            average(k).map_or(String::new(), style::bytes),
            k.name
        )?;
    }
    if classes.len() > shown.len() {
        let rest: f64 = classes[shown.len()..].iter().map(|k| k.bytes).sum();
        writeln!(
            c.out,
            "{}",
            c.style.dim(&format!(
                "{:>9}  {:>5}  and {} more classes: {}",
                style::bytes(rest),
                style::percent(if heap > 0.0 { rest / heap } else { 0.0 }),
                classes.len() - shown.len(),
                c.suggest(&format!("classes {}", classes.len()))
            ))
        )?;
    }
    writeln!(c.out)?;
    writeln!(c.out, "{}", c.style.dim("Shallow sizes, unreachable objects included: what each class's objects take themselves."))?;
    Ok(result)
}
