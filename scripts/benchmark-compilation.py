#!/usr/bin/env python3
"""Prepare and run the compilation benchmark using only Python's stdlib."""

import argparse
import csv
import datetime
import io
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADER = [
    "workload",
    "size",
    "mode",
    "phase",
    "sample",
    "elapsed_ns",
    "source_bytes",
    "c_bytes",
]
BUILD_COMMAND = [
    "cargo",
    "build",
    "--offline",
    "--release",
    "--bench",
    "compilation",
    "--message-format=json",
]


def positive_int(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected an integer") from error
    if number <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return number


def nonnegative_int(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected an integer") from error
    if number < 0:
        raise argparse.ArgumentTypeError("must be zero or greater")
    return number


def integer_list(value):
    numbers = [positive_int(part.strip()) for part in value.split(",")]
    if len(set(numbers)) != len(numbers):
        raise argparse.ArgumentTypeError("values must be distinct")
    return numbers


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=positive_int, default=7)
    parser.add_argument("--warmup", type=nonnegative_int, default=2)
    parser.add_argument(
        "--sizes",
        type=integer_list,
        default=[128, 512, 2048],
        help="comma-separated expression counts (default: 128,512,2048)",
    )
    parser.add_argument(
        "--depths",
        type=integer_list,
        default=[8, 16, 24],
        help="comma-separated shared-type DAG depths (default: 8,16,24)",
    )
    parser.add_argument(
        "--native",
        action="store_true",
        help="also compile/link emitted C; never run generated programs",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="save raw CSV here and metadata at PATH.json (also emit CSV on stdout)",
    )
    return parser.parse_args(argv)


def probe(command):
    """Unavailable optional metadata must not prevent a benchmark run."""
    try:
        result = subprocess.run(
            command, cwd=ROOT, capture_output=True, text=True, check=False
        )
    except OSError as error:
        return {"command": command, "error": str(error)}
    return {
        "command": command,
        "returncode": result.returncode,
        "stdout": result.stdout.strip(),
        "stderr": result.stderr.strip(),
    }


def load_average():
    try:
        return list(os.getloadavg())
    except (AttributeError, OSError):
        return None


def collect_metadata(args):
    cpu = platform.processor()
    if platform.system() == "Darwin":
        cpu_probe = probe(["sysctl", "-n", "machdep.cpu.brand_string"])
        if cpu_probe.get("returncode") == 0:
            cpu = cpu_probe["stdout"]
    else:
        cpu_probe = None
    status = probe(
        [
            "git",
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ]
    )
    return {
        "schema_version": 1,
        "started_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "repository": str(ROOT),
        "arguments": {
            "samples": args.samples,
            "warmup": args.warmup,
            "sizes": args.sizes,
            "depths": args.depths,
            "native": args.native,
            "output": str(args.output) if args.output else None,
        },
        "invocation": sys.argv,
        "host": {
            "os": platform.platform(),
            "machine": platform.machine(),
            "cpu": cpu,
            "cpu_probe": cpu_probe,
            "logical_cpus": os.cpu_count(),
            "load_average_before_preparation": load_average(),
        },
        "tools": {
            "python": sys.version,
            "cargo": probe(["cargo", "--version"]),
            "rustc": probe(["rustc", "--version", "--verbose"]),
            "cc": probe(["cc", "--version"]),
        },
        "git": {
            "revision": probe(["git", "--no-pager", "rev-parse", "HEAD"]),
            "status": status,
            "dirty": bool(status["stdout"]) if status.get("returncode") == 0 else None,
        },
        "preparation": {
            "command": BUILD_COMMAND,
            "clean_performed": False,
            "initial_cache_state": "unknown; no clean build claimed",
        },
    }


def compilation_artifact(stdout):
    artifacts = []
    all_fresh = True
    for line in stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            not isinstance(message, dict)
            or message.get("reason") != "compiler-artifact"
        ):
            continue
        all_fresh = all_fresh and message.get("fresh") is True
        target = message.get("target", {})
        if (
            target.get("name") == "compilation"
            and "bench" in target.get("kind", [])
            and message.get("executable")
        ):
            artifacts.append(message)
    if len(artifacts) != 1:
        raise ValueError(
            "expected exactly one compilation bench compiler-artifact executable"
        )
    artifact = artifacts[0]
    return artifact["executable"], artifact.get("fresh"), all_fresh


def prepare(metadata):
    start = time.perf_counter_ns()
    result = subprocess.run(BUILD_COMMAND, cwd=ROOT, capture_output=True, check=False)
    elapsed = time.perf_counter_ns() - start
    metadata["preparation"].update(elapsed_ns=elapsed, returncode=result.returncode)
    sys.stderr.write(result.stderr.decode("utf-8", errors="replace"))
    print(
        f"Cargo preparation: {elapsed / 1e9:.3f} s (not a clean-build measurement)",
        file=sys.stderr,
    )
    if result.returncode:
        # Cargo JSON diagnostics are on stdout; show them on failure, not in CSV.
        sys.stderr.write(result.stdout.decode("utf-8", errors="replace"))
        raise ValueError(f"Cargo preparation failed with exit code {result.returncode}")
    executable, fresh, all_fresh = compilation_artifact(result.stdout.decode("utf-8"))
    metadata["preparation"].update(
        executable=executable,
        benchmark_fresh=fresh,
        all_reported_artifacts_fresh=all_fresh,
    )
    return executable


def benchmark_command(executable, args):
    command = [
        executable,
        "--samples",
        str(args.samples),
        "--warmup",
        str(args.warmup),
        "--sizes",
        ",".join(map(str, args.sizes)),
        "--depths",
        ",".join(map(str, args.depths)),
    ]
    if args.native:
        command.append("--native")
    return command


def summarize(raw, args):
    reader = csv.DictReader(io.StringIO(raw))
    if reader.fieldnames != HEADER:
        raise ValueError(f"benchmark CSV header must be {','.join(HEADER)}")
    phases = ["nc_compile", "native_c"] if args.native else ["nc_compile"]
    expected = {
        (workload, size, mode, phase)
        for workload, sizes in [
            ("expression_list", args.sizes),
            ("shared_type_graph", args.depths),
        ]
        for size in sizes
        for mode in ["debug", "release"]
        for phase in phases
    }
    groups = {}
    for row in reader:
        if None in row or any(value is None for value in row.values()):
            raise ValueError("malformed benchmark CSV row")
        numbers = {
            key: int(row[key])
            for key in HEADER
            if key not in {"workload", "mode", "phase"}
        }
        if any(value < 0 for value in numbers.values()):
            raise ValueError("benchmark CSV contains a negative count or duration")
        key = (row["workload"], numbers["size"], row["mode"], row["phase"])
        if key not in expected:
            raise ValueError(f"unexpected benchmark group: {key}")
        samples = groups.setdefault(key, {})
        if numbers["sample"] in samples:
            raise ValueError(f"duplicate sample in benchmark group: {key}")
        samples[numbers["sample"]] = numbers["elapsed_ns"]
    if set(groups) != expected or any(
        len(values) != args.samples for values in groups.values()
    ):
        raise ValueError(
            "benchmark CSV does not contain the requested groups/sample counts"
        )
    return [
        {
            "workload": key[0],
            "size": key[1],
            "mode": key[2],
            "phase": key[3],
            "samples": len(values),
            "median_ns": statistics.median(values.values()),
            "min_ns": min(values.values()),
        }
        for key, values in sorted(groups.items())
    ]


def main(argv=None):
    args = parse_args(argv)
    try:
        metadata = collect_metadata(args)
        executable = prepare(metadata)
        command = benchmark_command(executable, args)
        metadata["benchmark"] = {"command": command}
        metadata["host"]["load_average_before_samples"] = load_average()
        start = time.perf_counter_ns()
        result = subprocess.run(command, cwd=ROOT, capture_output=True, check=False)
        metadata["benchmark"].update(
            elapsed_ns=time.perf_counter_ns() - start, returncode=result.returncode
        )
        metadata["host"]["load_average_after_samples"] = load_average()
        sys.stderr.write(result.stderr.decode("utf-8", errors="replace"))
        if result.returncode:
            raise ValueError(f"benchmark failed with exit code {result.returncode}")
        summaries = summarize(result.stdout.decode("utf-8"), args)
        metadata["summaries"] = summaries
        metadata["finished_at_utc"] = datetime.datetime.now(
            datetime.timezone.utc
        ).isoformat()
        encoded_metadata = json.dumps(metadata, indent=2) + "\n"
        if args.output:
            args.output.write_bytes(result.stdout)
            Path(str(args.output) + ".json").write_text(
                encoded_metadata, encoding="utf-8"
            )
        sys.stdout.flush()
        sys.stdout.buffer.write(result.stdout)
        sys.stdout.buffer.flush()
        for summary in summaries:
            print(
                "{workload} size={size} {mode} {phase}: median={median_ns} ns "
                "min={min_ns} ns ({samples} samples)".format(**summary),
                file=sys.stderr,
            )
        print("Metadata:\n" + encoded_metadata, file=sys.stderr, end="")
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
