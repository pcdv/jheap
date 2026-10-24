# jheap

Reads JVM heap dumps (`.hprof`, `.hprof.gz`) and says what holds the memory. Two
consumers:

- **an agent or a person at a terminal**, through the `jheap` command: a single
  native binary, no runtime to install, meant to run on the server next to the dump;
- **a person in a browser**, through heap-landscape's web app, which draws the heap
  as a landscape and gets its parse from jheap compiled to WebAssembly.

## Where this comes from

jheap is the Rust rewrite of the analysis half of
[heap-landscape](../heap-landscape): its parser and its `hl` command, which
heap-landscape no longer has. Its commit `75ac409`, the last before the split,
holds the JS implementation they were ported from, and says what an unexplained
behaviour was meant to do: `git -C ../heap-landscape show 75ac409:<path>`.

| What | In heap-landscape at `75ac409` |
|---|---|
| Parser: streaming cursor, object store, walk from the roots | `web/js/hprof/parser.js`, `constants.js` |
| From the parse to a profile (paths → bytes, objects) | `web/js/load.js` |
| Profile JSON, jfr-web's version 4 | `web/js/profile/profile.js`, `encode.js` |
| Ownership chains, the holders of an object | `web/js/model/ownership.js` |
| CLI-only analyses: retainers, leaks, diff | `web/js/model/retainers.js`, `leaks.js`, `diff.js` |
| The command, kept parses, exit codes | `tools/report.mjs` |
| Design notes, limits, measurements | `PLAN.md`, `README.md` |

## Crates

- `crates/jheap`: the library. Parser, object store, profile, models. No I/O beyond
  `Read + Seek`, so it builds for wasm32 unchanged.
- `crates/jheap-cli`: the `jheap` binary.
- `crates/jheap-wasm`: the web app's parser. Parses a dropped file in a Web
  Worker (synchronous reads of the `File`, behind `Read + Seek`) and hands back the
  profile and the report, which are small: 1.8 MB for a 4.6 GB dump. The object
  graph never crosses into JavaScript. No wasm-bindgen: a few plain exports,
  which heap-landscape's `web/js/hprof/jheap.js` wraps. The page keeps its own
  `model/ownership.js`, which names the same chains as `model::ownership`.

## Plan

1. **Done.** Reference set: test dumps and what the JS implementation makes of
   them, in `fixtures/` (see its README).
2. **Done.** Port the parser until it gives the expected parses. It gives them,
   and the same parse as `hl` on every local real dump; a 4.6 GB dump of 46M
   objects parses in 3.7 s at a peak of 964 MiB, 22 bytes an object
   (`cargo run --release --example measure -- parse <dump>`).
3. **Done.** Port the CLI-only models and the command. The models (`model/`,
   over `Profile`), shaped as `report.mjs` shapes them, give the fixtures'
   baseline JSON and `hl`'s on every local real dump. The command is jheap's
   own (see the README): a one-screen summary by default, `classes` and `info`
   added, JSON versioned; `crates/jheap-cli/tests/cli.rs` runs it. The command may be better than `hl`:
   the baseline is there to compare with, not to copy.
4. **Done.** Build the wasm package, run it in the web app, and delete the JS
   parser and the CLI-only modules from heap-landscape. The repositories split
   for good here. The page parses with jheap in a Web Worker that reads the
   `File` through `FileReaderSync`, and opens a `.jheap.json`. A 4.6 GB dump
   opens in 18 s there, against 4 s natively: the reads through
   `FileReaderSync` are most of the difference. heap-landscape's parse tests run
   jheap's module. `hl` is gone; the user's heap-dump-analysis skill calls jheap.

Later: a dominator tree (heap-landscape's holder tree is a breadth-first spanning
tree, see its PLAN.md), threads for the first pass, memory-mapped reads.

## What must not change, and what may

- **The parse** (`fixtures/expected/*.parse.json`): every path from a root with its
  bytes and objects, the counts of the report. The web app draws exactly that. A
  deliberate improvement (e.g. the walk's depth limit of 30, which leaves the end of
  a long chain without its root) changes the expected file in the same commit and
  says why in the message.
- **The profile document**: jfr-web's version 4 JSON plus `samples.objects` and a
  `heap` object (`key`, `report`), as `tools/report.mjs` writes it
  (`profile/document.rs`). heap-landscape
  and jfr-web read it. The parse kept beside a dump is named `<name>.jheap.json`.
- **Exit codes**, which agents rely on: 0 found, 1 broke, 2 called wrong, 3 found
  nothing, 4 a traced name matches several classes.
- **Everything else in the command's output may improve**: wording, layout, JSON
  shape (version it), flags.

## Constraints

- A dump is never held whole in memory: it is read through a window, and only the
  object graph is kept. Memory grows with the number of objects, not the file size.
  The JS version needs about 55 bytes an object (a 4.6 GB dump of 46M objects peaks
  at 2.5 GiB, in 13 s); do better, and measure it.
- wasm32 addresses 4 GiB: the browser handles up to roughly 100M objects. Beyond,
  the CLI parses and the page opens its `.jheap.json`.
- References' width is guessed from the heap's span (compressed under 32 GB, wide
  above), `--oops` overrides. Object sizes are aligned to 8.

## Real dumps never leave the machine

Production dumps hold whatever the process had in memory, and even their parse
names the application's classes: neither is committed, here or anywhere. What is
committed is synthetic:

- the small dumps of `fixtures/`, built byte by byte;
- if a shape needs a real JVM (its own layout, its JDK classes), a small Java
  program that builds a heap of that shape and dumps itself, with the program
  committed and the dump generated by the test run, not stored.

Real dumps are still worth testing against, locally: a test ignored by default
reads `JHEAP_REAL_DUMPS`, local directories of dumps (a path list, searched
with their subdirectories), and for each dump with a `.hl.json` beside it,
written by `hl --fresh`, compares jheap's parse with it path by path, gzipped
dumps unzipped aside first, and prints the time beside the JS version's.
`cargo test --release -p jheap --test real_dumps -- --ignored --nocapture` runs it.

## Inspiration and prior art

- ripgrep, fd: the plain command does the useful thing, colour on a terminal and
  plain text in a pipe, `--json` for machines, published benchmarks, a library
  crate under a thin binary, release binaries plus `cargo install` and scoop.
- [hprof-slurp](https://github.com/agourlay/hprof-slurp): a fast Rust hprof
  analyser, multithreaded. The speed to beat; it has no ownership, leaks or diff.
- `jvm-hprof` on crates.io: a parser crate, 0.1.0. Worth reading, not depending on.

## Working here

- `cargo test` runs everything; the fixtures' tests read `fixtures/`.
- `node crates/jheap-wasm/install.mjs` builds the WebAssembly module and copies
  it into heap-landscape beside this checkout, where it is committed: the page
  has no build step. Run heap-landscape's `npm test` after, which parses with it.
- The fixtures are what the JS implementation made, and are not regenerated
  any more: a deliberate change to the parse edits `fixtures/expected/` in the
  same commit, as said above. `fixtures/generate/record.js` still runs against a
  checkout of heap-landscape at `75ac409` (`git worktree add`), named by
  `HEAP_LANDSCAPE`, to see what the JS implementation made of a new dump.
- Comments say why, in full sentences, as in heap-landscape's code.
