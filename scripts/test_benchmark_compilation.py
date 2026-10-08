"""Stdlib-only runner tests; no Cargo build or native compilation is performed."""

import contextlib
import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("benchmark-compilation.py")
SPEC = importlib.util.spec_from_file_location("benchmark_compilation", SCRIPT)
BENCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCH)


def fixture(args):
    lines = [",".join(BENCH.HEADER)]
    phases = ["nc_compile", "native_c"] if args.native else ["nc_compile"]
    for workload, sizes in [
        ("expression_list", args.sizes),
        ("shared_type_graph", args.depths),
    ]:
        for size in sizes:
            for mode in ["debug", "release"]:
                for phase in phases:
                    for sample in range(args.samples):
                        lines.append(
                            f"{workload},{size},{mode},{phase},{sample},"
                            f"{(sample + 1) * 10},100,200"
                        )
    return "\n".join(lines) + "\n"


class RunnerTests(unittest.TestCase):
    def test_defaults_and_command(self):
        args = BENCH.parse_args([])
        self.assertEqual((args.samples, args.warmup), (7, 2))
        self.assertEqual(args.sizes, [128, 512, 2048])
        self.assertEqual(args.depths, [8, 16, 24])
        self.assertEqual(
            BENCH.benchmark_command("/bench", args),
            [
                "/bench",
                "--samples",
                "7",
                "--warmup",
                "2",
                "--sizes",
                "128,512,2048",
                "--depths",
                "8,16,24",
            ],
        )
        self.assertEqual(
            BENCH.BUILD_COMMAND,
            [
                "cargo",
                "build",
                "--offline",
                "--release",
                "--bench",
                "compilation",
                "--message-format=json",
            ],
        )

    def test_invalid_arguments(self):
        for flags in [
            ["--samples", "0"],
            ["--samples", "bad"],
            ["--warmup", "-1"],
            ["--sizes", ""],
            ["--sizes", "1,"],
            ["--sizes", "1,1"],
            ["--depths", "-1,2"],
            ["--depths", "1.5"],
        ]:
            with self.subTest(flags=flags), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    BENCH.parse_args(flags)
                self.assertEqual(error.exception.code, 2)
        self.assertEqual(BENCH.parse_args(["--warmup", "0"]).warmup, 0)

    def test_artifact_discovery_and_freshness(self):
        artifact = {
            "reason": "compiler-artifact",
            "target": {"name": "compilation", "kind": ["bench"]},
            "executable": "/target/release/deps/compilation-abc",
            "fresh": True,
        }
        unrelated = {
            "reason": "compiler-artifact",
            "target": {"name": "ncc", "kind": ["lib"]},
            "executable": None,
            "fresh": False,
        }
        output = (
            "ignored non-JSON line\n"
            + json.dumps(unrelated)
            + "\n"
            + json.dumps(artifact)
        )
        self.assertEqual(
            BENCH.compilation_artifact(output), (artifact["executable"], True, False)
        )
        for bad in [
            "",
            json.dumps(unrelated),
            json.dumps(artifact) + "\n" + json.dumps(artifact),
        ]:
            with self.subTest(output=bad), self.assertRaises(ValueError):
                BENCH.compilation_artifact(bad)

    def test_summaries_and_native_forwarding(self):
        args = BENCH.parse_args(
            ["--samples", "3", "--sizes", "2", "--depths", "4", "--native"]
        )
        summaries = BENCH.summarize(fixture(args), args)
        self.assertEqual(len(summaries), 8)
        for summary in summaries:
            self.assertEqual(summary["samples"], 3)
            self.assertEqual(summary["median_ns"], 20)
            self.assertEqual(summary["min_ns"], 10)
        self.assertEqual(BENCH.benchmark_command("/bench", args)[-1], "--native")

    def test_invalid_csv(self):
        args = BENCH.parse_args(["--samples", "1", "--sizes", "2", "--depths", "4"])
        raw = fixture(args)
        first_row = raw.splitlines()[1] + "\n"
        for bad in [
            "wrong,header\n",
            raw + first_row,
            raw.replace("nc_compile", "unknown"),
            raw.replace(",10,", ",-1,"),
            raw.replace(",10,", ",bad,"),
            "\n".join(raw.splitlines()[:-1]) + "\n",
            raw + first_row.rstrip() + ",extra\n",
        ]:
            with self.subTest(raw=bad), self.assertRaises(ValueError):
                BENCH.summarize(bad, args)

    def test_prepare_does_not_execute_benchmark(self):
        output = json.dumps(
            {
                "reason": "compiler-artifact",
                "target": {"name": "compilation", "kind": ["bench"]},
                "fresh": True,
                "executable": "/bench",
            }
        ).encode()
        metadata = {"preparation": {"clean_performed": False}}
        with mock.patch.object(
            BENCH.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                BENCH.BUILD_COMMAND, 0, output, b""
            ),
        ) as run:
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(BENCH.prepare(metadata), "/bench")
        run.assert_called_once_with(
            BENCH.BUILD_COMMAND, cwd=BENCH.ROOT, capture_output=True, check=False
        )
        self.assertTrue(metadata["preparation"]["benchmark_fresh"])
        self.assertFalse(metadata["preparation"]["clean_performed"])

    def test_output_and_sidecar_preserve_raw_csv(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.csv"
            flags = [
                "--samples",
                "1",
                "--sizes",
                "2",
                "--depths",
                "4",
                "--output",
                str(output),
            ]
            raw = fixture(BENCH.parse_args(flags)).replace("\n", "\r\n").encode()
            stdout_bytes = io.BytesIO()
            stdout = io.TextIOWrapper(stdout_bytes, encoding="utf-8")
            stderr = io.StringIO()
            metadata = {"host": {}, "preparation": {"clean_performed": False}}
            with (
                mock.patch.object(BENCH, "collect_metadata", return_value=metadata),
                mock.patch.object(BENCH, "prepare", return_value="/bench"),
                mock.patch.object(
                    BENCH.subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess([], 0, raw, b""),
                ) as run,
                contextlib.redirect_stdout(stdout),
                contextlib.redirect_stderr(stderr),
            ):
                self.assertEqual(BENCH.main(flags), 0)
            self.assertEqual(output.read_bytes(), raw)
            self.assertEqual(stdout_bytes.getvalue(), raw)
            sidecar = json.loads(Path(str(output) + ".json").read_text())
            self.assertEqual(len(sidecar["summaries"]), 4)
            self.assertIn("median=", stderr.getvalue())
            run.assert_called_once()

    def test_cli_help_needs_no_build(self):
        result = subprocess.run(
            [BENCH.sys.executable, str(SCRIPT), "--help"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--native", result.stdout)
        self.assertIn("--output", result.stdout)


if __name__ == "__main__":
    unittest.main()
