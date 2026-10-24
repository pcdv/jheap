# jheap

What holds the memory of a JVM, from its heap dump.

```sh
jheap heap.hprof                    # what holds the memory, and whether it could leak
jheap heap.hprof leaks              # the leak suspects
jheap diff before.hprof after.hprof # what grew between two dumps
```

A single binary, no runtime to install: copy it next to the dump. It also runs in
the browser, as WebAssembly, behind mountain-graph's drawing of the heap.

A dump of 4.6 GB, 46 million objects, is read in 4 seconds at a peak of 1 GB of
memory, 22 bytes an object: the dump itself is never held, only its object graph.

## Usage

```
jheap [<dump>] [<command> [<args>]] [options]
jheap diff [<earlier> <later>] [options]
```

The dump comes first, then what to ask of it: the command reads as a sentence,
"on this dump, find the leaks". `diff` is the exception, as it takes two dumps: it
comes first, and the dumps follow, the earlier one first.

| Command | Says |
|---|---|
| (none) | what holds the memory, whether anything could leak, and what to ask next |
| `peaks [<n>]` | the `n` heaviest peaks (10): what each is made of, and who holds it, outward |
| `leaks [<n>]` | what could grow: the leak suspects, each with a verdict |
| `classes [<n>]` | the `n` heaviest classes (20): bytes, objects, average size |
| `trace <class>` | what holds a class and what it holds; the full name, the simple one, or a part |
| `diff <earlier> <later> [<n>]` | what grew between two dumps of a process, and whether it looks like a leak |
| `info` | the dump and its parse: format, reference width, counts, roots, timings |

Without a command, jheap answers in one screen:

```
$ jheap server-after.hprof
server-after.hprof: 618.7 KiB in 949 objects, 2.1% unreachable
parsed in 3 ms, kept as server-after.jheap.json

Where the memory is
1. 525.4 KiB    85%  byte[] ← com.example.Session
                     held by com.example.Session ← com.example.Registry ← com.example.Server, from a thread's stack
2.  64.0 KiB    10%  byte[] ← java.util.zip.ZipFile$Source
                     held by java.util.zip.ZipFile$Source, from a static field
3.  12.8 KiB   2.1%  byte[] ← (unreachable)
                     nothing reaches it: garbage
…

Could grow: one com.example.Registry holds 85% of the heap in a collection of 400 com.example.Session.
and 1 more suspect: jheap server-after.hprof leaks

Next
  jheap server-after.hprof peaks                       each peak, what it is made of and who holds it
  jheap server-after.hprof leaks                       the leak suspects, each with what it is made of
  jheap server-after.hprof classes                     the heaviest classes
  jheap server-after.hprof trace com.example.Registry  what holds a Registry and what it holds
  jheap diff <earlier> server-after.hprof              what grew since an earlier dump
```

A peak is where memory gathers: the object that owns a chain of objects, named
by its class and the one holding it. Its holders are listed outward, a line each
time what they hold grows by over 10%, named by the classes of the application in
them: the class to look at is rarely the map next to the memory, but the one
further out where peaks meet.

A leak suspect is an object of the application holding over 10% of the heap
(`--share 5` looks lower), or many objects of one class that together do. Its
verdict tells a leak from a footprint, as far as one dump can:

- **collection**: many objects of one class in a few collections. It grows with
  what it holds, which is how a leak looks: a later dump tells whether it did.
- **large**: a few large objects, bounded unless their number grows.
- **instances**: many objects of a class, held from no one place: what the
  application is made of.

`diff` says what one dump cannot: what grew. It compares the dumps path by path,
a path being the same place in both, and runs the leak suspects on the growth. A
collection that gained many objects is the surest sign of a leak.

What an object holds is what the walk from the roots reached through it first: a
spanning tree, not a dominator tree. Totals are exact, but an object held by
several others is counted under one of them.

### Options

| Option | |
|---|---|
| `--share <percent>` | the share of the heap a leak suspect holds at least (10) |
| `--json` | one JSON document on stdout instead of text |
| `--fresh` | parse again, even when a parse is kept beside the dump |
| `--no-keep` | keep no parse beside the dump |
| `--oops compressed\|wide` | the width of references, guessed from the heap's span (compressed under 32 GB) |
| `--save <file>` | write the profile there too, for mountain-graph or jfr-web |
| `--dir <path>` | look for the most recent dump there, not in the current directory |
| `--stats` | parse again and say what it cost: time, and peak memory |
| `--color auto\|always\|never` | colour on a terminal by default; `NO_COLOR` turns it off |

### Dumps, and the parse kept beside them

A dump is `.hprof` or `.hprof.gz`, as jmap and jcmd write them; a gzipped one is
unzipped to a temporary file first, as the parser seeks. Which kind a file is
comes from its content, not its name.

The first command on a dump keeps its parse beside it, as MAT keeps its indexes:
`heap.hprof` or `heap.hprof.gz` gets `heap.jheap.json`, a few MB whatever the
size of the dump. The next command reads that instead, while the dump has not
changed (its name, size and time), `--oops` is the same and jheap is the same
version. `--fresh` parses again anyway; `--no-keep` keeps nothing, for a dump on a
share that is not yours to write to. Where the directory cannot be written, jheap
says so and goes on.

The kept parse is a profile in [jfr-web](https://github.com/pcdv/jfr-web)'s format
(version 4), with a column of object counts and the parse's report: mountain-graph
opens it as it opens the dump, and so does every command here.

### Leaving the dump out

Without a dump, jheap takes the most recent `.hprof`, `.hprof.gz` or `.jheap.json`
of the current directory (not its subdirectories) and says which on stderr, so that
a wrong guess is seen, not silent:

```sh
cd /var/dumps
jheap leaks                  # jheap: using heap-0412.hprof.gz (3 minutes ago), the most recent in .
jheap diff                   # the two most recent dumps, the earlier one first
jheap trace Config
```

No option turns this on: it only applies where a dump would have been required,
so it cannot change what a command that names its dump does. The rules:

- A first argument that is a command (`leaks`, `diff`, …) is a command, not a
  file: a dump of that name is passed as `./leaks`.
- A `.jheap.json` counts as the dump it was kept for, so a dump and its parse
  are one candidate, not two. The newest by modification time wins.
- `--dir <path>` looks elsewhere than the current directory.
- Nothing found, or `diff` with fewer than two dumps, is exit code 2 and a line
  saying where it looked. It never falls back to a file in another directory.
- In `--json`, `dump.file` names the file used, so a script can tell what it got.
  A script that must not depend on what happens to be in a directory names its dump.

### For a script or an agent

`--json` prints one JSON document instead of the text: the same answer, bounded by
the number of rows. Its first two fields are `jheap`, the version of the format
(raised when a field changes meaning or goes, not when one is added), and
`command`; `dump` says what the text's header does. Sizes are bytes, shares are 0
to 1. Errors stay one line on stderr, `jheap: …`, whatever the format.

The exit code tells an answer from none:

| Code | Meaning |
|---|---|
| 0 | found something |
| 1 | it broke: a file it cannot read, a dump cut short |
| 2 | called wrong, or no dump to take |
| 3 | found nothing: no leak suspect, nothing grew, no class of the name traced |
| 4 | the name traced is several classes': they are listed, to name one more fully |

## Installing

A release has an archive per platform, with its checksum: Linux (x86-64 and
arm64, built against musl, so no libc to match on the server the dump is on),
macOS (both) and Windows (both). Unpack it and put `jheap` on the path.

```sh
# Windows, with scoop: the latest, and `scoop update jheap` follows
scoop install https://github.com/pcdv/jheap/releases/latest/download/jheap.json

# any platform with a Rust toolchain
cargo install --git https://github.com/pcdv/jheap jheap-cli
```

## Building

```sh
cargo install --path crates/jheap-cli    # jheap on the path
cargo test                               # the fixtures' tests
```

A release is a tag: `git tag v0.1.0 && git push origin v0.1.0` builds the six
archives and the scoop manifest (`.github/workflows/release.yml`), and refuses a
tag that is not the version in `Cargo.toml`.

Work in progress: a Rust rewrite of heap-landscape's parser and `hl` command, and
now the parser of mountain-graph's page. See [CLAUDE.md](CLAUDE.md) for the plan.
