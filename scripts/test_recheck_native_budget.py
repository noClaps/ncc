"""Stdlib-only native budget helper tests; no Cargo or native builds are run."""

import contextlib
import importlib.util
import io
import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("recheck-native-budget.py")
SPEC = importlib.util.spec_from_file_location("recheck_native_budget", SCRIPT)
RECHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RECHECK)


def json_lines(messages):
    return "\n".join(json.dumps(message) for message in messages)


def test_result(passed, failed=0, outcome="ok"):
    return (
        f"test result: {outcome}. {passed} passed; {failed} failed; "
        "0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
    )


class ArgumentTests(unittest.TestCase):
    def test_defaults_and_output_path(self):
        args = RECHECK.parse_args(["--output", "results with spaces/report.json"])
        self.assertIsInstance(args.output, Path)
        self.assertEqual(args.output, Path("results with spaces/report.json"))
        for name, expected in {
            "samples": 3,
            "no_default_samples": 1,
            "cooldown": 90.0,
            "max_load": 6.0,
            "min_idle": 80.0,
            "attempts": 3,
            "timeout": 240.0,
            "budget": 90.0,
        }.items():
            with self.subTest(name=name):
                self.assertEqual(getattr(args, name), expected)

    def test_explicit_values_and_allowed_boundaries(self):
        args = RECHECK.parse_args(
            [
                "--output",
                "report.json",
                "--samples",
                "1",
                "--no-default-samples",
                "0",
                "--cooldown",
                "0",
                "--max-load",
                "0.01",
                "--min-idle",
                "0",
                "--attempts",
                "1",
                "--timeout",
                "0.25",
                "--budget",
                "0.5",
            ]
        )
        self.assertEqual(
            (args.samples, args.no_default_samples, args.attempts), (1, 0, 1)
        )
        self.assertEqual(
            (args.cooldown, args.max_load, args.min_idle), (0.0, 0.01, 0.0)
        )
        self.assertEqual((args.timeout, args.budget), (0.25, 0.5))
        for option, value, name, expected in [
            ("--no-default-samples", "2", "no_default_samples", 2),
            ("--cooldown", "0.5", "cooldown", 0.5),
            ("--min-idle", "100", "min_idle", 100.0),
            ("--min-idle", "80.5", "min_idle", 80.5),
        ]:
            with self.subTest(option=option, value=value):
                args = RECHECK.parse_args(["--output", "report.json", option, value])
                self.assertEqual(getattr(args, name), expected)

    def test_argv_defaults_to_process_arguments(self):
        with mock.patch.object(sys, "argv", [str(SCRIPT), "--output", "report.json"]):
            self.assertEqual(RECHECK.parse_args().output, Path("report.json"))

    def assert_rejected(self, flags):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                RECHECK.parse_args(flags)
        self.assertEqual(error.exception.code, 2)

    def test_output_is_required(self):
        self.assert_rejected([])
        self.assert_rejected(["--samples", "1"])
        self.assert_rejected(["--output"])

    def test_integer_bounds_and_invalid_numbers(self):
        for option in ["--samples", "--no-default-samples", "--attempts"]:
            invalid = ["-1", "1.5", "bad", "nan", "inf", "-inf"]
            if option != "--no-default-samples":
                invalid.append("0")
            for value in invalid:
                with self.subTest(option=option, value=value):
                    self.assert_rejected(["--output", "unused", f"{option}={value}"])

    def test_float_bounds_and_nonfinite_numbers(self):
        for option in [
            "--cooldown",
            "--max-load",
            "--min-idle",
            "--timeout",
            "--budget",
        ]:
            invalid = ["-0.01", "bad", "nan", "inf", "-inf"]
            if option in ["--max-load", "--timeout", "--budget"]:
                invalid.extend(["0", "-0.0"])
            if option == "--min-idle":
                invalid.append("100.01")
            for value in invalid:
                with self.subTest(option=option, value=value):
                    self.assert_rejected(["--output", "unused", f"{option}={value}"])


class FreshArtifactTests(unittest.TestCase):
    def test_all_compiler_artifacts_must_be_fresh(self):
        messages = [
            {"reason": "compiler-artifact", "fresh": True},
            {"reason": "compiler-artifact", "fresh": True, "executable": None},
            {"reason": "build-finished", "success": True, "fresh": False},
            {"reason": "compiler-message", "fresh": False},
        ]
        self.assertIs(RECHECK.fresh_artifacts(json_lines(messages)), True)

    def test_at_least_one_artifact_is_required(self):
        for text in ["", json_lines([{"reason": "build-finished", "success": True}])]:
            with self.subTest(text=text):
                self.assertIs(RECHECK.fresh_artifacts(text), False)

    def test_fresh_requires_literal_true_on_every_artifact(self):
        fresh = {"reason": "compiler-artifact", "fresh": True}
        for value in [False, None, 0, 1, "true"]:
            stale = {"reason": "compiler-artifact", "fresh": value}
            for messages in [[stale, fresh], [fresh, stale]]:
                with self.subTest(messages=messages):
                    self.assertIs(RECHECK.fresh_artifacts(json_lines(messages)), False)
        self.assertIs(
            RECHECK.fresh_artifacts(
                json_lines([fresh, {"reason": "compiler-artifact"}])
            ),
            False,
        )

    def test_malformed_json_raises_even_after_stale_artifact(self):
        for text in [
            "not json\n",
            '{"reason": "compiler-artifact",',
            json_lines([{"reason": "compiler-artifact", "fresh": False}]) + "\nbad",
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                RECHECK.fresh_artifacts(text)


class LiveIdleTests(unittest.TestCase):
    def test_returns_second_idle_sample_not_first(self):
        text = (
            "Processes: 500 total\n"
            "CPU usage: 10.00% user, 5.00% sys, 85.00% idle\n"
            "CPU usage: 1.25% user, 0.25% sys, 98.50% idle\n"
        )
        result = RECHECK.live_idle(text)
        self.assertIsInstance(result, float)
        self.assertEqual(result, 98.5)

    def test_idle_endpoints(self):
        for value in ["0", "100"]:
            with self.subTest(value=value):
                self.assertEqual(
                    RECHECK.live_idle(f"50% idle\n{value}% idle\n"), float(value)
                )

    def test_exactly_two_samples_are_required(self):
        for text in [
            "",
            "CPU usage: 10% user\n",
            "90% idle\n",
            "90% idle\n91% idle\n92% idle\n",
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                RECHECK.live_idle(text)

    def test_out_of_range_or_nonfinite_idle_is_rejected(self):
        for value in ["-0.1", "100.1", "nan", "inf", "-inf"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                RECHECK.live_idle(f"90% idle\n{value}% idle\n")


class MeasurementTests(unittest.TestCase):
    def test_summary_sums_tests_and_uses_last_real_line(self):
        text = (
            "real 999.0\n"
            + test_result(12)
            + test_result(3)
            + test_result(0)
            + "finished in 123.0s\nnot real 500\nreal 12.5\nuser 9.0\nsys 1.0\n"
        )
        result = RECHECK.measurement(text, 0, 90.0)
        self.assertEqual(
            result,
            {
                "status": 0,
                "wall_seconds": 12.5,
                "tests_passed": 15,
                "cargo_recompiled": False,
                "valid": True,
                "within_budget": True,
            },
        )

    def test_budget_boundary_and_over_budget_remain_valid(self):
        for wall, within in [(89.99, True), (90.0, True), (90.01, False)]:
            with self.subTest(wall=wall):
                result = RECHECK.measurement(test_result(1) + f"real {wall}\n", 0, 90.0)
                self.assertIs(result["valid"], True)
                self.assertIs(result["within_budget"], within)

    def test_nonzero_status_invalidates_measurement(self):
        for status in [1, 2, -9, 124]:
            with self.subTest(status=status):
                result = RECHECK.measurement(
                    test_result(1) + "real 1.0\n", status, 90.0
                )
                self.assertEqual(result["status"], status)
                self.assertIs(result["valid"], False)
                self.assertIs(result["within_budget"], False)

    def test_compilation_invalidates_otherwise_successful_measurement(self):
        text = "   Compiling ncc v0.1.0\n" + test_result(2) + "real 1.0\n"
        result = RECHECK.measurement(text, 0, 90.0)
        self.assertIs(result["cargo_recompiled"], True)
        self.assertIs(result["valid"], False)
        self.assertIs(result["within_budget"], False)

    def test_no_passing_tests_invalidates_measurement(self):
        for text in ["real 1.0\n", test_result(0) + "real 1.0\n"]:
            with self.subTest(text=text):
                result = RECHECK.measurement(text, 0, 90.0)
                self.assertEqual(result["tests_passed"], 0)
                self.assertIs(result["valid"], False)
                self.assertIs(result["within_budget"], False)

    def test_failed_result_invalidates_even_with_passing_results_and_zero_status(self):
        text = (
            test_result(2, failed=1, outcome="FAILED") + test_result(5) + "real 1.0\n"
        )
        result = RECHECK.measurement(text, 0, 90.0)
        self.assertIs(result["valid"], False)
        self.assertIs(result["within_budget"], False)

    def test_nonpositive_and_nonfinite_wall_times_are_invalid(self):
        for value in ["0", "-0.0", "-1.5", "nan", "inf", "-inf"]:
            with self.subTest(value=value):
                result = RECHECK.measurement(
                    test_result(1) + f"real {value}\n", 0, 90.0
                )
                if value == "nan":
                    self.assertTrue(math.isnan(result["wall_seconds"]))
                else:
                    self.assertEqual(result["wall_seconds"], float(value))
                self.assertIs(result["valid"], False)
                self.assertIs(result["within_budget"], False)

    def test_missing_real_line_raises(self):
        for text in [
            "",
            test_result(1),
            test_result(1) + "not real 1.0\nuser 1.0\nsys 0.5\n",
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                RECHECK.measurement(text, 0, 90.0)


class OrchestrationTests(unittest.TestCase):
    def setUp(self):
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        directory = stack.enter_context(tempfile.TemporaryDirectory())
        self.output = Path(directory).resolve() / "report"
        self.console = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
        self.stderr = stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
        stack.enter_context(mock.patch.dict(RECHECK.os.environ, {}, clear=True))
        self.sleep = stack.enter_context(mock.patch.object(RECHECK.time, "sleep"))
        self.load = stack.enter_context(
            mock.patch.object(RECHECK.os, "getloadavg", return_value=(1.0, 2.0, 3.0))
        )
        stack.enter_context(mock.patch.object(RECHECK.os, "cpu_count", return_value=8))
        self.fresh = True
        self.idle = 95.0
        self.wall = 12.5
        self.timed_error = None
        self.run = stack.enter_context(
            mock.patch.object(RECHECK.subprocess, "run", side_effect=self.fake_run)
        )

    def fake_run(self, command, **kwargs):
        if command[0] == "/usr/bin/time":
            if self.timed_error is not None:
                raise self.timed_error
            kwargs["stdout"].write(test_result(7) + f"real {self.wall}\n")
            return subprocess.CompletedProcess(command, 0)
        if command[0] == "top":
            text = f"10% idle\n{self.idle}% idle\n"
        elif "--message-format=json" in command:
            text = json_lines([{"reason": "compiler-artifact", "fresh": self.fresh}])
        elif command[0] == "git":
            text = "fixture-revision\n" if "rev-parse" in command else ""
        else:
            text = "fixture metadata\n"
        return subprocess.CompletedProcess(command, 0, stdout=text, stderr="")

    def main(self, *flags):
        return RECHECK.main(
            [
                "--output",
                str(self.output),
                "--samples",
                "1",
                "--no-default-samples",
                "1",
                "--cooldown",
                "0.5",
                "--attempts",
                "2",
                "--timeout",
                "10",
                *flags,
            ]
        )

    def report(self):
        return json.loads((self.output / "report.json").read_text())

    def timed_calls(self):
        return [
            call
            for call in self.run.call_args_list
            if call.args[0][0] == "/usr/bin/time"
        ]

    def test_completed_report_and_both_feature_modes(self):
        self.assertEqual(self.main(), 0)
        report = self.report()
        self.assertIs(report["complete"], True)
        self.assertIs(report["within_budget"], True)
        self.assertNotIn("error", report)
        self.assertEqual(report["revision"], "fixture-revision")
        self.assertEqual(report["test_threads"], "libtest default")
        self.assertEqual(report["logical_cpus"], 8)
        self.assertEqual(
            [sample["label"] for sample in report["samples"]],
            ["default-1", "no-default-1"],
        )
        for sample in report["samples"]:
            self.assertIs(sample["artifacts_fresh"], True)
            self.assertIs(sample["valid"], True)
            self.assertIs(sample["within_budget"], True)
            self.assertEqual(sample["tests_passed"], 7)
            self.assertEqual(sample["wall_seconds"], 12.5)
            self.assertEqual(len(sample["attempts"]), 1)
            self.assertIs(sample["attempts"][0]["accepted"], True)
        commands = [call.args[0] for call in self.run.call_args_list]
        self.assertIn(["cargo", "test", "--offline", "--no-run"], commands)
        self.assertIn(
            ["cargo", "test", "--offline", "--no-default-features", "--no-run"],
            commands,
        )
        self.assertEqual(
            [call.args[0] for call in self.timed_calls()],
            [
                ["/usr/bin/time", "-p", "cargo", "test", "--offline"],
                [
                    "/usr/bin/time",
                    "-p",
                    "cargo",
                    "test",
                    "--offline",
                    "--no-default-features",
                ],
            ],
        )
        self.assertEqual(self.sleep.call_args_list, [mock.call(0.5), mock.call(0.5)])
        for call in self.timed_calls():
            self.assertEqual(call.kwargs["cwd"], RECHECK.ROOT)
            self.assertEqual(call.kwargs["timeout"], 10.0)
            self.assertEqual(call.kwargs["stderr"], subprocess.STDOUT)
        self.assertNotIn("test result:", self.console.getvalue())

    def test_over_budget_exits_one_but_report_remains_complete(self):
        self.wall = 90.01
        self.assertEqual(self.main(), 1)
        report = self.report()
        self.assertIs(report["complete"], True)
        self.assertIs(report["within_budget"], False)
        self.assertNotIn("error", report)
        self.assertEqual(len(self.timed_calls()), 2)
        for sample in report["samples"]:
            self.assertIs(sample["valid"], True)
            self.assertIs(sample["within_budget"], False)

    def test_admission_exhaustion_persists_incomplete_report_without_timing(self):
        for load, idle in [(6.01, 95.0), (1.0, 79.99)]:
            with self.subTest(load=load, idle=idle):
                self.output = self.output.parent / f"rejected-{load}-{idle}"
                self.load.return_value = (load, 2.0, 3.0)
                self.idle = idle
                self.run.reset_mock()
                self.sleep.reset_mock()
                self.assertEqual(self.main(), 1)
                report = self.report()
                self.assertIs(report["complete"], False)
                self.assertIn("no low-load admission", report["error"])
                self.assertEqual(len(report["samples"]), 1)
                sample = report["samples"][0]
                self.assertIs(sample["artifacts_fresh"], True)
                self.assertEqual(len(sample["attempts"]), 2)
                self.assertTrue(
                    all(not state["accepted"] for state in sample["attempts"])
                )
                self.assertNotIn("wall_seconds", sample)
                self.assertEqual(self.timed_calls(), [])
                self.assertFalse((self.output / "default-1.log").exists())
                self.assertEqual(
                    self.sleep.call_args_list, [mock.call(0.5), mock.call(0.5)]
                )

    def test_admit_retries_and_accepts_threshold_boundaries(self):
        self.output.mkdir()
        args = RECHECK.parse_args(
            ["--output", str(self.output), "--attempts", "3", "--cooldown", "0.5"]
        )
        sample = {"label": "default-1", "attempts": []}
        report = {"complete": False, "samples": [sample]}
        self.load.side_effect = [(6.01, 2.0, 3.0), (6.0, 2.0, 3.0)]
        self.idle = 80.0
        RECHECK.admit(args, report, sample)
        self.assertEqual(
            [state["accepted"] for state in self.report()["samples"][0]["attempts"]],
            [False, True],
        )
        self.assertEqual(self.sleep.call_args_list, [mock.call(0.5), mock.call(0.5)])
        self.assertEqual(
            [call.args[0] for call in self.run.call_args_list],
            [["top", "-l", "2", "-s", "1", "-n", "0"]] * 2,
        )

    def test_stale_artifacts_stop_run_sample_before_admission_or_timing(self):
        self.output.mkdir()
        args = RECHECK.parse_args(["--output", str(self.output)])
        report = {"complete": False, "samples": []}
        self.fresh = False
        with self.assertRaisesRegex(ValueError, "refusing to time a build"):
            RECHECK.run_sample(args, report, "default-1", [])
        saved = self.report()
        self.assertIs(saved["complete"], False)
        self.assertIs(saved["samples"][0]["artifacts_fresh"], False)
        self.assertEqual(saved["samples"][0]["attempts"], [])
        self.run.assert_called_once_with(
            ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
            cwd=RECHECK.ROOT,
            capture_output=True,
            text=True,
            timeout=240,
            check=False,
        )
        self.sleep.assert_not_called()
        self.assertFalse((self.output / "default-1.log").exists())

    def test_timeout_is_sanitized_in_report_and_console(self):
        secret = "PRIVATE_ENVIRONMENT_VALUE"
        self.timed_error = subprocess.TimeoutExpired(
            ["/usr/bin/time", secret], 10, output=secret, stderr=secret
        )
        self.assertEqual(self.main(), 1)
        report = self.report()
        self.assertIs(report["complete"], False)
        self.assertEqual(
            report["error"], "command timed out after 10.0s; see private logs"
        )
        self.assertEqual(len(self.timed_calls()), 1)
        self.assertNotIn(secret, (self.output / "report.json").read_text())
        self.assertNotIn(secret, self.console.getvalue())
        self.assertNotIn(secret, self.stderr.getvalue())
        self.assertNotIn("wall_seconds", report["samples"][0])

    def test_rust_test_threads_is_rejected_before_preparation(self):
        for value in ["1", ""]:
            with self.subTest(value=value):
                self.output = self.output.parent / f"threads-{value or 'empty'}"
                self.run.reset_mock()
                with mock.patch.dict(RECHECK.os.environ, {"RUST_TEST_THREADS": value}):
                    self.assertEqual(self.main(), 1)
                report = self.report()
                self.assertIs(report["complete"], False)
                self.assertIn("unset RUST_TEST_THREADS", report["error"])
                self.assertEqual(report["samples"], [])
                self.assertTrue(
                    all(call.args[0][0] == "git" for call in self.run.call_args_list)
                )
                self.sleep.assert_not_called()

    def test_existing_output_is_not_overwritten(self):
        self.output.mkdir()
        original = b'{"existing": "keep exactly"}\n'
        report_path = self.output / "report.json"
        report_path.write_bytes(original)
        with self.assertRaises(FileExistsError):
            self.main()
        self.assertEqual(report_path.read_bytes(), original)
        self.assertEqual(list(self.output.iterdir()), [report_path])
        self.run.assert_not_called()
        self.sleep.assert_not_called()


if __name__ == "__main__":
    unittest.main()
