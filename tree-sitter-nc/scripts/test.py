#!/usr/bin/env python3
"""Generate and validate NC syntax without executing any NC programs."""

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

GRAMMAR = Path(__file__).resolve().parents[1]
ROOT = GRAMMAR.parent


def run(*args):
    subprocess.run(args, cwd=GRAMMAR, env=environment, check=True, timeout=120)


if not shutil.which("tree-sitter"):
    sys.exit("Install Tree-sitter CLI 0.27.0 or newer to run grammar tests.")
version = subprocess.check_output(["tree-sitter", "--version"], text=True)
match = re.search(r"(\d+)\.(\d+)\.(\d+)", version)
if not match or tuple(map(int, match.groups())) < (0, 27, 0):
    sys.exit("Tree-sitter CLI 0.27.0 or newer is required.")

environment = os.environ.copy()
environment["XDG_CACHE_HOME"] = str(ROOT / "target" / "tree-sitter-cache")
build = ROOT / "target" / "tree-sitter-nc"
build.mkdir(parents=True, exist_ok=True)
extension = (
    ".dll" if os.name == "nt" else ".dylib" if sys.platform == "darwin" else ".so"
)
library = str(build / ("nc" + extension))

run("tree-sitter", "generate", "--js-runtime", "native", "--abi", "15")
run("tree-sitter", "build", "--output", library)
run("tree-sitter", "test", "--lib-path", library, "--lang-name", "nc")
examples = sorted((ROOT / "nc-tests").glob("*.nc"))
run(
    "tree-sitter",
    "parse",
    "--lib-path",
    library,
    "--lang-name",
    "nc",
    "--quiet",
    "--timeout",
    "5000000",
    *map(str, examples),
)
run(
    "tree-sitter",
    "query",
    "--lib-path",
    library,
    "--lang-name",
    "nc",
    "--quiet",
    "queries/highlights.scm",
    *map(str, examples),
)
# Compare incrementally edited trees with fresh parses, especially scanner state.
for source, position, removed, inserted in [
    ('fn stop() {\n  return 1\n  @println("ok")\n}\n', 21, 0, "\n"),
    ("value\n(1)\n", 5, 1, " "),
]:
    original = build / "original.nc"
    edited = build / "edited.nc"
    original.write_text(source)
    edited.write_text(source[:position] + inserted + source[position + removed :])
    base = [
        "tree-sitter",
        "parse",
        "--lib-path",
        library,
        "--lang-name",
        "nc",
        "--no-ranges",
        "--timeout",
        "5000000",
    ]
    incremental = subprocess.check_output(
        base + [str(original), "--edits", f"{position} {removed} {inserted}"],
        cwd=GRAMMAR,
        env=environment,
        timeout=120,
    )
    fresh = subprocess.check_output(
        base + [str(edited)],
        cwd=GRAMMAR,
        env=environment,
        timeout=120,
    )
    if incremental != fresh:
        sys.exit("Incremental parse differs from a fresh parse.")
print(f"Corpus, {len(examples)} NC examples, queries, and incremental edits passed.")
