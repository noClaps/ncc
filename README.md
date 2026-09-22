# NC compiler

Rust front end, portable C output, and system-C-compiler executable builds.
The language specification is in [docs/design.md](docs/design.md). Implementation
is ongoing; see [the implementation ledger](docs/implementation.md).

[examples/bootstrap/compiler.nc](examples/bootstrap/compiler.nc) is a working
NC-written subset compiler and a starting point for the self-hosted compiler.

```sh
cargo build --release
target/release/ncc run example.nc
target/release/ncc build example.nc --release -o example.c
target/release/ncc check example.nc
target/release/ncc build --help
```

`run` keeps build artifacts in a private temporary directory and removes them on
exit. `build` leaves only the requested output. Explicit `--format C|obj|exe`
takes precedence over the output filename's extension.

`ncc --targets` lists supported targets (currently `macos-arm64`). Select one
with `ncc build --target macos-arm64` or `NC_TARGET`; an explicit option takes
precedence. Native builds require the macOS C toolchain. `@target()` is a
compile-time `(OS, architecture)` tuple. `@args()` and `@env()` read the running
program's arguments and environment, never the compiler's. Pass arguments with
`ncc run program.nc -- one two --help`.

`check` performs mandatory checks and emits non-fatal lint warnings, including
`capture`, which recommends passing function parameters instead of capturing
surrounding values. Suppress it for a file with
`// @ncc lint disable capture`. Imported files are also checked.

Release builds perform bounded, memoized, type-aware constant evaluation of pure
functions and loops, remove unreachable functions, and use the C compiler's
`-O3`. Debug builds preserve runtime evaluation and use `-O0 -g`. Integer
overflow remains an error, including when detected during constant evaluation.
Numeric casts to `byte[]` produce eight little-endian bytes; floats use their
64-bit IEEE-754 representation.

Constant evaluation supports signed/unsigned integers, bytes, floats, booleans,
characters, strings, arrays, tuples, maps, structs, enums, optionals, successful
error unions, nominal types, and named function callbacks. Arithmetic uses each
type's range. Effects, unsupported operations, and exhausted evaluation budgets
remain runtime code; futures and external calls are never executed by the
optimiser.

## Editor tooling

`ncc lsp` provides versioned incremental synchronization, diagnostics, formatting,
local definition navigation, documentation hover, completion, and symbols from
open documents. Imports are checked against unsaved buffers, and dependents are
rechecked when those buffers change or close. Definition and documentation lookup
also resolve exported import members, including unsaved files. Arbitrary struct
members and every pattern binding are not yet indexed.

The [Tree-sitter grammar](tree-sitter-nc/README.md) includes generated C and a
syntax corpus. The [Zed extension](editors/zed/README.md) adds highlighting,
indentation, brackets, outline navigation, and LSP integration. Its local
packaging helper uses this repository's committed grammar without publishing.

## Dependencies and self-hosting

The compiler core needs only Rust's standard library. Build it with
`cargo build --release --no-default-features` for **zero production crate
dependencies**. The optional, default-enabled `lsp` feature uses `serde_json`
for the editor protocol. The Unicode reference package is test-only.

Generated programs use C library facilities and, only for futures/mutexes,
POSIX threads. Unicode grapheme segmentation uses checked-in Unicode 16 data
and a small runtime, without ICU or another external Unicode library.
`UNICODE-LICENSE` contains the data license. `scripts/unicode-tables.mjs` is an
optional regeneration tool, not part of building or running the compiler.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
