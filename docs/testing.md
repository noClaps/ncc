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

Successful conformance fixtures use debug and release execution, including
module exports, imported generic types and patterns, and external C calls.
Module visibility, cyclic imports, and external-declaration errors are checked
in both modes. Optimizer tests
compare emitted behavior in both modes and, where specified, verify that evaluated
functions disappear from generated C. The very large Fibonacci test is release
only to avoid exponential runtime work. C integration tests exercise both modes.
Negative tests distinguish rejected programs from runtime range/bounds failures.
Integer boundary coverage includes byte, signed and unsigned 64-bit arithmetic,
with runtime-dependent operands for overflow cases so release folding cannot
replace the runtime checks. Runtime-failure conformance helpers require successful
compilation, exit code 1, and the expected diagnostic in both modes.
Arithmetic right shifts cover every valid signed shift count against a wider
floor-division oracle, including negative odd values and signed boundaries;
constant-folding tests check the same rounding and boundary behavior.
Negative conformance cases run in both modes, including the operator/type matrix,
escaping futures, duplicate generics, expansion limits, and return paths containing
unreachable statements. Concurrency tests cover shared awaits and discarded workers.
String-conversion tests cover the `error: ` prefix, inactive error payloads,
recursive constituent restrictions, and explicit custom-type unwrapping (including
custom strings versus interpolation and chained custom types).

The tests are a regression suite, not a proof of specification completeness.
See [the remaining work](../TODO.md) for known gaps.
