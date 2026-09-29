# Working on NC

## Scope and priorities

- Implement the full language and compiler toolchain described in `docs/design.md`.
  Read the relevant specification and recent documentation changes before editing.
- Follow the priorities in `TODO.md`: comprehensive language and edge-case tests,
  undefined-behavior clarification and elimination, then compiler simplification.
  Tree-sitter, LSP, and formatter work follows the compiler work.
- A task being listed in `TODO.md` is not an instruction to start it during a
  documentation-only request. Respect the user's current scope and stop requests.
- Maintain the root `TODO.md` as a remaining-work list only. Remove completed
  items, narrow partially completed items, and add newly discovered gaps. Do not
  treat passing tests as proof that the specification is fully implemented.
- The current CLI provides `build` and `run`. A Tree-sitter grammar, LSP, and
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
  - Reject equality and string conversion for functions and unawaited futures,
    including when nested in containers.
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
- Include C headers only when needed by the generated program.
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
- Never run `nc-tests/builtins.nc` with uncensored output: it prints environment
  variables. Run the large Fibonacci example only in release mode.
- Preserve user changes and examples. Commit small, coherent changes often.
  Never push. Report remaining limitations honestly.
- Keep this file current as the user refines the scope or verification needs.
  If commit approval is unavailable, preserve the changes and report the blocker;
  do not bypass approval controls.
