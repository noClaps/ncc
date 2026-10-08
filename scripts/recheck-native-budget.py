#!/usr/bin/env python3
"""Recheck the ready-artifact suite budget on macOS; test output stays private."""

import argparse
import datetime
import json
import math
import os
import re
import subprocess
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


def measurement(text, status, budget):
    times = re.findall(r"^real\s+(\S+)\s*$", text, re.MULTILINE)
    if not times:
        raise ValueError("missing wall time")
    wall = float(times[-1])
    passed = sum(
        int(value) for value in re.findall(r"test result: ok\. ([0-9]+) passed", text)
    )
    recompiled = "Compiling " in text
    valid = (
        status == 0
        and not recompiled
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


def capture(command, output, stem, timeout):
    # Never include subprocess output in exceptions: tests may print environment values.
    result = subprocess.run(
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
        snapshot = capture(
            ["top", "-l", "2", "-s", "1", "-n", "0"],
            args.output,
            f"{sample['label']}-before-{attempt}",
            args.timeout,
        )
        state = {
            "time": datetime.datetime.now().astimezone().isoformat(),
            "load_average": list(os.getloadavg()),
            "live_idle_percent": live_idle(snapshot),
        }
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
    sample = {"label": label, "command": command, "attempts": []}
    report["samples"].append(sample)
    save(args.output, report)
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
    with (args.output / f"{label}.log").open("w") as log:
        result = subprocess.run(
            ["/usr/bin/time", "-p", *command],
            cwd=ROOT,
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout=args.timeout,
        )
    sample.update(
        measurement(
            (args.output / f"{label}.log").read_text(), result.returncode, args.budget
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


def main(argv=None):
    args = parse_args(argv)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "complete": False,
        "samples": [],
        "settings": {
            key: value for key, value in vars(args).items() if key != "output"
        },
        "test_threads": "libtest default",
        "logical_cpus": os.cpu_count(),
        "instrumented": False,
        "clean_build": False,
    }
    save(args.output, report)
    try:
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
            if samples:
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
