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

## Follow-up, not implemented here

- Prioritize invocation volume and repeated linking before broad compiler
  reduction. Inspect runtime-failure and differential helpers for compile-once,
  runtime-selected cases, preserving each failure in a fresh process and both
  NC modes. Fresh compilation is itself part of CLI artifact/error tests; do
  not cache those paths away.
- Compare bounded native concurrency with the same fixtures and preparation
  protocol. More concurrent compiler processes need not improve wall time.
- If targeting individual expensive translation units, investigate generated
  helper/function size in runtime Unicode and recursive-map fixtures. Do not
  remove segmentation, sanitizer, or optimized coverage to meet a time target.
- Recheck uninstrumented low-load repeats after any change. These runs beat
  90 seconds, but do not establish consistent performance under background
  pressure, other machines, or clean builds. The parent budget task stays open.

Raw reports and private logs remain under ignored `tmp/native-profile-8-repeat`,
`tmp/native-profile-8-settled`, and `tmp/native-replay`; uninstrumented logs and
CPU snapshots use `tmp/native-baseline*` and `tmp/native-no-default*`. They are
local artifacts, not committed portable fixtures. The checked-in runner,
commands, conditions, and this summary provide the repeatable procedure.
