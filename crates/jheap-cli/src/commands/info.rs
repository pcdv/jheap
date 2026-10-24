//! The dump and its parse: what the other commands' header leaves out.

use std::io;

use serde_json::json;

use super::{Ctx, Found, emit, header};
use crate::load::Loaded;
use crate::style;

pub fn run(c: &mut Ctx, l: &Loaded) -> io::Result<Found> {
    let p = &l.profile;
    if c.args.json {
        let mut doc = json!({"dump": super::dump_json(l, c.peak_memory)});
        if let Some(r) = &l.report {
            doc["report"] = serde_json::to_value(r).map_err(io::Error::other)?;
        }
        doc["profile"] = json!({"nodes": p.len(), "paths": p.stack.len()});
        emit(c, doc)?;
        return Ok(Found::Something);
    }

    header(c, l)?;
    writeln!(c.out)?;
    let Some(r) = &l.report else {
        writeln!(c.out, "A profile without the report of its parse: {} paths, {} nodes.", style::count(p.stack.len() as f64), style::count(p.len() as f64))?;
        return Ok(Found::Something);
    };
    let mut lines: Vec<(&str, String)> = vec![
        ("format", format!("{}, {}-byte ids, {} bytes", r.version, r.id_size, style::count(r.file_size as f64))),
        ("pointers", if r.pointers_guessed {
            format!("{}, guessed from a heap spanning {}: --oops says otherwise", r.pointers.name(), style::bytes(r.heap_span as f64))
        } else {
            r.pointers.name().to_string()
        }),
        ("objects", format!(
            "{} ({} instances, {} object arrays, {} primitive arrays)",
            style::count((r.instance_count + r.object_array_count + r.primitive_array_count) as f64),
            style::count(r.instance_count as f64),
            style::count(r.object_array_count as f64),
            style::count(r.primitive_array_count as f64)
        )),
        ("classes", format!("{}, strings {}", style::count(r.class_count as f64), style::count(r.string_count as f64))),
    ];
    let mut roots = r.roots_by_type.clone();
    roots.sort_by_key(|r| std::cmp::Reverse(r.1));
    lines.push(("roots", format!(
        "{}: {}",
        style::count(r.root_count as f64),
        roots.iter().map(|(k, v)| format!("{k} {}", style::count(*v as f64))).collect::<Vec<_>>().join(", ")
    )));
    lines.push(("heap", format!(
        "{} reachable, {} unreachable ({})",
        style::bytes(r.reachable_size as f64),
        style::bytes(r.unreachable_size as f64),
        style::percent(r.unreachable_size as f64 / (r.total_size as f64).max(1.0))
    )));
    lines.push(("paths", format!(
        "{} distinct, {} deep at most; {} objects deeper than 31 filed under their last 31 frames",
        style::count(r.unique_path_count as f64),
        r.max_depth_reached,
        style::count(r.depth_limit_hits as f64)
    )));
    lines.push(("parse", format!(
        "{} (strings and classes {}, objects {}, walk {})",
        style::duration(r.parse_time_ms),
        style::duration(r.metadata_time_ms),
        style::duration(r.objects_time_ms),
        style::duration(r.graph_time_ms)
    )));
    for (k, v) in lines {
        writeln!(c.out, "{}{v}", c.style.dim(&format!("{k:10}")))?;
    }
    Ok(Found::Something)
}
