# Remaining work

- [ ] Expand language and edge-case coverage.
  - [ ] Cover every part of the language and as many edge cases as possible.
  - [ ] Ask the user about unclear specification semantics, then capture the
        agreed behavior in tests.
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
- [ ] Resolve the evaluator/C-backend discrepancy for indexed reads whose index
      evaluation mutates object storage or replaces its binding; clarify the
      intended read-index semantics before changing the C backend or evaluator. These cases remain
      barriers to the generalized helper termination proof.
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
