# Native test compilation profile — October 8, 2026

## Conditions and method

This profiles the remaining native C costs, not a compiler optimization change.
The source baseline was `5ba93900661840e74d98b18f46960f0475d6c6ac`, with the
profiling scripts added locally. Host: eight-CPU Apple M2, macOS 27.0.1,
Apple clang 21.0.0 (`clang-2100.3.34.2`), arm64; Rust 1.99.0
(`b940084d7`, September 28, 2026). Cargo's prepared development test artifacts
were fresh; no clean build was timed. The compiler, generated runtime, native
flags, test fixtures, and debug/release coverage were unchanged.

All measurements ran sequentially, without another agent build, test, or
profiler. A 90-second idle interval separated the completed profiling samples
and the uninstrumented repeat runs. Before starting, live CPU activity was
inspected with `top -l 2 -s 1 -n 0`; use the second CPU sample, not just the
longer-lived load averages. Ordinary editor/window-server activity remained:
this is a controlled scheduling/low-load comparison, not an isolated machine.

An initial attempt was rejected before samples at one-minute load 16.05.
Another attempt completed one 129.97-second sample but rejected its second
sample because the preceding suite itself raised the load. The runner now
inserts a cooldown between samples. Neither rejection was a test failure;
incomplete reports are explicitly marked and must not be treated as completed
multi-sample runs.

The completed profile used:

```sh
python3 scripts/profile-native-tests.py --output tmp/native-profile-8-repeat \
  --samples 2 --test-threads 8 --max-load 6 --cooldown 90 --keep-c-sources
```

It discovers Cargo's unit/integration test executables and runs each once per
sample, sequentially, with eight test threads within each executable. A temporary
`PATH` wrapper forwards native `cc` arguments unchanged and records the actual
compiler driver's wall and descendant CPU time. The suite wall clock also
includes Python startup, input inspection, and recording: **it is not a budget
measurement**. Cargo preparation took 0.032 seconds; separate doctests took
0.510 seconds and contained no tests.

## Full-suite observations

| Run                                    | Tests passed | Wall seconds | Starting one-minute load | Starting live CPU idle |
| -------------------------------------- | -----------: | -----------: | -----------------------: | ---------------------: |
| Uninstrumented `cargo test --offline`  |          949 |        79.46 |                     4.26 |                 91.40% |
| Uninstrumented default repeat          |          949 |        81.72 |                     2.80 |                 86.45% |
| Uninstrumented `--no-default-features` |          949 |        81.56 |                     2.49 |                 80.16% |
| Profile sample 1                       |          949 |       128.36 |                     2.52 |                 84.29% |
| Profile sample 2                       |          949 |       129.27 |                     3.37 |                 89.50% |

Uninstrumented commands were timed with `/usr/bin/time -p`, with stdout/stderr
saved privately. Cargo reported ready artifacts, and its small command overhead
is included in those wall times. Both profiling samples recorded **2,651 native
invocations**. There were 20 nonzero native statuses per sample from expected
negative cases; all Rust tests passed. The roughly 47-second difference between
the default repeat and profile samples illustrates substantial observer cost;
it is not a matched per-invocation overhead correction.

Sample 1's actual native-driver totals were **278.56 summed wall seconds** and
**281.01 CPU seconds**. Concurrent compiler invocations overlap, so summed wall
time cannot be added to or subtracted from suite wall time. CPU excludes the
Python wrapper itself. Actual C settings, not NC optimization mode, give:

| C optimization   | Invocations | Summed native wall seconds | Native CPU seconds |
| ---------------- | ----------: | -------------------------: | -----------------: |
| `-O0`            |       1,313 |                     128.05 |             127.59 |
| `-O3`            |       1,298 |                     147.89 |             150.36 |
| `-O2`            |           9 |                       1.19 |               1.06 |
| No explicit `-O` |          31 |                       1.43 |               2.01 |

Of the `-O0` invocations, 1,303 also used `-g`; direct native tests account for
the remaining ten. There were 724 invocations with Unicode tables in their
primary C inputs. Primary-source hashes plus optimization/debug settings gave
2,248 distinct identities, versus 2,651 invocations. This is only an indication
of repeated inputs: it ignores other flags, included files, and external state,
and is **not a sound cache key**.

### Highest aggregate native costs

Sample 1, ranked by native CPU time:

| Test binary                     | Invocations | Summed native wall seconds | Native CPU seconds |
| ------------------------------- | ----------: | -------------------------: | -----------------: |
| `conformance`                   |         358 |                      44.67 |              39.57 |
| `compiler`                      |         389 |                      39.03 |              34.71 |
| `optimizer`                     |         179 |                      18.87 |              16.91 |
| `index_failures`                |         172 |                      12.42 |              15.62 |
| `positive_containers`           |          98 |                      14.09 |              15.47 |
| `positive_nominal_interactions` |          96 |                      11.32 |              13.18 |
| `positive_async_payloads`       |         100 |                      11.52 |              13.16 |
| `positive_modules_generics`     |          62 |                      11.18 |              10.26 |

The first three account for 926 invocations and about 32% of native CPU time.
Large frontend-only negative matrices must not be mistaken for native builds.
The native Unicode unit oracle was not a dominant outlier: its `-O0`/`-O2`
compilations were below 0.3 seconds each in the initial sample. The longest
sample-1 invocation was instead `unicode_version` at `-O3`: 0.877 seconds for
224,920 bytes of emitted C. Recursive-map and imported-generic fixtures also
produced relatively expensive optimized builds, but invocation volume is the
larger aggregate issue.

## Compile versus link replay

Three retained, self-contained primary C inputs were replayed sequentially with
`cc -std=c11 -arch arm64 FLAGS -c INPUT -o OBJECT`, then
`cc OBJECT -o EXECUTABLE`. Each setting had one warmup and five measured samples;
no executable was run. Flags were the CLI's `-O0 -g` and `-O3`. Starting/ending
one-minute loads were 2.95/2.81. These phase medians are focused experiments,
not a full-suite extrapolation or a proposed production flag change.

| Input                              | C bytes | C flags  | Compile median seconds | Link median seconds |
| ---------------------------------- | ------: | -------- | ---------------------: | ------------------: |
| Smallest captured successful input |      88 | `-O0 -g` |                 0.0190 |              0.0320 |
| Same input                         |      88 | `-O3`    |                 0.0197 |              0.0320 |
| Runtime Unicode fixture            | 224,920 | `-O0 -g` |                 0.1267 |              0.0325 |
| Same input                         | 224,920 | `-O3`    |                 0.8497 |              0.0330 |
| Recursive-map fixture              | 123,051 | `-O0 -g` |                 0.0756 |              0.0323 |
| Same input                         | 123,051 | `-O3`    |                 0.4364 |              0.0327 |

The smallest input hash was
`dfe50750dcbd41744388a6e60dfaf3a3c01a43c194a1f1b1c172d5c936db4e8e`.
The Unicode input hash was
`ba2fb91e67c54e2a058f20847f88772388a366ba3ce3adc002d688b36722cac6`;
the recursive-map input hash was
`8af516321da76569a950859bfcfcddc1978e9d9703b5ff6a96b63265b770bbf1`.
Even an almost empty program pays roughly 51 milliseconds for two separate
compiler/linker driver invocations. Larger release fixtures spend their extra
time in compilation, not linking. Real combined `cc` calls have a different
process structure, so multiplying replay medians by all 2,651 calls is not a
valid prediction of achievable suite savings.

## Invocation reduction — October 8, 2026

The bounds-failure matrices in `tests/index_failures.rs` and three numeric-cast
failure matrices in `tests/conformance.rs` now compile runtime-selected cases
once per matrix in each NC mode. Each case still runs in a fresh process; a
panic cannot prevent subsequent cases from executing. Builds use the actual CLI
`build -d` / `build -r`, preserving `-O0 -g` / `-O3`. There is no compilation
cache. Case-count assertions guard the matrices, and each unguarded case still
passes through the compiler API in both modes before native execution so the
selector cannot hide front-end/optimizer diagnostics.

| Scope                       | Native invocations before | Native invocations after | Failure cases per mode |
| --------------------------- | ------------------------: | -----------------------: | ---------------------: |
| `index_failures`            |                       172 |                       10 |                     85 |
| Three numeric-cast matrices |                        34 |                        6 |                     17 |
| Full `conformance` binary   |                       358 |                      330 |              Unchanged |
| Full suite                  |                     2,651 |                    2,461 |              Unchanged |

The index total includes two unchanged successful-control compilations. All
204 failure executions remain, covering extreme signed/unsigned indices,
every last-index expression, map failures, unrecoverable panics, nonfinite and
out-of-range casts, and negative fractional casts. Runtime argument-count
indices in the grouped bounds cases account for the added selector argument.
Trace/panic checks remain; no production compiler code or fresh-build CLI
artifact/error checks changed.

The full-suite invocation count was verified with prepared Cargo artifacts:

```sh
python3 scripts/profile-native-tests.py --output tmp/native-profile-reduction \
  --samples 1 --test-threads 8
```

The report completed successfully, recording **2,461 native invocations** and
the same **20 expected nonzero native statuses**. The reduction is **190 calls
(7.2%)**, split evenly between debug and release. The new flag counts are 1,218
`-O0` (1,208 with `-g`), 1,203 `-O3`, nine `-O2`, and 31 without explicit `-O`.
All 949 Rust tests passed, as did the separate empty doctest suite. Full default
and no-default-feature test runs, all-target Clippy with warnings denied,
formatting checks, and the release build also passed.

**This is an invocation-count validation, not a budget measurement.** The
single instrumented sample started at one-minute load 8.72, took 142.36 seconds,
and had no live CPU-idle measurement because the terminal sandbox denied
`top`. No low-load acceptance threshold or repeated-sample protocol was applied.
Do not compare that wall time or native CPU total to the controlled baseline as
an optimization speedup/regression. Repeated ready-artifact, low-load,
uninstrumented budget checks were still open at that point; the completed recheck
below supplies those measurements. Raw records and private suite logs
are in ignored `tmp/native-profile-reduction`; default/no-default validation
logs are `tmp/native-reduction-default.log` and
`tmp/native-reduction-no-default.log`.

## Post-reduction budget recheck — October 8, 2026

Source revision: `20108226f32b01d1935d1ab226a268ebde672ad5`, with a clean
worktree. The host and toolchain were unchanged from the baseline above. No
compiler, native flags, fixtures, coverage, or production code changed for this
recheck. The 2,461-invocation count comes from the separate reduction profile;
these budget samples did not wrap or count native compiler calls.

Both feature configurations were prepared with `cargo test --offline --no-run`
(and `--no-default-features` for that configuration) before sampling. Before
each sample, the corresponding `--no-run --message-format=json` command confirmed
that every Cargo compiler artifact was fresh. Preparation was outside the timed
region. Samples ran sequentially, with no concurrent agent build or profiler,
and a 90-second idle interval before each admission check, including the first.

Admission required a one-minute load at most 6 and at least 80% CPU idle in the
second sample of `top -l 2 -s 1 -n 0`. All four samples were admitted on their
first attempt. Ordinary background/editor activity remained; this is a
controlled low-load comparison, not an isolated-machine guarantee. `top` ran
before timing, not during the suite. The sandbox blocks macOS `top`, so the
measurement command ran with explicitly approved unsandboxed access.

| Uninstrumented run               | Tests passed | Wall seconds | Starting one-minute load | Starting live CPU idle |
| -------------------------------- | -----------: | -----------: | -----------------------: | ---------------------: |
| Default sample 1                 |          949 |        74.55 |                     1.88 |                 88.65% |
| Default sample 2                 |          949 |        74.92 |                     3.17 |                 88.56% |
| Default sample 3                 |          949 |        75.11 |                     3.17 |                 88.76% |
| `--no-default-features` sample 1 |          949 |        74.98 |                     4.05 |                 88.91% |

The timed commands were `/usr/bin/time -p cargo test --offline` and
`/usr/bin/time -p cargo test --offline --no-default-features`, using default
libtest concurrency on the eight-CPU host. Cargo overhead and the empty doctest
suite are included. All commands exited successfully; none reported a Cargo
recompilation. Full stdout/stderr was saved privately, never printed uncensored.
After all timing samples, `cargo clippy --offline --all-targets -- -D warnings`,
`cargo fmt --check`, and `cargo build --offline --release` also passed; these
checks did not overlap the budget measurements.

The default median was **74.92 seconds**, with a **0.56-second range**. All four
samples met the 90-second budget, with minimum observed headroom of **14.89
seconds**. The default range is below the earlier 79.46–81.72-second baseline,
but these are sequential observations, not an interleaved before/after study;
they do not isolate the reduction's causal speedup from host variability.
The loaded, instrumented 142.36-second reduction profile remains separate and
is neither a budget failure nor an overhead-adjusted timing estimate.

This completes the post-reduction low-load recheck. The broader reproducibility
task remains open: three default samples and one alternate-feature sample do
not establish a guarantee under background pressure, on other hosts, for clean
builds, or after future fixture growth. Repeat the ready-artifact/admission/
cooldown protocol after further reductions or material coverage changes rather
than treating these results as permanent certification.

Raw records, fresh-artifact messages, CPU snapshots, and private logs are in
ignored `tmp/native-budget-recheck/report.json` and adjacent files. The local
orchestration script is `tmp/recheck-native-budget.py`; it is not a committed
fixture. To reproduce manually, prepare both configurations, then for each
sample confirm fresh artifacts with `--no-run --message-format=json`, wait
90 seconds, inspect the second `top` CPU sample and current one-minute load,
and time the appropriate command above with private stdout/stderr redirection.
Reject inadmissible starts rather than combining them with the low-load table.

## Checked-in budget runner and repeat — October 8, 2026

The ready-artifact procedure is now checked in as
`scripts/recheck-native-budget.py`, with stdlib-only regression tests in
`scripts/test_recheck_native_budget.py`. It replaces the need to reconstruct the
ignored orchestration script described above. Run it from the repository root:

```sh
python3 scripts/recheck-native-budget.py --output tmp/native-budget-repeatable
```

Use a new output directory for each run; existing directories are never
replaced. The default protocol prepares both feature configurations outside
measurement, requires every Cargo artifact to be fresh before each sample,
waits 90 seconds, and admits starts only at one-minute load at most 6 and at
least 80% idle in the second macOS `top` sample. It runs three default samples
and one `--no-default-features` sample with uninstrumented `/usr/bin/time -p`
and default libtest concurrency. `RUST_TEST_THREADS` overrides are rejected.
`top` still requires approved unsandboxed access in the editor environment.

The report records the source revision, worktree status, host/toolchain,
settings, admission attempts, test counts, timings, and completion status.
The test count is discovered from successful results rather than fixed at 949,
and must agree across samples. All test stdout/stderr stays in local logs;
never print those logs uncensored, since builtin tests can expose environment
values. Recompilation, failed tests, stale artifacts, missing measurements,
exhausted admission attempts, or timeouts leave an incomplete report. A
completed run exceeding the budget remains complete but exits unsuccessfully.
Changing admission settings or concurrency is a different comparison protocol,
not interchangeable with the default low-load observations.

This repeat used source revision `25704ecdbd14fc51034a82f088f85d191c8bc655`
with only the new runner and its tests added locally. Compiler code, NC/Rust
fixtures, coverage, native flags, host, and toolchain were unchanged. Samples
ran sequentially without concurrent agent builds or profilers, and every start
was admitted on the first attempt. Preparation and CPU inspection were outside
timing; no timed command recompiled Cargo artifacts.

| Uninstrumented run               | Tests passed | Wall seconds | Starting one-minute load | Starting live CPU idle |
| -------------------------------- | -----------: | -----------: | -----------------------: | ---------------------: |
| Default sample 1                 |          949 |        75.28 |                     2.33 |                 85.13% |
| Default sample 2                 |          949 |        72.84 |                     3.47 |                 80.62% |
| Default sample 3                 |          949 |        74.26 |                     3.04 |                 88.62% |
| `--no-default-features` sample 1 |          949 |        74.11 |                     3.37 |                 86.10% |

The default median was **74.26 seconds**, with a **2.44-second range**.
Minimum observed headroom across all four samples was **14.72 seconds**.
This preserves the previously observed low-load headroom; it is not evidence
of a new compiler speedup because the compiler and fixtures did not change.
Loaded and instrumented observations remain separate, without any overhead
correction. Background pressure, other hosts, clean builds, and future fixture
growth remain unestablished.

All 51 Python script tests passed, including mocked admission, stale-artifact,
timeout/privacy, output-preservation, and complete-over-budget reporting cases.
After timing, Clippy with warnings denied, formatting, and the offline release
build passed. Raw records and private logs are in ignored
`tmp/native-budget-repeatable/report.json` and adjacent files. The runner is
macOS-specific because its live CPU admission uses `top`'s macOS output.

## Remaining follow-up

- Continue prioritizing invocation volume and repeated linking before broad
  compiler reduction. Inspect other runtime-failure and differential helpers for compile-once,
  runtime-selected cases, preserving each failure in a fresh process and both
  NC modes. Fresh compilation is itself part of CLI artifact/error tests; do
  not cache those paths away.
- Compare bounded native concurrency with the same fixtures and preparation
  protocol. More concurrent compiler processes need not improve wall time.
- If targeting individual expensive translation units, investigate generated
  helper/function size in runtime Unicode and recursive-map fixtures. Do not
  remove segmentation, sanitizer, or optimized coverage to meet a time target.
- Recheck uninstrumented low-load repeats after further reductions or material
  coverage changes. The completed post-reduction samples beat 90 seconds, but do
  not establish consistent performance under background pressure, other machines,
  or clean builds. The parent budget task stays open.

Raw reports and private logs remain under ignored `tmp/native-profile-8-repeat`,
`tmp/native-profile-8-settled`, and `tmp/native-replay`; uninstrumented logs and
CPU snapshots use `tmp/native-baseline*` and `tmp/native-no-default*`. They are
local artifacts, not committed portable fixtures. The checked-in runner,
commands, conditions, and this summary provide the repeatable procedure.
