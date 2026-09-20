# NC compiler

Rust front end, portable C output, and system-C-compiler executable builds.
The language specification is in [docs/design.md](docs/design.md). Implementation
is ongoing; see [the implementation ledger](docs/implementation.md).

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

`check` performs mandatory checks and emits non-fatal lint warnings, including
`capture`, which recommends passing function parameters instead of capturing
surrounding values. Suppress it for a file with
`// @ncc lint disable capture`. Imported files are also checked.

Release builds perform bounded, memoized constant evaluation of pure scalar
functions and loops, remove unreachable functions, and use the C compiler's
`-O3`. Debug builds preserve runtime evaluation and use `-O0 -g`. Integer
overflow remains an error, including when detected during constant evaluation.

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
