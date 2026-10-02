The mostly vibe-coded compiler for the [NC programming language](https://nc.klado.dev). This version of the compiler was made primarily by GPT-6 Astra, which is the reason why it's not on Codeberg where I put all my other projects. The purpose of this version of the compiler is to give me a working implementation of NC, at least enough that I can later rewrite the compiler in NC itself and archive this repo.

There may be some differences between the specification on the NC website linked above and the implementation in this repo. In such cases, you can look at [`docs/design.md`](./docs/design.md) as it is the reference design for this compiler.

This code is unlicensed for now as I don't really know the legal implications of using LLMs to write code. I don't want to end up using the wrong license. The bootstrapped compiler will be properly licensed, but for now you'll just have to deal with this not being properly open source. For all intents and purposes though, it is open source and you can fork and use it however you wish, and I grant you explicit permission to do so.

Everything below the line is the original LLM-written README. I cannot guarantee that the information below is up to date or correct. If something isn't working and you need help, let me know and I'll try to sort it out for you. Chances are, a lot of things are broken, and I haven't tested the vast majority of them yet.

---

# NC compiler

Rust front end, portable C output, and system-C-compiler executable builds.
The language specification is in [docs/design.md](docs/design.md).
Known remaining work is tracked in [TODO.md](TODO.md).

A standalone Tree-sitter grammar, generated parser, syntax corpus, and highlighting
queries are in [tree-sitter-nc](tree-sitter-nc/README.md). Run `make grammar-test`
to validate it independently of the Rust compiler. Tree-sitter CLI 0.27.0 or newer,
Python 3, and a C compiler are required; no Node/npm or compiler dependency is added.

```sh
make build
target/release/ncc run example.nc
target/release/ncc test example.nc
target/release/ncc build example.nc --release -o example.c
target/release/ncc build --help
```

`run` and `test` keep build artifacts in a private temporary directory and remove
them on exit. `build` leaves only the requested output. Test blocks are ignored by
`build` and `run`; `ncc test` executes them, including imported tests, with only
their required outside dependencies. Those dependencies include global initializers
and prior mutations through assignments, functions, closures, and async work.
Unrelated top-level prints and declarations are discarded. Retained code keeps
its original order. Dependency analysis is conservative for dynamic/native calls
and keeps complete selected statements and initializers, including their effects.
Explicit `--format C|obj|exe` takes precedence over the output filename's extension.

`ncc --targets` lists supported targets (currently `macos-arm64`). Select one
with `ncc build --target macos-arm64` or `NC_TARGET`; an explicit option takes
precedence. Native builds require the macOS C toolchain. `@target()` is a
compile-time `(OS, architecture)` tuple. `@args()` and `@env()` read the running
program's arguments and environment, never the compiler's. Pass arguments with
`ncc run program.nc -- one two --help`.

Release builds perform proof-gated, memoized, type-aware constant evaluation of pure
functions and loops, remove unreachable functions, and use the C compiler's
`-O3`. Debug builds preserve runtime evaluation and use `-O0 -g`. Integer
overflow remains an error, including when detected during constant evaluation.
Numeric casts to `byte[]` produce eight little-endian bytes; floats use their
64-bit IEEE-754 representation.

Constant evaluation supports signed/unsigned integers, bytes, floats, booleans,
characters, strings, arrays, tuples, maps, structs, enums, optionals, successful
and failed error unions, nominal types, and closures. Mutable captures share
bindings within an evaluation, including returned closures; stateful calls are
not memoized. Arithmetic uses each type's range. Unknown state and unsupported
effects remain runtime code; futures and external calls are never executed by the
optimiser. Loops and recursive cycles must have termination certificates before
execution, with no artificial step or depth budgets for certified computation.
Counted integer loops require invariant bounds, monotonic progress on every path
that continues the loop, and no overflow before their exit. Progress can come from
conditional updates, known shared-state helpers, or pure scalar helper returns.
Known condition wrappers may perform unrelated effects before their comparison;
all checks, including the final false check, retain their original effects and order.
Proof-only helper expansion distinguishes shared cells from copied captures and
shadowed bindings, and rejects resets, changing bounds, and callable replacements.
Finite `for` traversals use their original snapshot. Direct self-recursion supports
covering integer base cases and decreasing
arguments; exact integral floats through `2^53` also qualify. Certified recursion
uses heap continuations. Other patterns, including mutual recursion, condition-side
counter changes, and helpers with loops or early returns, conservatively stay at
runtime rather than being tried with fuel.

When an entire program is known, release mode records output argument snapshots
without printing during compilation, then emits only constant-string output calls.
Known calls, shared-global mutations, and closures can disappear with their unused
storage. Any unknown operation or failure rolls back the entire transaction.
Otherwise, release mode can precompute a call-free initial region into final global
initializers, stopping at calls, closure creation, effects, or unknown state. This
fallback retains global storage for later mutations. Test-mode analysis keeps
runtime output and assertions while following known state and successful assertions.
Generated C binding names retain their NC names, such as `nc_var_buf_1`; unique
suffixes distinguish shadowed bindings, and closure fields use `nc_capture_` names.

Structurally infinite loops are left for runtime execution, while independent
expressions inside their bodies can still fold. The compiler emits non-fatal
infinite-loop and unreachable-code warnings in both modes. This conservative
control-flow analysis does not solve general program termination.

Async calls that may access ordinary shared mutable state produce a non-fatal
potential-data-race warning. Such programs remain valid. The analysis follows
named functions, immutable callback aliases and captures conservatively; opaque
external or unresolved indirect calls may also warn. It does not prove that tasks
overlap or that a race will occur. Mutex values require an explicit `lock` scope
for both reads and writes, including inside closures.
Warnings preserve original source locations and are emitted in both build modes.
Library callers can use `compile_source_with_diagnostics` to receive generated C
and warnings separately; existing compilation helpers continue to return C only.

## Dependencies and self-hosting

The compiler needs only Rust's standard library and has no production crate
dependencies. The Unicode reference package is test-only.

Generated programs use C library facilities and, only for futures/mutexes,
POSIX threads. Unicode grapheme segmentation uses checked-in Unicode 16 data
and a small runtime, without ICU or another external Unicode library. These are
emitted only when needed by character-aware operations; printing composite values
and converting strings to `byte[]` do not themselves require the tables. Actual
string conversions retain character boundaries, while internal output buffers
only assemble bytes.
`UNICODE-LICENSE` contains the data license. `scripts/unicode-tables.mjs` is an
optional regeneration tool, not part of building or running the compiler.

Strings retain their `char[]` element boundaries after indexed replacement,
concatenation, interpolation, and string conversion. Printing and `byte[]`
conversion flatten their UTF-8 bytes, but equality, inclusion, length, and
indexing operate on the retained character elements.

The generated C `nc_string` representation has `bytes`, `data`, `len`, and
`ends` fields. `ends` optionally stores cumulative byte-end offsets for `len`
character elements; a null `ends` means raw UTF-8 whose initial elements are
Unicode graphemes. C externs returning newly constructed raw strings can use
`NC_STRING` or zero-initialize the boundary fields. Externs passing existing NC
strings through should preserve all four fields. Rebuild native code that depends
on the generated string layout.

```sh
make test
```
