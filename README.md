# jheap

What holds the memory of a JVM, from its heap dump.

```sh
jheap heap.hprof          # who holds the memory
jheap leaks heap.hprof    # what could grow
jheap diff before.hprof after.hprof
```

A single binary, no runtime to install: copy it next to the dump. It also runs in
the browser, as WebAssembly, behind heap-landscape's drawing of the heap.

Work in progress: a Rust rewrite of heap-landscape's parser and `hl` command. See
[CLAUDE.md](CLAUDE.md) for the plan.
