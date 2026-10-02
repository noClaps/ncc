# Remaining work

- [ ] Expand language and edge-case coverage.
  - [ ] Cover every part of the language and as many edge cases as possible.
  - [ ] Ask the user about unclear specification semantics, then capture the
        agreed behavior in tests.
  - [ ] Expand top-level constant evaluation beyond its safely evaluatable
        execution prefix while preserving effects and unknown runtime state.
        Entirely known programs already reduce to constant-string output calls;
        known output arguments and successful test assertions are also analysable.
        False assertions retain their runtime failure and stop sequential analysis;
        remaining partial-program barriers are:
    - [ ] Extend partial-program precomputation beyond the initial call-free region,
          including known calls and multi-binding declarations without losing effects
          or escaped-storage dependencies.
    - [ ] Unknown runtime inputs.
    - [ ] Other unsupported effects.

    - [ ] Expand conservative termination proof coverage.
      - [ ] Prove condition-side counter changes and varying bounds with explicit
            ranking-state analysis rather than invariant-bound assumptions.
      - [ ] Extend helper proofs through early returns, helper loops, and effectful
            arguments while preserving copied parameters and lexical scopes.
      - [ ] Discharge nested-loop termination obligations before interpreting
            enclosing loop iterations, not only when the nested loop is reached.
      - [ ] Prove additional recursive ranking patterns, mutual recursion, and
            recursive closures without speculative execution.

    - [ ] Emit compact constant aggregate data when runtime containers must remain,
          preserving independent writable storage and nested value copies.
  - [ ] Refine test-dependency slicing precision.
    - [ ] Refine conservative retention caused by dynamic/native calls.
    - [ ] Distinguish callable creation from invocation in effect summaries.
      - [ ] Generalize deferred invocation summaries to named callback arguments,
            callable aliases, forwarding, and escaping callbacks.
      - [ ] Refine capture-creation value demand without losing immutable copies
            or mutable callable replacements.
      - [ ] Distinguish allocation from invocation for factories held in
            function-valued bindings.
    - [ ] Extend exit tracking through loops, locks, labeled conditionals, and
          value expressions without losing reachable continuation.
    - [ ] Refine whole-statement/initializer retention.
- [ ] Attempt to eliminate all undefined behavior from the language.
  - [ ] Identify potentially undefined cases.
  - [ ] Clarify intended behavior with the user and let the user document those
        decisions before implementation.
  - [ ] Implement agreed behavior and add regression tests.
- [ ] Simplify the compiler once the entire language has been captured in tests.
  - [ ] Remove as much unnecessary compiler code as possible while keeping all
        tests passing.
  - [ ] Preserve a fully functional general-purpose compiler; do not overfit
        implementations to the test cases.
- [ ] Implement language tooling.
  - [ ] Implement a language server (LSP).
  - [ ] Implement a formatter.
