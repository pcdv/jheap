# Fixtures

Small heap dumps, and what heap-landscape's JS implementation makes of them.

| Directory | What | Binding |
|---|---|---|
| `dumps/` | the dumps, built by `generate/record.js` | |
| `expected/` | the parse of each dump: every path from a root with its bytes and objects, and the report's counts (times and file name left out) | **yes**: jheap's parser gives the same |
| `baseline/` | what `hl` printed for each case, with its exit code and stderr in `cases.json` | no: only to compare with |

## The dumps

- `static-holder`: a class holding an object in a static field, rooted as a
  system class. 4-byte ids.
- `inherited-fields`: `B extends A`; an instance holds B's fields, then A's.
  4-byte ids.
- `server-before`, `server-after`: a server as a 64-bit JVM dumps it, with 50 then
  400 sessions in a registry; a cache over a HashMap with chained nodes; a JDK class
  holding a buffer in a static; a chain of 40 links from an unknown root, deeper
  than the walk's limit of 30; objects nothing reaches; a reference to an object
  defined further on, and one to an object never defined. `server-after` is also
  parsed with wide references (`server-after.wide.parse.json`).
- `truncated`: `server-after` cut at 60%, as a full disk leaves a dump.

## Regenerating

```sh
node fixtures/generate/record.js
```

Needs Node 21+ and heap-landscape with its JS parser and `hl`, which it no longer
has: a checkout of its commit `75ac409`, the last before they moved here, named
by `HEAP_LANDSCAPE`:

```sh
git -C ../heap-landscape worktree add ../heap-landscape-js 75ac409
HEAP_LANDSCAPE=../heap-landscape-js node fixtures/generate/record.js
```

The expected parses are jheap's to keep now: a deliberate change to the parse
edits them, and says why in its commit. `generate/hprof-builder.js` is
heap-landscape's `test/hprof/hprof-builder.js`, copied.
