# Verification

Run from the repository root:

```sh
cargo test
cargo test --no-default-features
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Use `--offline` on Cargo commands when dependencies are already cached. The core
compiler has no production dependencies. Unicode segmentation is a test oracle only.

`tests/differential_behavior.rs` compares exit codes and exact stdout/stderr bytes
between NC debug and release modes, with independent expected results. Positive
fixtures run both with known inputs and after process-dependent input, through
both `ncc run` and actual `ncc test` roots. They cover callable selection before
argument mutation, composite snapshots, RHS copies before target replacement,
short-circuit and fallback effects, embedded NUL/Unicode output, immutable tuple
and by-value captures, and independent mutable closure cells. Failure fixtures
check division-before-target precedence, bounds and missing-key panics, prior
output, skipped catch/tails, and assertion stopping across shared-state tests.

`tests/differential_locations.rs` compares structured and rendered semantic errors
and CLI rejection in both modes: original file, byte span, line/column, excerpt,
and caret for transitive imported generics, nested closures, and escaped
interpolations. Output probes must not execute on rejection. These are compile-time
locations, not a requirement for located runtime panics. Neither suite settles
unresolved indexed-read snapshots, map traversal order, or async scheduling, nor
does passing differential coverage establish full language conformance.

Release analysis follows safely known state through retained test blocks and
assertion-expression effects, including repeated named calls sharing global
mutations. Known-true assertions permit continued analysis; false or unknown
assertions stop it without converting assertion failures into compile-time errors.
Runtime assertions, output, and mutations remain intact. Regressions cover
cross-test state, lexical shadowing, original diagnostic locations, and runtime
fallback for process inputs, native calls, and futures.

Release precomputation transactionally reduces entirely known programs to output
calls containing exact constant bytes. `tests/output_precomputation.rs` compares
debug/release array-building, shared-global calls and closures, nested output order,
argument snapshots, formatting, and rollback at unknown input or effects. The
fallback call-free initial region still becomes final global initializers while
retaining storage for a runtime suffix.

`tests/loop_termination.rs` and `tests/recursion_termination.rs` cover proof-gated
execution beyond the former fuel/depth limits, counted-loop strides and overflow,
skipped progress, invariant-bound violations, alias/callback cycles, covering
recursive base cases, exact float ranks, heap continuations, and failure locations.
`tests/loop_proofs.rs` compares debug/release output for effectful condition wrappers,
conditional strides, progress before continues, exiting branches, shared-state
helper chains, scalar helper returns, aliases, copied bounds, and lexical shadows.
Negative probes use variable-dependent arithmetic failures to detect speculative
entry into candidates with resets, unproven bound changes, skipped progress, overflow,
recursive helpers, or callable replacement. Condition effects retain their final
false check and original diagnostic locations. Proof-only helper trees are never
executed and do not replace source trees or their checked metadata.

`tests/helper_proofs.rs` extends that coverage through call-local early returns,
labeled conditionals, counted and literal-array helper loops, immutable global and
captured loop inputs, and effectful scalar arguments. Debug/release comparisons
check argument snapshots, lexical parameter shadows, nested output order, RHS
copies before target-index effects, and first/final condition checks. Counted
helper loops obtain certificates without execution; independent monotonic updates
can supply enclosing-loop progress through closed-form displacement summaries.
Varied Boolean condition returns may use a wider monotonic comparison solely as a
termination envelope, never as a replacement for the actual returned value.
Negative probes cover return paths without progress, restored copied arguments,
unproven helper loops, changing bounds, arithmetic overflow, and short-circuit
argument effects. Unit tests also assert that proof construction leaves evaluator
cells unchanged and defensively protects writable parameter copies.

`tests/helper_loop_summaries.rs` covers variable trip counts and nonuniform
monotonic helper-loop updates. Checked input ranges bound iteration counts and
all intermediate excursions without evaluating a helper. Displacement envelopes
are termination-only and must never become executable replacements.
`tests/helper_results.rs` covers exact branch-dependent scalar and container
returns, return-site snapshots, direct and nested consumers, early exits, copied
arguments, and invalidation after writes. Proof continuations keep every possible
return path rather than choosing a representative value.
`tests/helper_arguments.rs` covers array, tuple, map, nested-container, and string
arguments, ordered aggregate effects, copied projections, assignment RHS copies,
and known and unknown short-circuit paths. Lifted RHS effects remain conditional;
conditional-only progress cannot establish termination. All three suites compare
debug/release output and compile unsafe arithmetic probes in both modes.

`tests/varying_bounds.rs` compares debug/release behavior for converging and
same-direction counter/bound updates, correlated conditional strides, labeled
continues, shared helpers, lexical shadows, all supported comparisons, and fixed
condition-prefix updates. Negative arithmetic probes cover diverging ranks, resets,
skipped progress, nonexact inequality strides, and overflow of either binding,
including intermediate updates and exiting branches. Certificate unit tests also
check each binding's limits and horizons beyond former execution budgets.
The relational certificate rejects unsummarized nested loops. Supported nested
loops are certified and summarized in proof-only trees before enclosing entry.

`tests/compound_loop_ranks.rs` covers calculated limits with several changing
bindings, known constant and calculated steps, conditional progress, helpers,
shared captures, copied constants, local shadows, and original error locations.
It compares debug/release output for resetting counters and multiplied counters,
including output order, breaks, labeled continues, unsigned strides, and fixed
condition prefixes. Negative probes check missing progress, changing steps,
unproven resets, nested loops, unreachable inequality targets, intermediate
arithmetic overflow, and overflow on the final update or condition check.
Unit tests exercise large proof horizons without executing candidate loops,
branch correlation, per-expression integer ranges, outer-counter stride residues,
and bounded reset invariants. Multiplication unit tests also compare small
recurrences against independent arithmetic safety calculations.

Unproven loops/recursion are not entered speculatively. Current certificates remain
conservative for nonlinear calculated limits, mutual recursion, and more general
ranking functions. Helper summaries support literal trip counts and universally
bounded variable trip counts with monotonic additive updates; resets, mixed-direction
excursions, and unbounded or overflowing helper updates remain conservative.
Certified rank-neutral helper loops may retain their control flow. Exact
branch-dependent scalar and structural container results use return-site samples.
Callables/futures inside containers, unsupported nominal coercions, and
loop-carried samples remain barriers. Indexed reads whose index effects replace
the object binding also remain barriers pending resolution of an existing C
backend/evaluator discrepancy; no intended semantics are assumed here.
Reset proofs currently track two counters; multiplication proofs require a positive
seed and identical continuing-path updates against a fixed or additive limit.
Nested-loop obligations are discharged before interpreting enclosing iterations,
condition-prefix effects, or `for` iterable effects. `tests/nested_loop_obligations.rs`
covers multi-level local counters, finite traversals, iterator shadowing, local and
outward labeled exits, hidden helper loops, final false checks, and reached failure
locations. Negative arithmetic probes compile in both modes; evaluator unit tests
also check that rejected candidates leave bindings, shared cells, and recorded
output untouched. Repeated-entry certificates use local initializers or universal
input ranges, never an outer mutable cell's current value. Unsupported obligations
remain runtime code. Compound and varying-bound certificates still reject
unsummarized nested loops. `tests/condition_ranks.rs` covers fixed and conditional prefix
updates, optional prefix progress combined with body strides, empty bodies, first
and final checks, nested output order, labeled and unlabeled continues, nested
`while`/literal `for` traversals, outward breaks, resets, missed strides, and
overflow/underflow. Negative cases compile in both modes without entering unproven
candidates; certified continues diagnose reached arithmetic failures at the original
expression. Certificate unit tests exercise full proof horizons, label routing,
conditional first-sample ranges, intermediate excursions, and nonexact inequality
rejection without executing candidate loops.
Physical memory and compilation time
still constrain large evaluations; there is no artificial evaluation budget.
`tests/c_names.rs` checks recognizable, collision-safe C names for bindings,
parameters, capture fields, shadowing, and imported globals, and executes its
fixtures in both modes.

`tests/unicode_dependencies.rs` checks emitted-C Unicode dependencies and execution
in debug and release modes. Output-only formatting of nested and recursive values,
active optional/error payloads, byte conversion, and empty-string concatenation
identities avoid segmentation tables. Real character-aware operations and string
conversions retain their required support. Tests keep output/value helper caches
separate, preserve combining-character boundaries and embedded NULs, and check
print-argument snapshots and single-interpolation effects. The strings workload is
executed only in a reduced regression, not at its full example iteration count.

## Coverage map

See [the specification coverage inventory](coverage.md) for a section-by-section
map to concrete assertions, compiler-mode limits, missing test cases, and semantics
questions requiring clarification. The summary below is a suite-level guide, not
a claim of complete language coverage.

| Area                                                                                                                                                                         | Regression tests               |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ |
| Types, bindings, operators, patterns, loops, errors, optionals, generics, imports, closures, concurrency, value semantics                                                    | `tests/conformance.rs`         |
| Compile-time folding, numeric widths, cast table, closures, active error payloads, termination certificates, sampled float formatting, evaluation order, Unicode/NUL strings | `tests/optimizer.rs`           |
| Binary embedding, lexical/tuple/captured paths, empty files, symlink and runtime-dependency rejection                                                                        | `tests/embed.rs`               |
| C ABI declarations, shared implementation files, inactive optional/error payloads                                                                                            | `tests/externs.rs`             |
| CLI help/options, removed-command rejection, release mode, targets, runtime process state                                                                                    | `tests/cli.rs`                 |
| Non-fatal async race warnings, mutex-safe cases, indirect/recursive calls, imported specialization locations, build/run success                                              | `tests/warnings.rs`            |
| Build formats, artifact isolation, required C headers, basic diagnostics                                                                                                     | `tests/compiler.rs`            |
| Escapes, multiline literals, graphemes, expression ranges, imported/specialized/constant-evaluation diagnostics                                                              | `tests/frontend.rs`            |
| Grapheme boundaries against an independent oracle                                                                                                                            | Unit tests in `src/unicode.rs` |

Test-slicing regressions invoke `ncc test` directly in debug and release modes,
without wrapping outside statements in synthetic test roots. Summaries exclude
executable effects after direct jumps through the same or nested plain lexical
blocks and in syntactically proven dead Boolean branches, false loops, and
short-circuit operands. References needed to type-check dead syntax remain, but
known runtime paths separately determine which prior mutations are retained.
Opaque calls fall back to full dependency demand to protect callback reads and
escaped state. Selected global initializers still execute in full. Tests protect
ordered multi-pattern arms,
unknown pattern effects, name comparisons, jump-operand mutations, captured writes,
await dependencies, qualification errors, and scope restoration. Boundary tests
preserve reached effects after loops, locks, labeled conditionals, and value
expressions that consume jumps. Dead helper references preserve imported semantic
error locations without retaining unrelated writers; direct and recursive reads,
callback state, and syntax-selected initializers have runtime regressions.
Synchronization traversal follows runtime references rather than dead syntax;
regressions preserve outside waits for native readers and escaped mutex-protected
closure state while discarding unrelated waits. Callable-dependency promotion
likewise follows runtime references in global initializers and callable replacements,
so dead helper references retain semantic checking without demanding unrelated
mutations. Regressions preserve returned recursive callbacks, escaped captured-cell
aliases, required mutex synchronization, and imported error locations.
Literal lambda arguments to unambiguous named functions with syntactically unused
parameters contribute semantic and conservative capture-creation dependencies, but
not invocation effects. Regressions cover discarded callback-only statements,
dynamic/recursive/async callback bodies, retained capture initializers and mutable
state, invoked and escaping callbacks, effectful producer arguments, name-pattern
comparisons, shadowing, and local/imported qualification errors. References in dead
or shadowed syntax deliberately prevent unused-parameter certification.
Escaped-cell write proxies attach to global value bindings, not named factories.
Destructured sibling bindings are conservatively grouped because their callbacks
can share storage. Regressions discard syntax-only invocation history and separate
independent factory instances while preserving real factory mutations of existing
cells, sibling writer/reader history, callable aliases and replacements, and awaited
mutex-protected sibling writes.
Reachable dynamic/native calls, other callable creation versus invocation cases,
capture-creation demand, factories held in function-valued bindings, and more complex
control flow remain conservative.

Successful conformance fixtures use debug and release execution, including
module exports, imported generic types and patterns, and external C calls.
Module visibility, cyclic imports, and external-declaration errors are checked
in both modes. Optimizer tests
compare emitted behavior in both modes and, where specified, verify that evaluated
functions disappear from generated C. The very large Fibonacci test is release
only to avoid exponential runtime work. C integration tests exercise both modes.
Negative tests distinguish rejected programs from runtime range/bounds failures.
Integer boundary coverage includes byte, signed and unsigned 64-bit arithmetic,
with runtime-dependent operands for overflow cases so release folding cannot
replace the runtime checks. Runtime-failure conformance helpers require successful
compilation, exit code 1, and the expected diagnostic in both modes.
Arithmetic right shifts cover every valid signed shift count against a wider
floor-division oracle, including negative odd values and signed boundaries;
constant-folding tests check the same rounding and boundary behavior.
Negative conformance cases run in both modes, including the operator/type matrix,
escaping futures, duplicate generics, expansion limits, and return paths containing
unreachable statements. Concurrency tests cover shared awaits and discarded workers.
String-conversion tests cover the `error: ` prefix, inactive error payloads,
recursive constituent restrictions, and explicit custom-type unwrapping (including
custom strings versus interpolation and chained custom types).

Nested optional/error fallback tests cover nonlocal loop jumps, early returns,
and `try` propagation while retaining shared mutations. An abandoned assignment
must not evaluate its target. Folding tests also verify restoration of shadowed
bindings on fallback jumps; negative cases retain rejection of missing values and
invalid jump targets.

Function side-effect coverage includes shared mutable scalar and container
bindings, named callbacks, nested and returned closures, write-only captures,
shadowing, tuple bindings, and independent factory invocations. Immutable closure
captures retain compile-time folding coverage. Shared mutable captures are tested
for folding, independent factory calls, repeated mutations, loop scopes, errors,
container callbacks, and computed embed paths. Persistent outer state and runtime
effects retain their runtime fallback. Closures created inside lock
scopes cannot inherit write permission; returned closures that acquire their own
locks are tested sequentially and concurrently. Unlocked mutex reads are rejected
for scalar and container access, interpolation, comparisons and patterns; generic
imported read errors retain their original expression location.

Pattern-only captures are checked across scalar, tuple, array, struct, and enum
patterns, including returned closures, mutable updates, comparison order,
constant folding, and shared-read race warnings.
Direct map subjects are rejected, including generic specializations; bare `if`
branches retain map equality and membership comparisons.

Data-race warning tests compile the racing examples without asserting a particular
result. CLI execution tests use an awaited single worker for deterministic output.
Warnings are collected before optimization and checked in both modes.

The tests are a regression suite, not a proof of specification completeness.
See [the remaining work](../TODO.md) for known gaps.
