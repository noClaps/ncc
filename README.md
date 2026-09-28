The mostly vibe-coded compiler for the [NC programming language](https://nc.klado.dev). This version of the compiler was made primarily by GPT-6 Astra, which is the reason why it's not on Codeberg where I put all my other projects. The purpose of this version of the compiler is to give me a working implementation of NC, at least enough that I can later rewrite the compiler in NC itself and archive this repo.

There may be some differences between the specification on the NC website linked above and the implementation in this repo. In such cases, you can look at [`docs/design.md`](./docs/design.md) as it is the reference design for this compiler.

This code is unlicensed for now as I don't really know the legal implications of using LLMs to write code. I don't want to end up using the wrong license. The bootstrapped compiler will be properly licensed, but for now you'll just have to deal with this not being properly open source. For all intents and purposes though, it is open source and you can fork and use it however you wish, and I grant you explicit permission to do so.

Everything below the line is the original LLM-written README. I cannot guarantee that the information below is up to date or correct. If something isn't working and you need help, let me know and I'll try to sort it out for you. Chances are, a lot of things are broken, and I haven't tested the vast majority of them yet.

---

# NC compiler

Rust front end, portable C output, and system-C-compiler executable builds.
The language specification is in [docs/design.md](docs/design.md).
Known remaining work is tracked in [TODO.md](TODO.md).

```sh
make build
target/release/ncc run example.nc
target/release/ncc build example.nc --release -o example.c
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

Release builds perform bounded, memoized, type-aware constant evaluation of pure
functions and loops, remove unreachable functions, and use the C compiler's
`-O3`. Debug builds preserve runtime evaluation and use `-O0 -g`. Integer
overflow remains an error, including when detected during constant evaluation.
Numeric casts to `byte[]` produce eight little-endian bytes; floats use their
64-bit IEEE-754 representation.

Constant evaluation supports signed/unsigned integers, bytes, floats, booleans,
characters, strings, arrays, tuples, maps, structs, enums, optionals, successful
and failed error unions, nominal types, and by-value closures. Arithmetic uses each
type's range. Effects, unsupported operations, and exhausted evaluation budgets
remain runtime code; futures and external calls are never executed by the
optimiser.

## Dependencies and self-hosting

The compiler needs only Rust's standard library and has no production crate
dependencies. The Unicode reference package is test-only.

Generated programs use C library facilities and, only for futures/mutexes,
POSIX threads. Unicode grapheme segmentation uses checked-in Unicode 16 data
and a small runtime, without ICU or another external Unicode library.
`UNICODE-LICENSE` contains the data license. `scripts/unicode-tables.mjs` is an
optional regeneration tool, not part of building or running the compiler.

```sh
make test
```
