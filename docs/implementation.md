# Implementation audit

`docs/design.md` is the language specification. Passing the initial smoke tests
does not imply specification completeness. This ledger records the audit and
must stay explicit about features still under construction.

## Implemented and covered by regression tests

- Checked scalar arithmetic, exponentiation, contextual literals, nominal casts,
  short-circuit evaluation, shadowing, labelled loops, and return-path checks.
- Arrays, maps, tuples/destructuring, structs, recursive enums, pattern bindings,
  optional values, error unions, catch/try/throw, and composite formatting.
- Explicit generic function/struct specialization and contextual generic enum
  constructors; nested generic types.
- First-class functions, nested anonymous functions, returned closures, and
  immutable by-value captures. Mutex captures retain the shared protected value.
- Background futures, repeated awaits, mutex snapshots, and automatic lock
  release on return, throw, and labelled breaks.
- UTF-8 grapheme literals, interpolation, Unicode-aware string length/indexing,
  replacement and iteration, and string-to-character/byte-array conversion.
- Source imports, exports, cycle diagnostics, and external C functions.
- CLI artifact isolation, explicit formats, argument validation, capture lint
  (including imported files and suppression), stdio LSP diagnostics/formatting,
  and bounded memoized scalar constant evaluation in release builds.

## Known remaining work

- Complete remaining specified casts and operators. Numeric byte-array casts
  now use fixed little-endian encoding (IEEE-754 bits for floats).
- Complete generic inference in nested contextual expressions. Validate recursive
  type layouts without backend recursion failures.
- Complete pattern/label coverage and restrictions on escaping futures.
- Audit value-copy and evaluation-order behavior across all composite operations.
- Finish standard modules, external ABI coverage, and source-aware diagnostics
  for all semantic errors (many still report the start of the file).
- Extend compile-time evaluation beyond scalar values and broaden optimisation
  within function bodies. The evaluation fuel/depth limits intentionally retain
  runtime code for work that cannot safely be completed at compile time.
- Improve multiline strings, embedded-NUL handling, canonical formatting, and
  editor features beyond diagnostics and formatting.
- Increase negative, differential, concurrency, and full-specification tests.

The Unicode crate is now a test oracle only. The compiler core can be built with
zero production dependencies by disabling the optional LSP feature.

## Self-hosting handoff

`examples/bootstrap/compiler.nc` implements a real subset compiler in NC:
stdin input, recursive enum AST, precedence parsing, symbol-table maps, shadowing,
diagnostics, and C output. Its integration test compiles the compiler in both
debug and release modes, feeds it source, compiles the resulting C, and runs it.
This is the verified handoff baseline, not a claim of full language completeness.

## Initial audit findings

The original emitter substituted `0` for unsupported expressions, generated
zero-iteration `for` loops, ignored labels and imports, and lowered `**` to XOR.
The formatter only trimmed trailing whitespace and the LSP was a stub. These
are missing implementations, not supported language features.
