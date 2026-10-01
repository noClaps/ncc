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

Release analysis follows safely known state through retained test blocks and
assertion-expression effects, including repeated named calls sharing global
mutations. Known-true assertions permit continued analysis; false or unknown
assertions stop it without converting assertion failures into compile-time errors.
Runtime assertions, output, and mutations remain intact. Regressions cover
cross-test state, lexical shadowing, original diagnostic locations, and runtime
fallback for process inputs, native calls, and futures.

## Coverage map

| Area                                                                                                                                                                  | Regression tests               |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ |
| Types, bindings, operators, patterns, loops, errors, optionals, generics, imports, closures, concurrency, value semantics                                             | `tests/conformance.rs`         |
| Compile-time folding, numeric widths, cast table, closures, active error payloads, evaluation limits, sampled float formatting, evaluation order, Unicode/NUL strings | `tests/optimizer.rs`           |
| Binary embedding, lexical/tuple/captured paths, empty files, symlink and runtime-dependency rejection                                                                 | `tests/embed.rs`               |
| C ABI declarations, shared implementation files, inactive optional/error payloads                                                                                     | `tests/externs.rs`             |
| CLI help/options, removed-command rejection, release mode, targets, runtime process state                                                                             | `tests/cli.rs`                 |
| Non-fatal async race warnings, mutex-safe cases, indirect/recursive calls, imported specialization locations, build/run success                                       | `tests/warnings.rs`            |
| Build formats, artifact isolation, required C headers, basic diagnostics                                                                                              | `tests/compiler.rs`            |
| Escapes, multiline literals, graphemes, expression ranges, imported/specialized/constant-evaluation diagnostics                                                       | `tests/frontend.rs`            |
| Grapheme boundaries against an independent oracle                                                                                                                     | Unit tests in `src/unicode.rs` |

Test-slicing regressions invoke `ncc test` directly in debug and release modes,
without wrapping outside statements in synthetic test roots. Summaries exclude
executable effects after direct jumps through the same or nested plain lexical
blocks and in syntactically proven dead Boolean branches, false loops, and
short-circuit operands. References
needed to type-check dead syntax remain. Tests protect ordered multi-pattern arms,
unknown pattern effects, name comparisons, jump-operand mutations, captured writes,
await dependencies, qualification errors, and scope restoration. Boundary tests
preserve reached effects after loops, locks, labeled conditionals, and value
expressions that consume jumps. Reachable
dynamic/native calls, callable creation versus invocation, and more complex
control flow remain conservative.

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

Nested optional/error fallback tests cover nonlocal loop jumps, early returns,
and `try` propagation while retaining shared mutations. An abandoned assignment
must not evaluate its target. Folding tests also verify restoration of shadowed
bindings on fallback jumps; negative cases retain rejection of missing values and
invalid jump targets.

Function side-effect coverage includes shared mutable scalar and container
bindings, named callbacks, nested and returned closures, write-only captures,
shadowing, tuple bindings, and independent factory invocations. Immutable closure
captures retain compile-time folding coverage. Shared mutable captures are tested
for folding, independent factory calls, repeated mutations, loop scopes, errors,
container callbacks, and computed embed paths. Persistent outer state and runtime
effects retain their runtime fallback. Closures created inside lock
scopes cannot inherit write permission; returned closures that acquire their own
locks are tested sequentially and concurrently. Unlocked mutex reads are rejected
for scalar and container access, interpolation, comparisons and patterns; generic
imported read errors retain their original expression location.

Pattern-only captures are checked across scalar, tuple, array, struct, and enum
patterns, including returned closures, mutable updates, comparison order,
constant folding, and shared-read race warnings.
Direct map subjects are rejected, including generic specializations; bare `if`
branches retain map equality and membership comparisons.

Data-race warning tests compile the racing examples without asserting a particular
result. CLI execution tests use an awaited single worker for deterministic output.
Warnings are collected before optimization and checked in both modes.

The tests are a regression suite, not a proof of specification completeness.
See [the remaining work](../TODO.md) for known gaps.
