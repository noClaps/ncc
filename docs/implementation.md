# Implementation audit

`docs/design.md` is the language specification. Passing the initial smoke tests
does not imply specification completeness. This ledger records the audit and
must stay explicit about features still under construction.

## Implemented and covered by regression tests

- Checked scalar arithmetic, exponentiation, contextual literals, nominal casts,
  short-circuit evaluation, shadowing, labelled loops, and return-path checks.
- Arrays, maps, tuples/destructuring, structs, recursive enums, pattern bindings,
  optional values, error unions, catch/try/throw, and composite formatting.
- Top-level and exported tuple bindings, discarded bindings, and partial tuple
  destructuring into grouped values; initializers are evaluated once and copied.
- Recursive structs through arrays, including mutual recursion; infinite inline
  layouts and cyclic aliases are rejected. Composite copy/equality/format helpers
  are generated once per type and operation.
- Explicit generic function/struct specialization and contextual generic enum
  constructors; nested generic types.
- First-class functions, nested anonymous functions, returned closures, and
  immutable by-value captures. Mutex captures retain the shared protected value.
- Background futures, repeated awaits, mutex snapshots, and automatic lock
  release on return, throw, and labelled breaks.
- UTF-8 grapheme literals, interpolation, Unicode-aware string length/indexing,
  replacement and iteration, and string-to-character/byte-array conversion.
- Source imports, exports, cycle diagnostics, and external C functions.
- Validated external C signatures with stable argument/result aliases, including
  composite arguments and error-union results; see `docs/c-abi.md`.
- Runtime argument/environment builtins and compile-time target introspection;
  explicit target selection and `NC_TARGET` support for `macos-arm64`.
- CLI artifact isolation, explicit formats, argument validation, capture lint
  (including imported files and suppression), stdio LSP diagnostics/formatting,
  and bounded memoized type-aware constant evaluation in release builds, covering
  numeric widths, composites, optionals, named callbacks, and nominal types.
- Tree-sitter syntax grammar, corpus tests and shared highlight queries; a
  WebAssembly-built Zed extension with local grammar packaging and LSP launch.
- Incremental UTF-16 LSP edits, local definitions, documentation hover,
  completions and open-document symbols. Unsaved imported buffers participate in
  checking, and changes/closure trigger fresh diagnostics in open importers.
  Imported exports support definition/hover lookup. Parser errors and semantic
  errors in variable/function declarations retain their original file locations.

## Known remaining work

- Complete remaining specified casts and operators. Numeric byte-array casts
  now use fixed little-endian encoding (IEEE-754 bits for floats).
- Complete generic inference in nested contextual expressions.
- Complete pattern/label coverage and restrictions on escaping futures.
- Audit value-copy and evaluation-order behavior across all composite operations.
- Finish external C ABI coverage and source-aware diagnostics
  for all semantic errors (many still report the start of the file).
- Extend compile-time evaluation to remaining operations and broaden optimisation
  within function bodies. The evaluation fuel/depth limits intentionally retain
  runtime code for work that cannot safely be completed at compile time.
- Improve multiline strings, embedded-NUL handling, canonical formatting, and
  editor indexing of struct members and pattern bindings.
- Increase negative, differential, concurrency, and full-specification tests.

The Unicode crate is now a test oracle only. The compiler core can be built with
zero production dependencies by disabling the optional LSP feature.
Standard-library modules are intentionally out of scope: the language author
will implement them separately. External implementations target C only; an Etch
backend is not part of this toolchain.

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
