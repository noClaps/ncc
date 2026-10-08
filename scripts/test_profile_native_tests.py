"""Stdlib-only profiler tests; no Cargo or native builds are performed."""

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

SCRIPT = Path(__file__).with_name("profile-native-tests.py")
SPEC = importlib.util.spec_from_file_location("profile_native_tests", SCRIPT)
PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROFILE)


def artifact(
    name="suite", kind="test", executable="/tests/suite", fresh=True, test=True
):
    return {
        "reason": "compiler-artifact",
        "target": {"name": name, "kind": [kind]},
        "executable": executable,
        "fresh": fresh,
        "profile": {"test": test},
    }


def json_lines(messages):
    return "\n".join(json.dumps(message) for message in messages) + "\n"


class DiscoveryTests(unittest.TestCase):
    def test_test_executables_sorted_and_deduplicated_by_path(self):
        binary = artifact("shared", "bin", "/tests/bin")
        library = artifact("shared", "lib", "/tests/lib")
        suite = artifact()
        tests, fresh = PROFILE.discover_tests(
            json_lines(
                [
                    suite,
                    library,
                    binary,
                    suite,
                    artifact("production", executable="/bin/ncc", test=False),
                    artifact("dependency", executable=None),
                    {"reason": "build-finished", "success": True},
                ]
            )
        )
        self.assertEqual(
            tests,
            [
                {"name": "shared", "kind": ["bin"], "executable": "/tests/bin"},
                {"name": "shared", "kind": ["lib"], "executable": "/tests/lib"},
                {"name": "suite", "kind": ["test"], "executable": "/tests/suite"},
            ],
        )
        self.assertTrue(fresh)

    def test_freshness_includes_non_test_artifacts(self):
        for changed in [
            artifact(fresh=False),
            artifact(test=False, fresh=False),
            artifact(executable=None, fresh=False),
        ]:
            with self.subTest(changed=changed):
                tests, fresh = PROFILE.discover_tests(json_lines([artifact(), changed]))
                self.assertEqual(len(tests), 1)
                self.assertFalse(fresh)

    def test_no_test_artifacts(self):
        for messages in [
            [],
            [{"reason": "build-finished"}],
            [artifact(test=False)],
            [artifact(executable=None)],
        ]:
            with self.subTest(messages=messages):
                with self.assertRaisesRegex(ValueError, "no test executables"):
                    PROFILE.discover_tests(json_lines(messages) if messages else "")

    def test_invalid_json_is_rejected(self):
        with self.assertRaises(json.JSONDecodeError):
            PROFILE.discover_tests("not json\n")


class SummaryTests(unittest.TestCase):
    @staticmethod
    def record(**changes):
        return {
            "sample": 1,
            "suite": "test:suite",
            "optimization": "-O2",
            "debug_info": False,
            "operation": "compile_link",
            "elapsed_ns": 1_000_000_000,
            "user_seconds": 0.5,
            "system_seconds": 0.25,
            "status": 0,
            "sources": [{"bytes": 10}],
        } | changes

    def test_durations_cpu_failures_and_all_source_bytes(self):
        records = [
            self.record(elapsed_ns=500_000_000, sources=[]),
            self.record(
                elapsed_ns=1_500_000_000,
                status=2,
                sources=[{"bytes": 10}, {"bytes": 20}],
            ),
            self.record(elapsed_ns=3_000_000_000, status=-9),
            self.record(elapsed_ns=7_000_000_000),
        ]
        self.assertEqual(
            PROFILE.summarize(records),
            [
                {
                    "sample": 1,
                    "suite": "test:suite",
                    "optimization": "-O2",
                    "debug_info": False,
                    "operation": "compile_link",
                    "count": 4,
                    "wall_sum_seconds": 12.0,
                    "median_seconds": 2.25,
                    "max_seconds": 7.0,
                    "cpu_sum_seconds": 3.0,
                    "failed_invocations": 2,
                    "source_bytes": 50,
                }
            ],
        )

    def test_every_group_dimension_and_sorted_order(self):
        for key, value in [
            ("sample", 2),
            ("suite", "test:z"),
            ("optimization", "-O3"),
            ("debug_info", True),
            ("operation", "compile_only"),
        ]:
            with self.subTest(key=key):
                records = [self.record(**{key: value}), self.record()]
                groups = PROFILE.summarize(records)
                self.assertEqual(len(groups), 2)
                self.assertEqual(
                    [group[key] for group in groups],
                    sorted([records[0][key], records[1][key]]),
                )
                self.assertEqual([group["count"] for group in groups], [1, 1])

    def test_empty_records(self):
        self.assertEqual(PROFILE.summarize([]), [])


class SourceInfoTests(unittest.TestCase):
    def test_exact_bytes_hash_includes_and_unicode_marker_without_retention(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "input.c"
            data = (
                '#include <stdio.h>\n#include "local.h"\n// nc_unicode_ranges: \u03bb\n'
            ).encode("utf-8") + b"\xff\n"
            source.write_bytes(data)
            ignored = directory / "input.h"
            ignored.write_bytes(data)
            with mock.patch.dict(os.environ, {"NC_PROFILE_KEEP_C": "0"}):
                info = PROFILE.source_info(
                    [
                        "-O2",
                        str(source),
                        str(ignored),
                        str(directory / "missing.c"),
                        str(directory),
                    ],
                    directory,
                )
            self.assertEqual(
                info,
                [
                    {
                        "path": str(source),
                        "bytes": len(data),
                        "sha256": hashlib.sha256(data).hexdigest(),
                        "unicode_tables": True,
                        "includes": ["#include <stdio.h>", '#include "local.h"'],
                    }
                ],
            )
            self.assertEqual(set(directory.iterdir()), {source, ignored})

    def test_optional_retention_is_content_addressed_and_leaves_no_temporaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            saved_directory = directory / "sources"
            saved_directory.mkdir()
            source = directory / "plain.c"
            data = b"int main(void) { return 0; }\n"
            source.write_bytes(data)
            with mock.patch.dict(os.environ, {"NC_PROFILE_KEEP_C": "1"}):
                first = PROFILE.source_info([str(source)], directory)
                second = PROFILE.source_info([str(source)], directory)
            saved = saved_directory / (hashlib.sha256(data).hexdigest() + ".c")
            self.assertEqual(first, second)
            self.assertFalse(first[0]["unicode_tables"])
            self.assertEqual(first[0]["includes"], [])
            self.assertEqual(first[0]["saved"], str(saved))
            self.assertEqual(saved.read_bytes(), data)
            self.assertEqual(list(saved_directory.iterdir()), [saved])


class WrapperTests(unittest.TestCase):
    def test_mocked_resource_timing_status_and_record(self):
        for status, expected in [(0, 0), (7, 7), (-9, 137)]:
            with (
                self.subTest(status=status),
                tempfile.TemporaryDirectory() as temporary,
            ):
                directory = Path(temporary)
                (directory / "records").mkdir()
                environment = {
                    "NC_PROFILE_DIRECTORY": temporary,
                    "NC_PROFILE_CC": "/fake/cc",
                    "NC_PROFILE_SUITE": "test:fixture",
                    "NC_PROFILE_SAMPLE": "3",
                }
                arguments = [
                    "-O0",
                    "-g",
                    "-c",
                    "-O2",
                    "input with spaces.c",
                    "-o",
                    "output with spaces.o",
                ]
                sources = [{"path": "input with spaces.c", "bytes": 15}]
                usage = [
                    SimpleNamespace(ru_utime=10.0, ru_stime=2.0),
                    SimpleNamespace(ru_utime=10.5, ru_stime=2.25),
                ]
                with (
                    mock.patch.dict(os.environ, environment),
                    mock.patch.object(
                        PROFILE, "source_info", return_value=sources
                    ) as scan,
                    mock.patch.object(
                        PROFILE.subprocess,
                        "run",
                        return_value=SimpleNamespace(returncode=status),
                    ) as run,
                    mock.patch.object(
                        PROFILE.resource, "getrusage", side_effect=usage
                    ) as resources,
                    mock.patch.object(
                        PROFILE.time, "perf_counter_ns", side_effect=[100, 400]
                    ),
                    mock.patch.object(PROFILE.time, "time_ns", return_value=123),
                    mock.patch.object(PROFILE.os, "getpid", return_value=456),
                ):
                    self.assertEqual(PROFILE.wrap_cc(arguments), expected)
                scan.assert_called_once_with(arguments, directory)
                # No output redirection: the compiler inherits stdout and stderr.
                run.assert_called_once_with(["/fake/cc", *arguments], check=False)
                self.assertEqual(
                    resources.call_args_list,
                    [
                        mock.call(PROFILE.resource.RUSAGE_CHILDREN),
                        mock.call(PROFILE.resource.RUSAGE_CHILDREN),
                    ],
                )
                record_path = directory / "records" / "456-123.json"
                self.assertEqual(
                    json.loads(record_path.read_text(encoding="utf-8")),
                    {
                        "suite": "test:fixture",
                        "sample": 3,
                        "command": ["/fake/cc", *arguments],
                        "cwd": os.getcwd(),
                        "status": status,
                        "elapsed_ns": 300,
                        "user_seconds": 0.5,
                        "system_seconds": 0.25,
                        "optimization": "-O2",
                        "debug_info": True,
                        "operation": "compile_only",
                        "sources": sources,
                    },
                )
                self.assertEqual(list((directory / "records").iterdir()), [record_path])

    def test_python_fake_compiler_preserves_arguments_streams_and_exit_status(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "records").mkdir()
            fake = directory / "fake compiler.py"
            fake.write_text(
                "import json, sys\n"
                "print(json.dumps(sys.argv[1:]), flush=True)\n"
                "print('compiler stderr', file=sys.stderr, flush=True)\n"
                "sys.exit(int(sys.argv[1]))\n",
                encoding="utf-8",
            )
            source = directory / "input with spaces.c"
            source.write_bytes(b"/* fixture, not compiled */\n")
            environment = os.environ | {
                "NC_PROFILE_DIRECTORY": temporary,
                "NC_PROFILE_CC": sys.executable,
                "NC_PROFILE_SUITE": "bin:fake",
                "NC_PROFILE_SAMPLE": "1",
                "NC_PROFILE_KEEP_C": "0",
            }
            for status in [0, 7]:
                with self.subTest(status=status):
                    arguments = [
                        str(fake),
                        str(status),
                        str(source),
                        "-DNAME=a b",
                        "-o",
                        "output with spaces",
                    ]
                    result = subprocess.run(
                        [sys.executable, str(SCRIPT), "--wrap-cc", *arguments],
                        cwd=directory,
                        env=environment,
                        capture_output=True,
                        text=True,
                        check=False,
                        timeout=30,
                    )
                    self.assertEqual(result.returncode, status)
                    self.assertEqual(result.stdout, json.dumps(arguments[1:]) + "\n")
                    self.assertEqual(result.stderr, "compiler stderr\n")
            records = [
                json.loads(path.read_text(encoding="utf-8"))
                for path in (directory / "records").glob("*.json")
            ]
            self.assertEqual(sorted(record["status"] for record in records), [0, 7])
            for record in records:
                self.assertEqual(
                    record["command"],
                    [
                        sys.executable,
                        str(fake),
                        str(record["status"]),
                        str(source),
                        "-DNAME=a b",
                        "-o",
                        "output with spaces",
                    ],
                )
                self.assertEqual(record["cwd"], str(directory.resolve()))
                self.assertEqual((record["suite"], record["sample"]), ("bin:fake", 1))
                self.assertEqual(record["optimization"], "default")
                self.assertFalse(record["debug_info"])
                self.assertEqual(record["operation"], "compile_link")
                self.assertGreater(record["elapsed_ns"], 0)
                self.assertGreaterEqual(record["user_seconds"], 0)
                self.assertGreaterEqual(record["system_seconds"], 0)
                self.assertEqual(
                    record["sources"][0]["sha256"],
                    hashlib.sha256(source.read_bytes()).hexdigest(),
                )
                self.assertNotIn("saved", record["sources"][0])


class ValidationTests(unittest.TestCase):
    def test_positive_numeric_validators(self):
        self.assertEqual(PROFILE.positive_int("3"), 3)
        self.assertEqual(PROFILE.positive_float("0.25"), 0.25)
        for value in ["0", "-1"]:
            with self.subTest(value=value):
                with self.assertRaises(argparse.ArgumentTypeError):
                    PROFILE.positive_int(value)
        for value in ["0", "-0.1", "nan", "inf", "-inf"]:
            with self.subTest(value=value):
                with self.assertRaises(argparse.ArgumentTypeError):
                    PROFILE.positive_float(value)

    def test_cli_rejects_invalid_values_before_any_build_or_probe(self):
        for flags in [
            ["--samples", "0"],
            ["--test-threads", "-1"],
            ["--timeout", "bad"],
            ["--max-load", "nan"],
            ["--max-load", "inf"],
        ]:
            with (
                self.subTest(flags=flags),
                mock.patch.object(PROFILE.subprocess, "run") as run,
                mock.patch.object(PROFILE.shutil, "which") as which,
                contextlib.redirect_stderr(io.StringIO()),
            ):
                with self.assertRaises(SystemExit) as error:
                    PROFILE.main(["--output", "unused", *flags])
                self.assertEqual(error.exception.code, 2)
                run.assert_not_called()
                which.assert_not_called()


if __name__ == "__main__":
    unittest.main()
