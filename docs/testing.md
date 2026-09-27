# Verification

Run from the repository root:

```sh
cargo test
cargo test --no-default-features
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Use `--offline` on Cargo commands when dependencies are already cached. The core
compiler has no production dependencies with default features disabled. The LSP
uses the optional JSON dependency; Unicode segmentation is a test oracle only.

## Coverage map

| Area | Regression tests |
| --- | --- |
| Types, bindings, operators, patterns, loops, errors, optionals, generics, imports, closures, concurrency, value semantics | `tests/conformance.rs` |
| Compile-time folding, numeric widths, every documented cast-table row, closures, error propagation, evaluation order, Unicode/NUL strings | `tests/optimizer.rs` |
| Binary embedding, path resolution, empty files, symlink rejection | `tests/embed.rs` |
| C ABI declarations, shared implementation files, inactive optional/error payloads | `tests/externs.rs` |
| CLI help/options, release mode, targets, runtime process state, linting | `tests/cli.rs` |
| Build formats, artifact isolation, required C headers, basic diagnostics | `tests/compiler.rs` |
| Escapes, formatting preservation/idempotence, LSP protocol, unsaved imports | `tests/tooling.rs` |
| Editor scope resolution, rename safety, member/builtin completion, incomplete edits | Unit tests in `src/lsp_index.rs` and `src/lsp.rs` |
| Grapheme boundaries against an independent oracle | Unit tests in `src/unicode.rs` |

Successful conformance fixtures use debug and release execution. Optimizer tests
compare emitted behavior in both modes and, where specified, verify that evaluated
functions disappear from generated C. The very large Fibonacci test is release
only to avoid exponential runtime work. C integration tests exercise both modes.
Negative tests distinguish rejected programs from runtime range/bounds failures.

The tests are a regression suite, not a proof of specification completeness.
See [the remaining work](../TODO.md), including
LSP features not yet at parity with Gleam. The standalone language server is
tested independently of editor plugins.
