#!/usr/bin/env python3
"""Recheck native suite budgets on Linux/macOS; test output stays private."""

import argparse
import datetime
import json
import math
import os
import platform
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def number(value):
    try:
        result = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected a finite number") from error
    if not math.isfinite(result):
        raise argparse.ArgumentTypeError("expected a finite number")
    return result


def positive(value):
    result = number(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return result


def nonnegative(value):
    result = number(value)
    if result < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return result


def count(value):
    result = int(value)
    if result < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return result


def positive_count(value):
    result = count(value)
    if not result:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return result


def percent(value):
    result = nonnegative(value)
    if result > 100:
        raise argparse.ArgumentTypeError("must be at most 100")
    return result


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=positive_count, default=3)
    parser.add_argument("--no-default-samples", type=count, default=1)
    parser.add_argument("--cooldown", type=nonnegative, default=90)
    parser.add_argument("--max-load", type=positive, default=6)
    parser.add_argument("--min-idle", type=percent, default=80)
    parser.add_argument("--attempts", type=positive_count, default=3)
    parser.add_argument("--timeout", type=positive, default=240)
    parser.add_argument("--budget", type=positive, default=90)
    parser.add_argument(
        "--clean-build",
        action="store_true",
        help="time compilation and tests in a fresh Cargo target per sample (not a cold-cache guarantee)",
    )
    parser.add_argument(
        "--pressure-workers",
        type=count,
        default=0,
        help="uncalibrated CPU-bound Python workers around each timed suite",
    )
    return parser.parse_args(argv)


def fresh_artifacts(text):
    messages = [json.loads(line) for line in text.splitlines() if line.strip()]
    artifacts = [item for item in messages if item.get("reason") == "compiler-artifact"]
    return bool(artifacts) and all(item.get("fresh") is True for item in artifacts)


def live_idle(text):
    samples = re.findall(r"(\S+)% idle", text)
    if len(samples) != 2:
        raise ValueError("expected exactly two live CPU samples")
    values = [float(value) for value in samples]
    if not all(math.isfinite(value) and 0 <= value <= 100 for value in values):
        raise ValueError("invalid CPU idle percentage")
    return values[-1]


def measurement(text, status, budget, clean_build=False):
    times = re.findall(r"^real\s+(\S+)\s*$", text, re.MULTILINE)
    if not times:
        raise ValueError("missing wall time")
    wall = float(times[-1])
    passed = sum(
        int(value) for value in re.findall(r"test result: ok\. ([0-9]+) passed", text)
    )
    recompiled = "Compiling " in text
    compilation_evidence = bool(
        re.search(r"^\s*Compiling \S+ v\S+", text, re.MULTILINE)
    )
    valid = (
        status == 0
        and (compilation_evidence if clean_build else not recompiled)
        and passed > 0
        and "test result: FAILED" not in text
        and math.isfinite(wall)
        and wall > 0
    )
    return {
        "status": status,
        "wall_seconds": wall,
        "tests_passed": passed,
        "cargo_recompiled": recompiled,
        "valid": valid,
        "within_budget": valid and wall <= budget,
    }


def save(output, report):
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


def stop_group(process):
    # Kill the whole session even if its leader has already exited.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def managed_run(
    command, *, timeout, check=False, capture_output=False, text=False, **kwargs
):
    if capture_output:
        kwargs.update(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    process = subprocess.Popen(command, start_new_session=True, text=text, **kwargs)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        result = subprocess.CompletedProcess(
            command, process.returncode, stdout, stderr
        )
        if check:
            result.check_returncode()
        return result
    finally:
        # Covers timeouts, interrupts, and children surviving a successful leader.
        try:
            stop_group(process)
        finally:
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None:
                    stream.close()


def linux_cpu(text):
    lines = text.splitlines()
    fields = lines[0].split() if lines else []
    if not fields or fields[0] != "cpu" or not 5 <= len(fields) <= 11:
        raise ValueError("malformed /proc/stat aggregate CPU counters")
    if any(not re.fullmatch(r"[0-9]+", field) for field in fields[1:]):
        raise ValueError("malformed /proc/stat CPU counter")
    counters = [int(field) for field in fields[1:]]
    return counters + [0] * (10 - len(counters))


def linux_idle(before, after):
    first, second = linux_cpu(before), linux_cpu(after)
    # guest and guest_nice are already included in user/nice; don't double count.
    deltas = [end - start for start, end in zip(first[:8], second[:8])]
    if any(delta < 0 for delta in deltas) or sum(deltas) <= 0:
        raise ValueError("invalid /proc/stat CPU delta")
    return 100 * (deltas[3] + deltas[4]) / sum(deltas)


def snapshot(args, stem):
    if platform.system() == "Linux":
        before = Path("/proc/stat").read_text()
        time.sleep(1)
        after = Path("/proc/stat").read_text()
        (args.output / f"{stem}.stdout").write_text(before + "\n" + after)
        idle = linux_idle(before, after)
    elif platform.system() == "Darwin":
        idle = live_idle(
            capture(
                ["top", "-l", "2", "-s", "1", "-n", "0"],
                args.output,
                stem,
                args.timeout,
            )
        )
    else:
        raise ValueError("only Linux and Darwin are supported")
    return {
        "time": datetime.datetime.now().astimezone().isoformat(),
        "load_average": list(os.getloadavg()),
        "live_idle_percent": idle,
    }


def capture(command, output, stem, timeout):
    # Never include subprocess output in exceptions: tests may print environment values.
    result = managed_run(
        command, cwd=ROOT, capture_output=True, text=True, timeout=timeout, check=False
    )
    (output / f"{stem}.stdout").write_text(result.stdout)
    (output / f"{stem}.stderr").write_text(result.stderr)
    if result.returncode:
        raise ValueError(
            f"{stem} failed (status {result.returncode}); see private logs"
        )
    return result.stdout


def admit(args, report, sample):
    for attempt in range(1, args.attempts + 1):
        print(
            f"{sample['label']}: cooldown {args.cooldown}s before admission {attempt}",
            flush=True,
        )
        time.sleep(args.cooldown)
        state = snapshot(args, f"{sample['label']}-before-{attempt}")
        state["accepted"] = (
            state["load_average"][0] <= args.max_load
            and state["live_idle_percent"] >= args.min_idle
        )
        sample["attempts"].append(state)
        save(args.output, report)
        print(
            f"load={state['load_average'][0]:.2f}, idle={state['live_idle_percent']:.2f}%, accepted={state['accepted']}",
            flush=True,
        )
        if state["accepted"]:
            return
    raise ValueError("no low-load admission; incomplete report retained")


def run_sample(args, report, label, flags):
    command = ["cargo", "test", "--offline", *flags]
    target = None
    if args.clean_build:
        target = args.output / f"{label}-target"
        target.mkdir(mode=0o700)
        command.extend(["--target-dir", str(target)])
    sample = {
        "label": label,
        "command": command,
        "attempts": [],
        "clean_build": args.clean_build,
        "target_dir": str(target) if target else None,
    }
    report["samples"].append(sample)
    save(args.output, report)
    if not args.clean_build:
        ready = capture(
            [*command, "--no-run", "--message-format=json"],
            args.output,
            f"{label}-ready",
            args.timeout,
        )
        sample["artifacts_fresh"] = fresh_artifacts(ready)
        save(args.output, report)
        if not sample["artifacts_fresh"]:
            raise ValueError("artifacts were not ready; refusing to time a build")
    admit(args, report, sample)
    result = timed_suite(args, report, sample, command)
    sample.update(
        measurement(
            (args.output / f"{label}.log").read_text(),
            result.returncode,
            args.budget,
            args.clean_build,
        )
    )
    sample["end_load_average"] = list(os.getloadavg())
    save(args.output, report)
    print(
        f"{label}: status={sample['status']}, wall={sample['wall_seconds']:.2f}s, passed={sample['tests_passed']}, within_budget={sample['within_budget']}",
        flush=True,
    )
    if not sample["valid"]:
        raise ValueError("invalid measurement; incomplete report retained")
    if sample["tests_passed"] != report["samples"][0]["tests_passed"]:
        raise ValueError(
            "test count changed between samples; incomplete report retained"
        )


WORKLOAD = (
    "value = 1\nwhile True:\n    value = (value * 1664525 + 1013904223) & 0xffffffff\n"
)


def pressure_snapshot(args, sample, workers, phase):
    state = snapshot(args, f"{sample['label']}-pressure-{phase}")
    state["active_workers"] = sum(worker.poll() is None for worker in workers)
    sample["pressure"][phase] = state
    if state["active_workers"] != args.pressure_workers:
        raise ValueError("pressure worker exited; invalid measurement")


def timed_suite(args, report, sample, command):
    workers = []
    sample["pressure"] = {
        "requested_workers": args.pressure_workers,
        "pids": [],
        "workload": "uncalibrated Python integer arithmetic"
        if args.pressure_workers
        else None,
    }
    try:
        for _ in range(args.pressure_workers):
            worker = subprocess.Popen(
                [sys.executable, "-c", WORKLOAD],
                start_new_session=True,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            workers.append(worker)
            sample["pressure"]["pids"].append(worker.pid)
        if workers:
            time.sleep(0.2)
            pressure_snapshot(args, sample, workers, "before")
            save(args.output, report)
        with (args.output / f"{sample['label']}.log").open("w") as log:
            result = managed_run(
                ["/usr/bin/time", "-p", *command],
                cwd=ROOT,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=args.timeout,
            )
        if workers:
            pressure_snapshot(args, sample, workers, "after")
        return result
    finally:
        for worker in workers:
            stop_group(worker)
        sample["pressure"]["cleaned_up"] = True
        save(args.output, report)


def main(argv=None):
    args = parse_args(argv)
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {
        "complete": False,
        "samples": [],
        "settings": {
            key: value for key, value in vars(args).items() if key != "output"
        },
        "test_threads": "libtest default",
        "logical_cpus": os.cpu_count(),
        "instrumented": False,
        "clean_build": args.clean_build,
        "clean_build_scope": "fresh Cargo target per sample; compiler wrappers, Cargo configuration and OS caches are not disabled",
        "platform": platform.system(),
        "pressure_protocol": {
            "workers": args.pressure_workers,
            "stabilization_seconds": 0.2 if args.pressure_workers else 0,
            "snapshots": "before and after suite, workers alive, outside timed interval",
            "overhead": "startup, stabilization, two live CPU probes and cleanup excluded; workers remain active during probes",
            "calibrated_benchmark": False,
        },
    }
    save(args.output, report)
    try:
        if report["platform"] not in ("Linux", "Darwin"):
            raise ValueError("only Linux and Darwin are supported")
        report["revision"] = capture(
            ["git", "--no-pager", "rev-parse", "HEAD"],
            args.output,
            "revision",
            args.timeout,
        ).strip()
        report["worktree_status"] = capture(
            ["git", "--no-optional-locks", "status", "--short"],
            args.output,
            "worktree",
            args.timeout,
        ).strip()
        # Explicit thread overrides would silently change the comparison protocol.
        if "RUST_TEST_THREADS" in os.environ:
            raise ValueError(
                "unset RUST_TEST_THREADS for default-concurrency measurements"
            )
        if args.clean_build:
            for name in ("CARGO_BUILD_TARGET", "CARGO_BUILD_BUILD_DIR"):
                if name in os.environ:
                    raise ValueError(
                        f"unset {name} for fresh-target clean measurements"
                    )
            report["cargo_target_dir_override"] = (
                "explicit per-sample --target-dir overrides environment/config"
            )
        for name, command in [
            ("host", ["uname", "-a"]),
            ("rust", ["rustc", "--version"]),
            ("cc", ["cc", "--version"]),
        ]:
            report[name] = capture(command, args.output, name, args.timeout).strip()
        for label, flags, samples in [
            ("default", [], args.samples),
            ("no-default", ["--no-default-features"], args.no_default_samples),
        ]:
            if samples and not args.clean_build:
                capture(
                    ["cargo", "test", "--offline", *flags, "--no-run"],
                    args.output,
                    f"prepare-{label}",
                    args.timeout,
                )
        for label, flags, samples in [
            ("default", [], args.samples),
            ("no-default", ["--no-default-features"], args.no_default_samples),
        ]:
            for index in range(1, samples + 1):
                run_sample(args, report, f"{label}-{index}", flags)
        report["complete"] = True
        report["within_budget"] = all(
            sample["within_budget"] for sample in report["samples"]
        )
        save(args.output, report)
        return 0 if report["within_budget"] else 1
    except (OSError, ValueError, subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        report["error"] = (
            f"command timed out after {args.timeout}s; see private logs"
            if isinstance(error, subprocess.TimeoutExpired)
            else "interrupted"
            if isinstance(error, KeyboardInterrupt)
            else str(error)
        )
        save(args.output, report)
        print(report["error"], flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
