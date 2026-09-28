# Verification

Run from the repository root:

```sh
cargo test
cargo test --no-default-features
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Use `--offline` on Cargo commands when dependencies are already cached. The core
compiler has no production dependencies. Unicode segmentation is a test oracle only.

## Coverage map

| Area | Regression tests |
| --- | --- |
| Types, bindings, operators, patterns, loops, errors, optionals, generics, imports, closures, concurrency, value semantics | `tests/conformance.rs` |
| Compile-time folding, numeric widths, cast table, closures, active error payloads, evaluation limits, sampled float formatting, evaluation order, Unicode/NUL strings | `tests/optimizer.rs` |
| Binary embedding, lexical/tuple/captured paths, empty files, symlink and runtime-dependency rejection | `tests/embed.rs` |
| C ABI declarations, shared implementation files, inactive optional/error payloads | `tests/externs.rs` |
| CLI help/options, removed-command rejection, release mode, targets, runtime process state | `tests/cli.rs` |
| Build formats, artifact isolation, required C headers, basic diagnostics | `tests/compiler.rs` |
| Escapes, multiline literals, graphemes, expression ranges, imported/specialized/constant-evaluation diagnostics | `tests/frontend.rs` |
| Grapheme boundaries against an independent oracle | Unit tests in `src/unicode.rs` |

Successful conformance fixtures use debug and release execution. Optimizer tests
compare emitted behavior in both modes and, where specified, verify that evaluated
functions disappear from generated C. The very large Fibonacci test is release
only to avoid exponential runtime work. C integration tests exercise both modes.
Negative tests distinguish rejected programs from runtime range/bounds failures.
Negative conformance cases run in both modes, including the operator/type matrix,
escaping futures, duplicate generics, expansion limits, and return paths containing
unreachable statements. Concurrency tests cover shared awaits and discarded workers.

The tests are a regression suite, not a proof of specification completeness.
See [the remaining work](../TODO.md) for known gaps.
