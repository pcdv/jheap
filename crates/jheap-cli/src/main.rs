//! jheap: what holds the memory of a JVM, from its heap dump.

mod args;
mod commands;
mod find;
mod load;
mod memory;
mod style;

use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use args::{Args, Color, Command, Parsed, USAGE};
use commands::Ctx;
use load::LoadOptions;
use style::Style;

/// It broke: a file it cannot read, a dump cut short.
const BROKE: u8 = 1;
/// Called wrong.
const WRONG: u8 = 2;

fn main() -> ExitCode {
    match args::parse(std::env::args_os().skip(1).collect()) {
        Err(e) => {
            eprintln!("jheap: {e}\nRun jheap --help for how it is called.");
            ExitCode::from(WRONG)
        }
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Parsed::Version) => {
            println!("jheap {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Parsed::Run(args)) => run(&args),
    }
}

fn run(args: &Args) -> ExitCode {
    // the dumps named, or the most recent of the directory, said on stderr so
    // that a wrong guess is seen
    let needed = if args.command == Command::Diff { 2 } else { 1 };
    let (paths, named): (Vec<PathBuf>, Vec<Option<String>>) = if args.dumps.is_empty() {
        let dir = args.dir.clone().unwrap_or_else(|| PathBuf::from("."));
        let found = match find::latest(&dir, needed) {
            Ok(found) => found,
            Err(e) => {
                eprintln!("jheap: {e}");
                return ExitCode::from(WRONG);
            }
        };
        for f in &found {
            eprintln!("jheap: using {} ({}), the most recent in {}", f.path.display(), find::ago(f.modified), dir.display());
        }
        let named = found.iter().map(|_| None).collect();
        (found.into_iter().map(|f| f.path).collect(), named)
    } else {
        let named = args.dumps.iter().map(|d| Some(d.to_string_lossy().into_owned())).collect();
        (args.dumps.clone(), named)
    };

    let options = LoadOptions {
        oops: args.oops,
        fresh: args.fresh,
        keep: args.keep,
        save: args.save.clone(),
        progress: true,
    };
    let mut loaded = Vec::new();
    for path in &paths {
        match load::load(path, &options) {
            Ok(l) => loaded.push(l),
            Err(e) => {
                eprintln!("jheap: {e}");
                return ExitCode::from(BROKE);
            }
        }
    }

    let color = match args.color {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
    };
    let style = Style { color: color && ansi() };
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let mut ctx = Ctx {
        args,
        style,
        out: &mut out,
        named,
        peak_memory: args.stats.then(memory::peak),
    };
    let result = match &args.command {
        Command::Summary => commands::summary::run(&mut ctx, &loaded[0]),
        Command::Peaks => commands::peaks::run(&mut ctx, &loaded[0]),
        Command::Leaks => commands::leaks::run(&mut ctx, &loaded[0]),
        Command::Classes => commands::classes::run(&mut ctx, &loaded[0]),
        Command::Trace(class) => commands::trace::run(&mut ctx, &loaded[0], class),
        Command::Info => commands::info::run(&mut ctx, &loaded[0]),
        Command::Diff => commands::diff::run(&mut ctx, &loaded[0], &loaded[1]),
    };
    let result = result.and_then(|found| out.flush().map(|_| found));
    match result {
        Ok(found) => ExitCode::from(found as u8),
        // a reader that stopped, as `head` does: what it read was enough
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jheap: cannot write the output: {e}");
            ExitCode::from(BROKE)
        }
    }
}

/// Whether the terminal reads colour codes: Windows' console does once
/// asked to, and says so.
#[cfg(windows)]
fn ansi() -> bool {
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn GetConsoleMode(console: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(console: isize, mode: u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    // SAFETY: the handle is this process's standard output, and the mode a
    // word written by the call
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            // not a console: a terminal emulator's pipe, which reads them
            return true;
        }
        mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
            || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn ansi() -> bool {
    true
}
