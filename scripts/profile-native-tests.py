#!/usr/bin/env python3
"""Profile native C builds in the full test suite using only Python's stdlib."""

import argparse
import datetime
import hashlib
import json
import os
import platform
import resource
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def positive_int(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def positive_float(value):
    number = float(value)
    if not 0 < number < float("inf"):
        raise argparse.ArgumentTypeError("must be finite and positive")
    return number


def probe(command):
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
        return {
            "command": command,
            "status": result.returncode,
            "stdout": result.stdout.strip(),
            "stderr": result.stderr.strip(),
        }
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "error": str(error)}


def host_state():
    return {
        "load_average": list(os.getloadavg()),
        "cpu_snapshot": probe(["top", "-l", "2", "-s", "1", "-n", "0"])
        if platform.system() == "Darwin"
        else None,
    }


def source_info(arguments, directory):
    sources = []
    for argument in arguments:
        path = Path(argument)
        if path.suffix != ".c" or not path.is_file():
            continue
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        info = {
            "path": str(path),
            "bytes": len(data),
            "sha256": digest,
            "unicode_tables": b"nc_unicode_ranges" in data,
            "includes": [
                line.decode("utf-8", errors="replace")
                for line in data.splitlines()
                if line.startswith(b"#include")
            ],
        }
        if os.environ.get("NC_PROFILE_KEEP_C") == "1":
            saved = directory / "sources" / (digest + ".c")
            # Each process has its own temporary file; publishing is atomic.
            temporary = saved.with_suffix(f".{os.getpid()}.tmp")
            temporary.write_bytes(data)
            temporary.replace(saved)
            info["saved"] = str(saved)
        sources.append(info)
    return sources


def wrap_cc(arguments):
    directory = Path(os.environ["NC_PROFILE_DIRECTORY"])
    sources = source_info(arguments, directory)
    command = [os.environ["NC_PROFILE_CC"], *arguments]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter_ns()
    status = subprocess.run(command, check=False).returncode
    elapsed = time.perf_counter_ns() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    record = {
        "suite": os.environ["NC_PROFILE_SUITE"],
        "sample": int(os.environ["NC_PROFILE_SAMPLE"]),
        "command": command,
        "cwd": os.getcwd(),
        "status": status,
        "elapsed_ns": elapsed,
        "user_seconds": after.ru_utime - before.ru_utime,
        "system_seconds": after.ru_stime - before.ru_stime,
        "optimization": next(
            (arg for arg in reversed(arguments) if arg.startswith("-O")), "default"
        ),
        "debug_info": "-g" in arguments,
        "operation": "compile_only" if "-c" in arguments else "compile_link",
        "sources": sources,
    }
    (directory / "records" / f"{os.getpid()}-{time.time_ns()}.json").write_text(
        json.dumps(record) + "\n", encoding="utf-8"
    )
    return status if status >= 0 else 128 - status


def discover_tests(output):
    artifacts = {}
    fresh = []
    for line in output.splitlines():
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact":
            continue
        fresh.append(message["fresh"])
        if not message.get("profile", {}).get("test") or not message.get("executable"):
            continue
        target = message["target"]
        # Use the executable as identity; lib and bin can share a target name.
        executable = message["executable"]
        artifacts[executable] = {
            "name": target["name"],
            "kind": target["kind"],
            "executable": executable,
        }
    if not artifacts:
        raise ValueError("Cargo reported no test executables")
    return sorted(
        artifacts.values(), key=lambda item: (item["kind"], item["name"])
    ), all(fresh)


def summarize(records):
    groups = {}
    for record in records:
        key = (
            record["sample"],
            record["suite"],
            record["optimization"],
            record["debug_info"],
            record["operation"],
        )
        groups.setdefault(key, []).append(record)
    result = []
    for key, group in sorted(groups.items()):
        durations = [item["elapsed_ns"] / 1e9 for item in group]
        result.append(
            {
                "sample": key[0],
                "suite": key[1],
                "optimization": key[2],
                "debug_info": key[3],
                "operation": key[4],
                "count": len(group),
                "wall_sum_seconds": sum(durations),
                "median_seconds": statistics.median(durations),
                "max_seconds": max(durations),
                "cpu_sum_seconds": sum(
                    item["user_seconds"] + item["system_seconds"] for item in group
                ),
                "failed_invocations": sum(item["status"] != 0 for item in group),
                "source_bytes": sum(
                    source["bytes"] for item in group for source in item["sources"]
                ),
            }
        )
    return result


def save_report(directory, report):
    records = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted((directory / "records").glob("*.json"))
    ]
    report["native"] = records
    report["summary"] = summarize(records)
    (directory / "report.json").write_text(
        json.dumps(report, indent=2) + "\n", encoding="utf-8"
    )


def timed_run(command, log, environment, timeout):
    start = time.perf_counter()
    with log.open("wb") as output:
        result = subprocess.run(
            command,
            cwd=ROOT,
            env=environment,
            stdout=output,
            stderr=subprocess.STDOUT,
            check=False,
            timeout=timeout,
        )
    return {
        "command": command,
        "status": result.returncode,
        "wall_seconds": time.perf_counter() - start,
        "log": str(log),
    }


def measure(args, directory, report):
    build_command = ["cargo", "test", "--offline", "--no-run", "--message-format=json"]
    if args.no_default_features:
        build_command.append("--no-default-features")
    start = time.perf_counter()
    build = subprocess.run(
        build_command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=args.timeout,
    )
    (directory / "cargo-build.jsonl").write_text(build.stdout, encoding="utf-8")
    (directory / "cargo-build.stderr").write_text(build.stderr, encoding="utf-8")
    report["preparation"] = {
        "command": build_command,
        "status": build.returncode,
        "wall_seconds": time.perf_counter() - start,
    }
    if build.returncode:
        raise ValueError("Cargo preparation failed; see cargo-build.stderr")
    tests, fresh = discover_tests(build.stdout)
    report["preparation"]["all_artifacts_fresh"] = fresh
    report["tests"] = tests
    for sample in range(1, args.samples + 1):
        if sample > 1:
            time.sleep(args.cooldown)
        state = host_state()
        if args.max_load is not None and state["load_average"][0] > args.max_load:
            report["rejected_host_state"] = state
            raise ValueError("one-minute load exceeds --max-load; no sample started")
        sample_report = {"sample": sample, "before": state, "suites": []}
        report["samples"].append(sample_report)
        environment = os.environ.copy()
        environment.update(
            {
                "PATH": str(directory / "bin")
                + os.pathsep
                + environment.get("PATH", ""),
                "NC_PROFILE_DIRECTORY": str(directory),
                "NC_PROFILE_CC": report["cc"],
                "NC_PROFILE_SAMPLE": str(sample),
                "NC_PROFILE_KEEP_C": "1" if args.keep_c_sources else "0",
            }
        )
        start = time.perf_counter()
        for index, test in enumerate(tests):
            suite = ":".join(test["kind"]) + ":" + test["name"]
            environment["NC_PROFILE_SUITE"] = suite
            result = timed_run(
                [test["executable"], f"--test-threads={args.test_threads}"],
                directory / f"sample-{sample}-{index}-{test['name']}.log",
                environment,
                args.timeout,
            )
            result["suite"] = suite
            sample_report["suites"].append(result)
            print(
                f"sample {sample} {suite}: {result['wall_seconds']:.2f}s "
                f"(status {result['status']})",
                flush=True,
            )
            if result["status"]:
                raise ValueError(f"test binary failed: {suite}; see its private log")
        sample_report["wall_seconds"] = time.perf_counter() - start
        sample_report["after"] = host_state()
        save_report(directory, report)
    # Rust doctests are not emitted as standalone Cargo test artifacts. Run them
    # separately, without the wrapper, and do not silently include their time.
    command = ["cargo", "test", "--offline", "--doc"]
    if args.no_default_features:
        command.append("--no-default-features")
    report["doctests"] = timed_run(
        command, directory / "doctests.log", os.environ.copy(), args.timeout
    )
    if report["doctests"]["status"]:
        raise ValueError("doctests failed; see doctests.log")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="new directory for report, per-invocation records and private logs",
    )
    parser.add_argument("--samples", type=positive_int, default=3)
    parser.add_argument(
        "--test-threads", type=positive_int, default=os.cpu_count() or 1
    )
    parser.add_argument(
        "--cooldown",
        type=positive_int,
        default=90,
        help="idle seconds between samples, excluded from timings (default: 90)",
    )
    parser.add_argument(
        "--timeout",
        type=positive_int,
        default=600,
        help="maximum seconds per build/test binary (default: 600)",
    )
    parser.add_argument(
        "--max-load",
        type=positive_float,
        help="reject a sample if the initial one-minute load exceeds this value",
    )
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument(
        "--keep-c-sources",
        action="store_true",
        help="retain primary C inputs (may contain embedded private data)",
    )
    args = parser.parse_args(argv)
    cc = shutil.which("cc")
    if cc is None:
        parser.error("cc is not on PATH")
    directory = args.output.resolve()
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    for child in ["bin", "records", "sources"]:
        (directory / child).mkdir()
    wrapper = directory / "bin" / "cc"
    wrapper.write_text(
        f"#!{sys.executable}\nimport runpy, sys\n"
        "sys.argv.insert(1, '--wrap-cc')\n"
        f"runpy.run_path({str(Path(__file__).resolve())!r}, run_name='__main__')\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    report = {
        "schema_version": 1,
        "started_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "arguments": vars(args) | {"output": str(directory)},
        "cc": cc,
        "cc_version": probe([cc, "--version"]),
        "rustc_version": probe(["rustc", "--version"]),
        "cargo_version": probe(["cargo", "--version"]),
        "python_version": sys.version,
        "platform": platform.platform(),
        "cpu_count": os.cpu_count(),
        "cpu": probe(["sysctl", "-n", "machdep.cpu.brand_string"]),
        "git_revision": probe(["git", "--no-pager", "rev-parse", "HEAD"]),
        "git_status": probe(["git", "--no-optional-locks", "status", "--porcelain"]),
        "samples": [],
    }
    try:
        measure(args, directory, report)
        report["complete"] = True
        return 0
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        report["complete"] = False
        report["error"] = str(error)
        print(f"profiling failed: {error}", file=sys.stderr)
        return 1
    finally:
        save_report(directory, report)
        print(f"Report: {directory / 'report.json'}", flush=True)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--wrap-cc"]:
        sys.exit(wrap_cc(sys.argv[2:]))
    sys.exit(main())
