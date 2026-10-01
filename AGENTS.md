# Working on NC

## Scope and priorities

- Implement the full language and compiler toolchain described in `docs/design.md`.
  Read the relevant specification and recent documentation changes before editing.
- Follow the priorities in `TODO.md`: comprehensive language and edge-case tests,
  undefined-behavior clarification and elimination, then compiler simplification.
  Tree-sitter, LSP, and formatter work follows the compiler work.
- A task being listed in `TODO.md` is not an instruction to start it during a
  documentation-only request. Respect the user's current scope and stop requests.
- Maintain the root `TODO.md` as a nested checklist. Check off each task when it
  is completed; before committing, remove checked items so the committed list
  contains remaining work only. Narrow partially completed items and add new
  checklist items whenever new tasks or gaps are discovered. Do not treat passing
  tests as proof that the specification is fully implemented.
- The current CLI provides `build`, `run`, and `test`. Test blocks are checked and
  executed only in test mode; normal builds/runs ignore them after parsing,
  including imported tests. Test mode retains only tests and their transitive
  outside dependencies, including prior mutations through assignments, functions,
  closures, and async synchronization; unrelated top-level output is discarded.
  Preserve order and effects of retained dependencies. A Tree-sitter grammar, LSP, and
  formatter are planned again; the earlier prohibition on those tools is
  superseded. Their CLI integration is not yet decided. Do not restore the
  removed `check` command or linter, editor plugins, extensions, or editor setup
  files without a new request.
- Do not implement a standard library. External implementations are C-only for
  now; an Etch backend is out of scope.

## Implementation constraints

- Continue with Rust. Keep dependencies minimal: the compiler core must remain
  self-contained and build without production dependencies. The user plans to
  self-host the compiler later.
- Keep implementations small and dependencies minimal. Defer the broad compiler
  reduction pass until the whole language is represented in tests. Preserve
  general language functionality, not merely behavior exercised by specific tests.
- Ask the user about unclear feature semantics and edge cases before encoding
  assumptions in tests. For potentially undefined behavior, agree on the intended
  behavior and let the user document it before implementation. Passing tests do
  not establish the absence of undefined behavior.

## Resolved semantics and compiler invariants

- Previously resolved decisions (consult `docs/design.md` for the full language):
  - Bit shifts are arithmetic: signed right shifts round toward negative infinity.
    Generate portable C without relying on negative signed right shifts.
  - Functions share surrounding mutable bindings, including nested/anonymous
    functions and returned closures. Immutable captures remain by value. Prefer
    explicit parameters in examples; the capture linter remains removed.
  - Data races on ordinary mutable variables are allowed. Emit a non-fatal
    potential-data-race warning for async access; do not reject the program.
  - Mutex values are inaccessible outside explicit lock scopes, for reads as
    well as writes. Closures capture the mutex, not surrounding lock permission,
    and must acquire their own lock before accessing its value.
  - Reject equality and string conversion for functions and unawaited futures,
    including when nested in containers.
  - Strings behave as `char[]`: indexed replacement and concatenation preserve
    separate character elements even when their joined UTF-8 bytes would form a
    single Unicode grapheme. Preserve those boundaries in string operations and
    constant evaluation; printing and byte conversion flatten the elements.
  - `for` retains the original array/string indices or map keys even when the
    iterable binding changes size. Explicit body lookups use the current binding
    and retain ordinary bounds/key failures. Do not assume stable map order.
  - Map concatenation overwrites duplicate keys with the right-hand value.
  - Assignments evaluate and copy the RHS before evaluating the target. Evaluate
    target indices/keys once, then resolve and validate the entire path against
    current bindings before writing; RHS and index effects may replace ancestors.
  - IEEE-754 NaN and infinities are valid floats, including arithmetic results
    and values returned by C externs. Printing, interpolation, and string
    conversion use `NaN`, `inf`, and `-inf`. Converting nonfinite floats to `int`
    or `uint` must panic; never emit an unchecked nonfinite-to-integer C cast.
  - Float-to-`uint` conversion rejects inputs below zero before truncation:
    negative fractions must panic rather than truncate to zero.
  - Numeric byte-array encoding may use the simplest consistent implementation;
    the implementation uses little-endian bytes and IEEE-754 bits for floats.
  - Printing, interpolation, and string conversion of an error union use its
    active success value or `error: ` followed by the error message without
    requiring `try`/`catch` first. Never read inactive payloads.
  - String conversion requires every constituent type to support it, even for
    inactive variants and empty containers. This excludes void payloads. Custom
    types must first be explicitly converted to their immediate underlying types;
    unwrapping a custom string to `str` is allowed.
- `ncc run` must leave no generated files. `ncc build` must emit only the requested
  output, or the executable when output is unspecified. Honor explicit formats.
- Release mode must actually optimize, correctly across all supported types.
  Preserve effects, evaluation order, value semantics, and runtime failures.
- Warn non-fatally for structurally infinite loops and unreachable code. Do not
  interpret proven infinite loops during constant evaluation; continue folding
  independent expressions inside their bodies. Retain evaluation step/depth
  safeguards for cases structural analysis cannot decide.
- Include C headers only when needed by the generated program. Preserve NC
  binding names in generated C variables, parameters, and capture fields, with
  collision-safe prefixes and suffixes; anonymous temporaries may remain numbered.
- Preserve source locations through module loading, generic specialization, and
  optimization. Semantic errors should identify the failing expression or
  statement in its original file, including imported code.
- Preserve location wrappers during AST rewrites and use the canonical expression
  ID for type and capture metadata. Interpolation is parsed after escape decoding;
  report its original string literal, not offsets into the decoded text.
- Treat pattern comparisons as equality operations: apply the same restrictions
  on functions and unawaited futures. Value-carrying `break` needs a surrounding
  value-producing block; labels belong only on `if`, `for`, `while`, or `lock`.
- Return-path analysis must track jumps through nested blocks and expressions.
  Unreachable returns after `break` or `continue` do not satisfy a function's
  return requirement. Keep the defensive C fallthrough trap for non-void functions.
- Compile-time mutable captures share evaluator-local storage. Do not memoize
  calls whose callable or arguments contain shared cells, or reuse stateful
  closure results. Persistent outer mutable state must retain runtime evaluation.
  Release evaluation also analyses safely evaluatable top-level statements in
  source order to diagnose reached arithmetic failures, without removing effects.
  A successfully evaluated, call-free initial execution region may be replaced
  with final global initializers before any values or storage escape. Keep global
  runtime storage for later mutations and captures; roll back the entire region on
  unsupported operations, failures, or evaluator limits. Do not resume this
  precomputation past calls, closure creation, effects, or unknown state.
  Known `@print`/`@println` arguments
  are analysed without executing output; runtime calls remain intact. Test-mode
  analysis follows retained test blocks and known-true assertions in source order;
  false or unknown assertions stop analysis and retain runtime checks. Named calls
  use known lexical global storage, never caller-local shadows, and analysis must
  repeat call effects rather than memoizing them.
- Compile-time string conversion must match runtime formatting, including field
  order, quoting, embedded NULs, Unicode, and nominal types. Keep a safe runtime
  fallback for operations that cannot yet be reproduced exactly.

## Verification and workflow

- Add regression tests for fixes and expand positive, negative, differential,
  concurrency, and full-language coverage. Compare debug and release behavior.
- Run negative conformance cases in both modes. Include immutable tuple bindings
  and immutable by-value closure captures in compile-time evaluation tests; runtime-dependent
  `@embed` paths must fail without executing effects.
- Run relevant tests during development and broader checks before handoff:
  `cargo test --offline`, `cargo test --offline --no-default-features`,
  `cargo clippy --offline --all-targets -- -D warnings`, `cargo fmt --check`,
  and `cargo build --offline --release`.
- Keep Clippy's configured pedantic lints passing with warnings denied. Prefer
  fixes over exemptions; intentional semantics may use narrowly scoped, explained
  allowances only for lints that are not forbidden. `too_many_lines`,
  `needless_pass_by_value`, and `struct_excessive_bools` are forbidden: split long
  functions into cohesive helpers, borrow values that do not need ownership, and
  model state explicitly rather than exempting or compressing code.
  `case_sensitive_file_extension_comparisons` and `unicode_not_nfc` are also
  forbidden. Use path extension APIs and preserve decomposed Unicode fixtures
  with Rust escapes or external source fixtures, never by normalizing their bytes.
  Scope remaining allowances to the specific operation requiring them and briefly
  explain the language semantics that make each necessary.
- Never run `nc-tests/builtins.nc` with uncensored output: it prints environment
  variables. Run the large Fibonacci example only in release mode.
- Preserve user changes and examples. Commit small, coherent changes often.
  Never push. Report remaining limitations honestly.
- Keep this file current as the user refines the scope or verification needs.
  If commit approval is unavailable, preserve the changes and report the blocker;
  do not bypass approval controls.
