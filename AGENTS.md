# Working on NC

## Scope and priorities

- Implement the full language and compiler toolchain described in `docs/design.md`.
  Read the relevant specification and recent documentation changes before editing.
- Prioritize compiler correctness, verification, and cleanup. Continue until the
  remaining-work list is complete; report blockers rather than claiming completion.
- Maintain the root `TODO.md` as a remaining-work list only. Remove completed
  items, narrow partially completed items, and add newly discovered gaps. Do not
  treat passing tests as proof that the specification is fully implemented.
- The CLI provides `build` and `run`. Do not restore `check`, `fmt`, or `lsp`,
  their linter/formatter/language-server code, a Tree-sitter grammar, editor
  plugins, extensions, or editor setup files. These are outside the requested scope.
- Do not implement a standard library. External implementations are C-only for
  now; an Etch backend is out of scope.

## Implementation constraints

- Continue with Rust. Keep dependencies minimal: the compiler core must remain
  self-contained and build without production dependencies. The user plans to
  self-host the compiler later.
- Prefer less code. Remove or rewrite obsolete code and simplify implementations
  as tests establish that behavior is preserved.
- Ask when language semantics are materially ambiguous rather than inventing
  behavior. Previously resolved decisions:
  - Anonymous functions capture surrounding values by value. Prefer explicit
    parameters in examples; the previously requested capture linter is removed.
  - Reject equality and string conversion for functions and unawaited futures,
    including when nested in containers.
  - Numeric byte-array encoding may use the simplest consistent implementation;
    the implementation uses little-endian bytes and IEEE-754 bits for floats.
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
- Compile-time string conversion must match runtime formatting, including field
  order, quoting, embedded NULs, Unicode, and nominal types. Keep a safe runtime
  fallback for operations that cannot yet be reproduced exactly.

## Verification and workflow

- Add regression tests for fixes and expand positive, negative, differential,
  concurrency, and full-language coverage. Compare debug and release behavior.
- Run negative conformance cases in both modes. Include immutable tuple bindings
  and by-value closure captures in compile-time evaluation tests; runtime-dependent
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
