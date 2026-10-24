//! The command line: the dump first, then what to ask of it.

use std::ffi::OsString;
use std::path::PathBuf;

use jheap::Pointers;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// What holds the memory, whether anything looks like a leak, what next.
    Summary,
    Peaks,
    Leaks,
    Classes,
    /// A class, by its full name, its simple name or a part of either.
    Trace(String),
    Diff,
    Info,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Summary => "summary",
            Command::Peaks => "peaks",
            Command::Leaks => "leaks",
            Command::Classes => "classes",
            Command::Trace(_) => "trace",
            Command::Diff => "diff",
            Command::Info => "info",
        }
    }
}

const COMMANDS: [&str; 6] = ["peaks", "leaks", "classes", "trace", "diff", "info"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub command: Command,
    /// As named; none to take the most recent of the directory.
    pub dumps: Vec<PathBuf>,
    /// How many rows; each command has its default.
    pub rows: Option<usize>,
    /// The part of the heap, 0 to 1, a leak suspect holds at least.
    pub share: f64,
    pub json: bool,
    /// Parse again, even when a parse is kept.
    pub fresh: bool,
    /// Keep the parse beside the dump.
    pub keep: bool,
    pub stats: bool,
    pub oops: Option<Pointers>,
    pub save: Option<PathBuf>,
    /// Where to look for the most recent dump.
    pub dir: Option<PathBuf>,
    pub color: Color,
}

pub enum Parsed {
    Run(Args),
    Help,
    Version,
}

pub const USAGE: &str = "\
jheap: what holds the memory of a JVM, from its heap dump

Usage: jheap [<dump>] [<command> [<args>]] [options]
       jheap diff [<earlier> <later>] [options]

The dump comes first, then what to ask of it. Without a dump, the most recent
.hprof, .hprof.gz or .jheap.json of the directory is used, and named on stderr.

Commands:
  (none)            what holds the memory, whether anything could leak, what next
  peaks [<n>]       the n heaviest peaks (10), what each is made of and who holds it
  leaks [<n>]       what could grow: the leak suspects, each with a verdict
  classes [<n>]     the n heaviest classes (20): bytes, objects, average size
  trace <class>     what holds a class and what it holds; a full name, a simple
                    one, or a part of either
  diff <earlier> <later> [<n>]
                    what grew between two dumps of a process, and whether it leaks
  info              the dump and its parse: format, counts, roots, timings

Options:
  --share <percent>   the share of the heap a leak suspect holds at least (10)
  --json              one JSON document on stdout instead of text
  --fresh             parse again, even when a parse is kept beside the dump
  --no-keep           keep no parse beside the dump
  --oops <width>      compressed or wide references; guessed from the heap's span
  --save <file>       write the profile there too, for heap-landscape or jfr-web
  --dir <path>        look for the most recent dump there, not here
  --stats             parse again and say what it cost: time and peak memory
  --color <when>      auto, always or never
  -h, --help          this
  -V, --version

The parse of a dump is kept beside it as <name>.jheap.json and read in its place
the next time, if the dump has not changed.

Exit codes: 0 found something, 1 it broke, 2 called wrong, 3 found nothing,
4 a traced name matches several classes.";

fn number(s: &str) -> Option<usize> {
    s.parse::<usize>().ok().filter(|&n| n > 0)
}

pub fn parse(argv: Vec<OsString>) -> Result<Parsed, String> {
    let mut words: Vec<OsString> = Vec::new();
    let mut args = Args {
        command: Command::Summary,
        dumps: Vec::new(),
        rows: None,
        share: 0.1,
        json: false,
        fresh: false,
        keep: true,
        stats: false,
        oops: None,
        save: None,
        dir: None,
        color: Color::Auto,
    };

    let mut it = argv.into_iter();
    let mut flags_done = false;
    while let Some(arg) = it.next() {
        let text = arg.to_string_lossy().into_owned();
        if flags_done || !text.starts_with('-') || text == "-" {
            words.push(arg);
            continue;
        }
        if text == "--" {
            flags_done = true;
            continue;
        }
        let (flag, inline) = match text.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(OsString::from(v))),
            _ => (text.clone(), None),
        };
        let mut value = |name: &str| -> Result<OsString, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--json" => args.json = true,
            "--fresh" => args.fresh = true,
            "--no-keep" => args.keep = false,
            "--stats" => {
                args.stats = true;
                args.fresh = true;
            }
            "--share" => {
                let v = value("--share")?;
                let v = v.to_string_lossy();
                let percent: f64 = v
                    .trim_end_matches('%')
                    .parse()
                    .ok()
                    .filter(|p: &f64| *p > 0.0 && *p <= 100.0)
                    .ok_or_else(|| format!("--share takes a percent, over 0 and up to 100: not {v}"))?;
                args.share = percent / 100.0;
            }
            "--oops" => {
                args.oops = match value("--oops")?.to_string_lossy().as_ref() {
                    "compressed" => Some(Pointers::Compressed),
                    "wide" => Some(Pointers::Wide),
                    "auto" => None,
                    other => return Err(format!("--oops takes compressed, wide or auto: not {other}")),
                }
            }
            "--save" => args.save = Some(PathBuf::from(value("--save")?)),
            "--dir" => args.dir = Some(PathBuf::from(value("--dir")?)),
            "--color" | "--colour" => {
                args.color = match value("--color")?.to_string_lossy().as_ref() {
                    "auto" => Color::Auto,
                    "always" => Color::Always,
                    "never" => Color::Never,
                    other => return Err(format!("--color takes auto, always or never: not {other}")),
                }
            }
            _ => return Err(format!("no option {flag}")),
        }
    }

    let is_command = |w: &OsString| COMMANDS.contains(&w.to_string_lossy().as_ref());
    // the dump first, unless the first word is a command: a dump named like
    // one is passed as ./peaks
    let mut rest = words.into_iter().peekable();
    if rest.peek().is_some_and(|w| !is_command(w)) {
        args.dumps.push(PathBuf::from(rest.next().unwrap()));
    }
    let Some(command) = rest.next() else {
        return Ok(Parsed::Run(args));
    };
    let command = command.to_string_lossy().into_owned();
    if !COMMANDS.contains(&command.as_str()) {
        return Err(format!(
            "no command {command}: the commands are {}. A second dump goes with diff: jheap diff <earlier> <later>",
            COMMANDS.join(", ")
        ));
    }
    let rest: Vec<String> = rest.map(|w| w.to_string_lossy().into_owned()).collect();
    let rows = |extra: &[String]| -> Result<Option<usize>, String> {
        match extra {
            [] => Ok(None),
            [n] => number(n).map(Some).ok_or_else(|| format!("{command} takes a number of rows: not {n}")),
            [_, more @ ..] => Err(format!("{command} takes one number of rows at most: not {}", more.join(" "))),
        }
    };
    args.command = match command.as_str() {
        "peaks" => {
            args.rows = rows(&rest)?;
            Command::Peaks
        }
        "leaks" => {
            args.rows = rows(&rest)?;
            Command::Leaks
        }
        "classes" => {
            args.rows = rows(&rest)?;
            Command::Classes
        }
        "info" => {
            if !rest.is_empty() {
                return Err(format!("info takes nothing more: not {}", rest.join(" ")));
            }
            Command::Info
        }
        "trace" => match rest.as_slice() {
            [class] => Command::Trace(class.clone()),
            [] => return Err("trace takes the class to trace: jheap <dump> trace <class>".into()),
            [_, more @ ..] => return Err(format!("trace takes one class: not {}", more.join(" "))),
        },
        "diff" => {
            // the dumps follow the command; one before it, as `jheap a diff b`,
            // is taken as the earlier
            let mut extra = rest.as_slice();
            while let Some((first, tail)) = extra.split_first() {
                if args.dumps.len() == 2 || (number(first).is_some() && tail.is_empty()) {
                    break;
                }
                args.dumps.push(PathBuf::from(first));
                extra = tail;
            }
            if args.dumps.len() == 1 {
                return Err("diff takes two dumps, the earlier first, or none for the two most recent".into());
            }
            args.rows = rows(extra)?;
            Command::Diff
        }
        _ => unreachable!(),
    };
    Ok(Parsed::Run(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(line: &str) -> Args {
        match parse(line.split_whitespace().map(OsString::from).collect()) {
            Ok(Parsed::Run(a)) => a,
            Ok(_) => panic!("{line}: help or version"),
            Err(e) => panic!("{line}: {e}"),
        }
    }

    fn fails(line: &str) -> String {
        match parse(line.split_whitespace().map(OsString::from).collect()) {
            Err(e) => e,
            Ok(_) => panic!("{line}: accepted"),
        }
    }

    #[test]
    fn the_dump_comes_first() {
        let a = run("heap.hprof");
        assert_eq!((a.command, a.dumps), (Command::Summary, vec![PathBuf::from("heap.hprof")]));
        let a = run("heap.hprof peaks 20 --json");
        assert_eq!((a.command, a.rows, a.json), (Command::Peaks, Some(20), true));
        let a = run("heap.hprof trace store.Store");
        assert_eq!(a.command, Command::Trace("store.Store".into()));
        let a = run("heap.hprof leaks --share 5");
        assert!((a.share - 0.05).abs() < 1e-12);
    }

    #[test]
    fn or_is_left_out() {
        let a = run("leaks");
        assert_eq!((a.command, a.dumps.len()), (Command::Leaks, 0));
        let a = run("trace Config --dir=/var/dumps");
        assert_eq!(a.dir, Some(PathBuf::from("/var/dumps")));
        // a dump named like a command
        assert_eq!(run("./peaks").dumps, vec![PathBuf::from("./peaks")]);
    }

    #[test]
    fn diff_takes_two_dumps_or_none() {
        let a = run("diff a.hprof b.hprof 5");
        assert_eq!((a.dumps.len(), a.rows), (2, Some(5)));
        assert_eq!(run("diff").dumps.len(), 0);
        assert_eq!(run("a.hprof diff b.hprof").dumps, vec![PathBuf::from("a.hprof"), PathBuf::from("b.hprof")]);
        assert!(fails("diff a.hprof").contains("two dumps"));
    }

    #[test]
    fn says_what_is_wrong() {
        assert!(fails("heap.hprof peek").contains("no command peek"));
        assert!(fails("heap.hprof peaks ten").contains("number of rows"));
        assert!(fails("heap.hprof trace").contains("the class to trace"));
        assert!(fails("--oops narrow").contains("compressed, wide"));
        assert!(fails("--bogus").contains("no option"));
    }
}
