# jheap

What holds the memory of a JVM, from its heap dump.

```sh
jheap heap.hprof                    # who holds the memory
jheap heap.hprof leaks              # what could grow
jheap diff before.hprof after.hprof # what grew between two dumps
```

A single binary, no runtime to install: copy it next to the dump. It also runs in
the browser, as WebAssembly, behind heap-landscape's drawing of the heap.

## Usage

```
jheap [<dump>] [<command> [<args>]] [options]
jheap diff [<earlier> <later>] [options]
```

The dump comes first, then what to ask of it: the command reads as a sentence,
"on this dump, find the leaks". No command means `peaks`, the one to start with.
`diff` is the exception, as it takes two dumps: it comes first, and the dumps
follow, the earlier one first.

| Command | Says |
|---|---|
| `peaks [<n>]` | who holds the memory: the `n` heaviest peaks (ten) and their holders |
| `leaks` | what in the dump could grow (`--share 5` looks lower than 10%) |
| `diff <earlier> <later>` | what grew between two dumps, and which of it looks like a leak |
| `trace <class>` | what holds a class and what it holds; the full name, the simple one, or a part |

```sh
jheap heap.hprof peaks 20
jheap heap.hprof trace store.Store
jheap diff before.hprof after.hprof
```

A dump is `.hprof` or `.hprof.gz`, as jmap and jcmd write them. The parse kept beside
it (`heap.jheap.json`) is read in its place, and so is a profile saved with
`--save`: which one it is comes from the content, not the name.

### Leaving the dump out

Without a dump, jheap takes the most recent `.hprof`, `.hprof.gz` or `.jheap.json`
of the current directory (not its subdirectories) and says which on stderr, so that
a wrong guess is seen, not silent:

```sh
cd /var/dumps
jheap leaks                  # jheap: using heap-0412.hprof.gz (3 minutes ago)
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
- In `--json`, `dump` names the file used, so a script can tell what it got.
  A script that must not depend on what happens to be in a directory names its dump.

### Output

Colour on a terminal, plain text in a pipe. `--json` prints one JSON document
instead, versioned. The exit codes are the same for every command: 0 found,
1 broke, 2 called wrong, 3 found nothing, 4 the traced name matches several classes.

Work in progress: a Rust rewrite of heap-landscape's parser and `hl` command. See
[CLAUDE.md](CLAUDE.md) for the plan.
