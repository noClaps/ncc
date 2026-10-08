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

Cargo's development/test builds optimize the Rust compiler at level 2 while
retaining debug assertions, overflow checks, and debug information. This affects
the compiler executable, not NC's debug/release modes. Test discovery and the
individual integration-test binaries are unchanged; focused suites still use
commands such as `cargo test --offline --test evaluation_performance`.

For performance measurements, separate Rust build time from test execution and
run timed checks sequentially, without concurrent builds or profiling processes.
The October 2026 optimization pass measured an original full `cargo test --offline`
run at 119.35 seconds on an eight-core Apple Silicon machine. Reusing immutable
constant-evaluation indexes, memoizing completed type-layout checks, eliminating
redundant scope copies, and optimizing development builds reduce compiler work.
A 432-assertion fixture's unoptimized Rust evaluator pass dropped from 5.22 seconds
to about 16 milliseconds by sharing lookup indexes. Suite timings also depend
heavily on native C builds and machine load; benchmark with ready build artifacts
and report rebuild time separately. Measurements are not timing assertions or a
guarantee for clean builds and other machines. Debug/release coverage and runtime
failure checks are unchanged. The final default run passed all 949 tests in
86.01 seconds with ready build artifacts. Subsequent no-default-features runs
also passed all tests but took 94.94 and 121.87 seconds under changing machine
load, so the 90-second budget is not yet consistently reproducible.

`tests/evaluation_performance.rs`, `tests/layout_performance.rs`, and
`tests/scope_performance.rs` guard large independent-expression workloads, shared
type graphs, evaluator-state isolation, lexical scopes, and source diagnostics.
The evaluator shares only immutable expression/lambda lookup maps: mutable cells,
call memoization, recorded output, and failure/jump state remain evaluator-local.

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

`tests/negative_container_edges.rs` and `tests/negative_function_edges.rs`
check 116 rejection fixtures in both NC debug and release modes, asserting the
relevant diagnostic rather than accepting any compilation failure. They cover
noninteger array/string indexing, typed array/map writes, immutable map insertion,
required return annotations, function-local scope, nonoptional `none` arguments,
and representative parenthesized function-type requirements. Compile-only valid
controls guard against blanket rejection; these suites do not execute the controls
or establish semantics for byte/nominal indices or unresolved map key domains.

`tests/function_type_positions.rs` pairs parenthesized syntax controls with
individually unparenthesized rejections across the remaining binding, declaration,
container, wrapper, generic, conversion and nested/anonymous/extern signature
positions. Rejections compile in both modes; map-key and extern positive controls
only parse and do not assert a callable key domain or C callback ABI. Execution
fixtures invoke callbacks in the other positions in both modes with process-dependent
inputs, including nominal conversions, generic forwarding, futures and mutexes.

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

## Compilation benchmarks

Run the stdlib-only Python runner from the repository root (Python 3, Cargo, and
Rust are required; `--native` also requires the system C toolchain):

```sh
python3 scripts/benchmark-compilation.py --help
python3 scripts/benchmark-compilation.py --output compilation.csv
python3 scripts/benchmark-compilation.py --samples 9 --warmup 3 \
  --sizes 128,512,2048 --depths 8,16,24 --native --output compilation-native.csv
python3 -m unittest discover -s scripts -p 'test_benchmark_compilation.py'
```

The runner first prepares the benchmark with
`cargo build --offline --release --bench compilation --message-format=json`.
Cargo supports `build --bench`; this command builds but does not execute the
benchmark. The runner discovers its executable from Cargo's `compiler-artifact`
JSON rather than assuming a target directory or artifact filename, then invokes
it once, sequentially. Cargo preparation wall time is reported separately on
stderr and is not included in the compilation samples. No `cargo clean` is run.
The metadata records whether the benchmark artifact and all reported artifacts
were fresh, but freshness describes Cargo reuse, not a true clean build. For a
clean-build comparison, deliberately prepare a separate empty Cargo target
directory, record that procedure, and account for preparation separately.

Defaults are seven measured samples and two warmups, expression counts
`128,512,2048`, and shared-type graph depths `8,16,24`. Samples, sizes, and depths
must be positive integers; warmups may be zero. Size/depth lists must contain
distinct comma-separated integers. Both NC debug and release modes are measured
using the same release-built Rust benchmark executable; the `mode` column does
not describe Cargo's build profile.

The harness can also run directly with
`cargo bench --offline --bench compilation -- --samples 7`; that command does not
separately record Cargo preparation or host metadata, so use the Python runner
for comparisons.

The `expression_list` workload times `compile_test_source_with_options`, while
`shared_type_graph` times `compile_source_with_options` on shared type DAGs.
`nc_compile` measures the end-to-end library compilation to emitted C, not
individual parser, checker, evaluator, or code-generation phases. With `--native`,
`native_c` separately measures C compilation and linking using the CLI's debug
`-O0 -g` or release `-O3` settings and macOS architecture flags. It never executes
the generated programs. Native timings depend on the C toolchain as well as the
size of emitted code; they are not NC-only compilation timings.

Stdout contains only the raw harness CSV:

```text
workload,size,mode,phase,sample,elapsed_ns,source_bytes,c_bytes
```

`size` counts expression pairs (one arithmetic assertion and one closure-call
assertion) for `expression_list`, and is the graph depth for
`shared_type_graph`; `elapsed_ns` is the measured duration in nanoseconds. The
runner validates workload/mode/phase groups and measured sample counts, then
reports median and minimum durations per group on stderr. `--output PATH` also
saves the unmodified CSV to `PATH` and JSON metadata to `PATH.json` (existing
files are overwritten; the parent directory must exist). Without `--output`,
CSV can be redirected from stdout and metadata is still printed on stderr.
Metadata includes arguments, exact build/run commands, Cargo preparation and
whole-harness wall times, tool versions, OS/CPU details, load averages before
preparation and before/after samples, Git revision and dirty status, artifact
freshness, and summaries. Dirty status includes untracked files; retain the
source diff separately when comparing dirty revisions.

Run comparisons sequentially under controlled load, with the same toolchain,
arguments, host, and cache-preparation procedure. Stop unrelated builds,
profilers, and other CPU-heavy processes, and inspect the recorded load averages.
Warmups do not make these results independent of caches, thermal state, or
background activity. There are no wall-time limits or pass/fail timing thresholds;
large requested workloads can take substantial time. These focused measurements
are not full-suite execution timings, proof of language conformance, or evidence
that the full test suite consistently completes within 90 seconds.

## Native full-suite profiling

Use the stdlib-only runner to attribute actual native C-driver costs to test
binaries, independently of the compilation-only workloads above:

```sh
python3 scripts/profile-native-tests.py --output tmp/native-profile \
  --samples 3 --test-threads 8 --max-load 6 --cooldown 90
python3 -m unittest discover -s scripts -p 'test_profile_native_tests.py'
```

Choose thread count and load threshold for the host rather than copying these
values universally. Stop unrelated CPU-heavy work, prepare artifacts, let the
machine settle, and inspect live CPU idle capacity as well as load averages.
`--max-load` rejects a sample whose initial one-minute load exceeds the threshold;
it does not certify quiet conditions throughout the run. `--cooldown` defaults
to 90 seconds between samples and is excluded from sample wall times. macOS
`top` snapshots before/after each sample use two observations one second apart;
process-inspection permissions may be required. Probe failures are retained in
the report rather than silently claiming controlled load.

The output must be a new directory. `report.json` contains toolchain/host/Git
metadata, exact commands, Cargo artifact freshness and preparation time,
per-binary test wall times, per-invocation native records, and grouped summaries.
The runner builds with `cargo test --offline --no-run --message-format=json`,
discovers test executable paths from Cargo messages, then runs unit/integration
binaries sequentially with the requested test-thread count. Doctests run once
separately and their time is not included in profile samples. Use
`--no-default-features` for the corresponding preparation and doctest commands.
`--timeout` bounds each build or test binary, not the entire profiling run.

A temporary `cc` wrapper forwards arguments and inherited stdout/stderr to the
original compiler. It records compiler-driver wall/descendant CPU time, exit
status, actual optimization/debug flags, primary `.c` input sizes/hashes,
includes, and Unicode-table presence. It does not instrument Rust compilation,
absolute compiler paths, or deliberately substituted test compilers. Primary
inputs do not include transitive header/extern contents. Summed concurrent
native durations are not suite wall time, and wrapper startup/input inspection
adds substantial observer overhead. **Measure the 90-second budget separately
with uninstrumented, ready-artifact `cargo test --offline` repeats.**

All test output is saved to private per-binary logs; the terminal shows only
suite names, timings, and statuses. Do not dump logs containing environment
output. `--keep-c-sources` optionally retains content-addressed primary inputs
for separate compile/link experiments; these can contain embedded private data
and may not be self-contained when including temporary external sources. Keep
the output directory local/ignored. Existing outputs are never overwritten.
Failures preserve an incomplete report and return nonzero; expected negative
native statuses are not failures when their enclosing Rust tests pass.

See `docs/native-compilation-profile.md` for the October 2026 measurements,
compile/link replays, observed wrapper overhead, and remaining budget work.

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
