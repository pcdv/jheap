//! The most recent dumps of a directory, for a command that names none.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A dump, or the parse kept of one: the two are one candidate, the dump's.
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// What a file is called without the suffix of a dump or a kept parse, or
/// None for another file.
fn stem(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    for suffix in [".hprof.gz", ".hprof", ".jheap.json"] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return Some(&name[..name.len() - suffix.len()]);
        }
    }
    None
}

/// The `count` most recent dumps of `dir`, the earlier first. A kept parse
/// counts as the dump it was kept for, when that is there, so a dump and its
/// parse are one candidate; alone, it stands for the dump it was kept for.
pub fn latest(dir: &Path, count: usize) -> Result<Vec<Found>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    let mut found: Vec<(String, Found, bool)> = Vec::new();
    for entry in entries.flatten() {
        // in the current directory, a name as the user would type it
        let path = if dir == Path::new(".") { PathBuf::from(entry.file_name()) } else { entry.path() };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = stem(&name) else { continue };
        let is_dump = !name.to_ascii_lowercase().ends_with(".jheap.json");
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        match found.iter_mut().find(|(s, _, _)| s == stem) {
            // the dump stands for its parse, and its time is the dump's: the
            // parse is written after it
            Some((_, f, dump)) if is_dump && !*dump => {
                *f = Found { path, modified };
                *dump = true;
            }
            Some(_) => {}
            None => found.push((stem.to_string(), Found { path, modified }, is_dump)),
        }
    }
    let mut found: Vec<Found> = found.into_iter().map(|(_, f, _)| f).collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.modified));
    found.truncate(count);
    found.reverse();
    if found.len() < count {
        let what = if count == 1 { "no dump".to_string() } else { format!("{} dumps of the {count} needed", found.len()) };
        return Err(format!(
            "{what} in {}: no .hprof, .hprof.gz or .jheap.json to take. Name {}, or --dir <path> looks elsewhere",
            dir.display(),
            if count == 1 { "one" } else { "them" }
        ));
    }
    Ok(found)
}

/// How long ago, roughly, in words.
pub fn ago(when: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(when).map_or(0, |d| d.as_secs());
    let (n, unit) = match secs {
        0..60 => return "just now".into(),
        60..3600 => (secs / 60, "minute"),
        3600..86400 => (secs / 3600, "hour"),
        _ => (secs / 86400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(dir: &Path, name: &str, age: u64) {
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        let when = SystemTime::now() - Duration::from_secs(age);
        fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn the_most_recent_dump_and_its_parse_are_one() {
        let dir = std::env::temp_dir().join(format!("jheap-find-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        touch(&dir, "old.hprof", 3000);
        // its parse, written after it, does not make it the newest
        touch(&dir, "mid.hprof.gz", 2000);
        touch(&dir, "old.jheap.json", 10);
        touch(&dir, "notes.txt", 0);
        touch(&dir, "alone.jheap.json", 1000);

        let one = latest(&dir, 1).unwrap();
        assert_eq!(one[0].path.file_name().unwrap(), "alone.jheap.json");
        let two = latest(&dir, 3).unwrap();
        let names: Vec<_> = two.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["old.hprof", "mid.hprof.gz", "alone.jheap.json"]);
        assert!(latest(&dir, 4).unwrap_err().contains("3 dumps of the 4 needed"));
        fs::remove_dir_all(&dir).ok();
    }
}
