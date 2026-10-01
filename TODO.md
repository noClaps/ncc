# Remaining work

- [ ] Expand language and edge-case coverage.
  - [ ] Cover every part of the language and as many edge cases as possible.
  - [ ] Ask the user about unclear specification semantics, then capture the
        agreed behavior in tests.
  - [ ] Expand top-level constant evaluation beyond its safely evaluatable
        execution prefix while preserving effects and unknown runtime state.
        Output with known arguments and test assertions with known conditions are
        already analysable. False assertions retain their runtime failure and stop
        sequential analysis; other remaining barriers are:
    - [ ] Unknown runtime inputs.
    - [ ] Other unsupported effects.
    - [ ] Evaluator limits; consider configurable evaluation budgets for large
          individual computations.
  - [ ] Refine test-dependency slicing precision.
    - [ ] Refine conservative retention caused by dynamic/native calls.
    - [ ] Distinguish callable creation from invocation in effect summaries,
          including lambda arguments whose bodies are never called.
    - [ ] Track exits through nested blocks without confusing consumed jumps
          with exits from their enclosing statements.
    - [ ] Separate semantic declaration dependencies from runtime value
          dependencies so dead references do not retain unrelated prior writers.
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
