# Implementation audit

`docs/design.md` is the language specification. Passing the initial smoke tests
does not imply specification completeness. This ledger records the audit and
must stay explicit about features still under construction.

## Implemented and covered by regression tests

- Checked scalar arithmetic, exponentiation, contextual literals, nominal casts,
  short-circuit evaluation, shadowing, labelled loops, and return-path checks.
- Every row of the specified cast table has a debug/release differential test.
  Numeric byte-array casts use fixed little-endian encoding (IEEE-754 for floats).
  Nominal composite casts preserve representation, contextual literals and copies.
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
- Equality, membership and map keys reject function/future values, including
  recursively inside containers, nominal types and recursive composites.
  Printing, interpolation and string casts reject those values during checking.
- Enum payload constructors are first-class callables, including async calls.
- Background futures, repeated awaits, mutex snapshots, and automatic lock
  release on return, throw, and labelled breaks.
- Async runtime builtins snapshot their arguments before spawning. Printing
  evaluates all arguments left-to-right before emitting output.
- UTF-8 grapheme literals, interpolation, Unicode-aware string length/indexing,
  replacement and iteration, and string-to-character/byte-array conversion.
- Counted UTF-8 strings preserve embedded NUL bytes. All documented escapes,
  Unicode scalar validation and multiline literal preservation are covered.
- Compile-time `@embed` with computed paths, module-relative resolution, binary
  and empty files, unreadable-file diagnostics and symlink rejection.
  Paths can use lexical immutable constants, interpolation, pure functions and
  nested embeddings; runtime-dependent paths fail at their original call site.
- Source imports, exports, cycle diagnostics, and external C functions.
- Validated external C signatures with stable argument/result aliases, including
  composite arguments and error-union results; see `docs/c-abi.md`.
- Shared external C sources are included once after all ABI declarations;
  conflicting symbol signatures are rejected. Optional/error copies and equality
  inspect only active payloads, including values returned by C implementations.
- Runtime argument/environment builtins and compile-time target introspection;
  explicit target selection and `NC_TARGET` support for `macos-arm64`.
- CLI artifact isolation, explicit formats, argument validation, capture lint
  (including imported files and suppression), stdio LSP diagnostics/formatting,
  and bounded memoized type-aware constant evaluation in release builds, covering
  numeric widths, composites, optionals, named callbacks, and nominal types.
- Pure calls through by-value closures can be evaluated with captured environments
  included in memoization. Effectful/escaping closures retain their source code.
  Value-producing branches propagate assignments to surrounding local variables.
- Pure thrown errors, `catch`/`try` propagation and early returns from value
  branches can fold; impure error-producing calls retain their runtime effects.
- Independent Tree-sitter syntax grammar, corpus tests and highlight queries.
  Editor plugins and configuration are intentionally not provided.
- Incremental UTF-16 LSP edits, local definitions, documentation hover,
  completions and open-document symbols. Unsaved imported buffers participate in
  checking, and changes/closure trigger fresh diagnostics in open importers.
  Imported exports support definition/hover/completion lookup. Parser errors and semantic
  errors in variable/function declarations retain their original file locations.
- Local references and collision-checked local rename, type definitions, struct
  member navigation/completion, pattern/catch bindings, signature help and folds.
  Builtins have documentation/signatures and completion replaces the entire sigil.
  Editor-only parser recovery retains useful indexing through incomplete input.
- Workspace references and validated export renaming include unopened source
  files and unsaved overlays. Imported signatures follow the active nested call.
- Private module declarations support rename with binding-identity verification.
  Enum variants support local navigation, documentation, completion and signatures.
  Parsed declaration types drive member navigation after calls, indexing,
  destructuring and nested generic fields, plus signatures for typed callbacks.
  Code actions convert integer bases and comment kinds, and offer checked fixes
  for unused call results. Edits preserve Unicode positions and literal contents.
- Formatting uses a single token pass for indentation and preserves literal
  contents and comments; tests check token equivalence and idempotence.

## Known remaining work

- Continue auditing nested generic contexts, pattern/label restrictions, escaping
  futures and negative operator/type combinations beyond the current fixtures.
- Audit value-copy and evaluation-order behavior across all composite operations.
- Finish external C ABI coverage and source-aware diagnostics
  for all semantic errors (many still report the start of the file).
- Extend compile-time evaluation to remaining operations and broaden optimisation
  within function bodies. The evaluation fuel/depth limits intentionally retain
  runtime code for work that cannot safely be completed at compile time.
- Full canonical spacing/layout formatting, rather than indentation only.
- More inferred member types, code actions, workspace indexing performance and
  resilient indexing through every invalid edit.
  LSP usability has improved, but parity with Gleam's LSP is not yet achieved.
- Further Tree-sitter error-recovery and malformed-input cases.
- Increase negative, differential, concurrency, and full-specification tests.

The Unicode crate is now a test oracle only. The compiler core can be built with
zero production dependencies by disabling the optional LSP feature.
Standard-library modules are intentionally out of scope: the language author
will implement them separately. External implementations target C only; an Etch
backend is not part of this toolchain.

## Initial audit findings

The original emitter substituted `0` for unsupported expressions, generated
zero-iteration `for` loops, ignored labels and imports, and lowered `**` to XOR.
The formatter only trimmed trailing whitespace and the LSP was a stub. These
are missing implementations, not supported language features.
