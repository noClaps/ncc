# Remaining work

- [ ] Expand language and edge-case coverage.
  - [ ] Cover every part of the language and as many edge cases as possible.
    - [ ] Cover numeric boundaries, Unicode character boundaries, embedded NULs,
          empty containers, and runtime failures.
    - [ ] Expand async, future, mutex, and shared-storage coverage without relying
          on a particular thread schedule.
      - [ ] Add deterministic pre-await progress and mutex-contention checks;
            assert bare-break lock reacquisition without timing assumptions.
    - [ ] Compare debug and release behavior, including output, evaluation order,
          side effects, runtime failures, and source locations.
      - [ ] Expand builtin process-state failure edges and build/run/test artifact
            cleanup checks on backend and runtime failures in both modes;
            retain the successful inferred/explicit/default build-format matrix,
            empty/Unicode argument forwarding and silent no-tests regressions.

  - [ ] Resolve unclear specification semantics before encoding assumptions.
    - [ ] Resolve the questions recorded in `docs/coverage.md` and collect further
          ambiguous interactions from differential tests.
      - [ ] Clarify operator compatibility, numeric/index failure conventions,
            tuple index requirements, aggregate-pattern exhaustiveness, and
            missing/duplicate struct initializer fields.
      - [ ] Clarify specialized generic function-value syntax; bare
            `identity<Count>` currently fails parsing, while typed callback
            wrappers forwarding explicit generic calls are covered.

      - [ ] Clarify multiline/Unicode contracts, observable representation,
            map key rules, function-result consumption, and caught error values.
      - [ ] Clarify nested lock jump targets, future lifecycle, permitted race
            guarantees, module initialization identity, and optimization scope.
    - [ ] Ask the user to resolve each ambiguity; do not infer intended behavior
          from the current implementation or passing tests.
    - [ ] Have the user document decisions, or obtain explicit approval for any
          specification edits, before adding conformance tests and fixes.
  - [ ] Refine test-dependency slicing precision.
    - [ ] Refine conservative retention caused by dynamic/native calls.
      - [ ] Identify which retained statements are required by unknown call
            targets, callback invocation, shared state, and external effects.
      - [ ] Propagate available callable identities and effect summaries through
            aliases and arguments; retain conservative fallbacks for unknowns.
      - [ ] Add retention and execution regressions for dynamic/native calls,
            including mutations and callbacks required by tests.
    - [ ] Distinguish callable creation from invocation in effect summaries.
      - [ ] Generalize deferred invocation summaries.
        - [ ] Track invocation through named callback arguments and callable
              aliases.
        - [ ] Propagate deferred invocation through callback-forwarding helpers.
        - [ ] Track escaping callbacks and retain their possible later effects.
      - [ ] Refine capture-creation value demand.
        - [ ] Separate immutable by-value capture snapshots from shared mutable
              storage dependencies.
        - [ ] Retain earlier mutations needed to create immutable capture copies.
        - [ ] Preserve mutable callable replacements and their invocation order.
      - [ ] Distinguish allocation from invocation for factories held in
            function-valued bindings.
        - [ ] Separate factory execution effects from returned-callable effects.
        - [ ] Track returned callables through storage, forwarding, and invocation.
      - [ ] Add regressions proving unrelated callable bodies can be discarded
            without losing creation effects, capture snapshots, or later calls.
    - [ ] Extend exit tracking without losing reachable continuation.
      - [ ] Track local and outward break/continue targets through nested loops.
      - [ ] Track exits through lock scopes and labeled conditionals.
      - [ ] Track returns, throws, and value-carrying jumps through expressions.
      - [ ] Add regressions for retained dependencies before and after exits,
            including conditional exits and unreachable tails.
    - [ ] Refine whole-statement/initializer retention.
      - [ ] Identify over-retained compound statements and multi-binding
            initializers.
      - [ ] Separate demanded results from effects and shared-storage dependencies
            that must still execute.
      - [ ] Preserve RHS-before-target evaluation, argument snapshots, declaration
            order, and runtime failures when narrowing retention.
      - [ ] Add regressions for both discarded unrelated output and retained
            effects needed by tests, including imported tests.
- [ ] Avoid the generated C tautological negative-range check for bool-to-uint
      conversions; preserve the successful true/false cast regressions.
- [ ] Resolve the evaluator/C-backend discrepancy for indexed reads whose index
      evaluation mutates object storage or replaces its binding.
  - [ ] Isolate mutation, binding replacement, nested-container, and restored-value
        cases; record current evaluator and runtime behavior.
  - [ ] Ask the user to clarify object/index evaluation order, snapshot timing,
        and bounds/key validation against the original or current object.
  - [ ] Have the user document the intended semantics, or obtain explicit approval
        for specification edits, before changing read behavior.
  - [ ] Align evaluator and C-backend reads with the agreed semantics, preserving
        side effects, value copies, and original source locations.
  - [ ] Add positive and failure regressions comparing debug and release behavior.
  - [ ] Revisit constant-evaluation and generalized helper-proof barriers only
        after the discrepancy is resolved and storage dependencies are tracked.
- [ ] Attempt to eliminate all undefined behavior from the language.
  - [ ] Identify potentially undefined cases.
    - [ ] Audit numeric arithmetic, shifts, casts, and nonfinite float handling.
    - [ ] Audit bounds/key failures, allocation sizes, container copies, inactive
          payloads, and generated C storage lifetimes.
    - [ ] Audit closure capture lifetimes, async synchronization, shared access,
          and C extern boundaries.
    - [ ] Separate unspecified language behavior from generated-C undefined
          behavior and implementation bugs.
  - [ ] Clarify intended behavior with the user before implementation.
    - [ ] Present each unresolved case with a minimal reproducer and the relevant
          specification gap.
    - [ ] Agree on the intended result, failure, or permitted nondeterminism.
    - [ ] Let the user document those decisions before implementing them.
  - [ ] Implement agreed behavior and add regression tests.
    - [ ] Fix evaluator, semantic analysis, and backend behavior as applicable.
    - [ ] Add positive, negative, and debug/release differential regressions.
    - [ ] Use available C compiler diagnostics and sanitizers to investigate
          remaining hazards; do not treat clean runs as proof of absence.
- [ ] Simplify the compiler once the entire language has been captured in tests.
  - [ ] Confirm the coverage inventory represents the full specification before
        starting the broad reduction pass.
  - [ ] Identify duplicate logic, unnecessary abstractions, obsolete paths, and
        avoidable special cases.
  - [ ] Remove unnecessary code in small, coherent changes while preserving a
        fully functional general-purpose compiler.
    - [ ] Consolidate shared logic without erasing language-specific semantics.
    - [ ] Remove obsolete code and redundant state only after checking all callers.
    - [ ] Compare debug/release behavior and generated C for each changed area;
          do not overfit implementations to the test cases.
  - [ ] Run the full test, Clippy, formatting, and release-build checks after each
        coherent reduction.
- [ ] Implement language tooling after compiler coverage, undefined-behavior work,
      and simplification.
  - [ ] Decide the tooling interface and CLI integration with the user; do not
        restore removed commands or add editor setup files implicitly.
  - [ ] Implement a language server (LSP).
    - [ ] Define document tracking, incremental update handling, and the boundary
          between editor analysis and compiler operations.
    - [ ] Publish located parsing and semantic diagnostics, including imported
          files, without executing programs or runtime effects.
    - [ ] Add symbol indexing, navigation, hover information, and completion using
          compiler type and scope information where available.
    - [ ] Add protocol tests for document changes, Unicode positions, imports,
          errors, and recovery from incomplete source.
  - [ ] Implement a formatter.
    - [ ] Agree on formatting conventions and comment-preservation behavior.
    - [ ] Preserve comments, literal contents, statement separators, and multiline
          expression semantics when formatting valid source.
    - [ ] Add idempotence, parsing-equivalence, and semantic regression tests.
    - [ ] Define handling of invalid or incomplete source without silently changing
          program meaning.
