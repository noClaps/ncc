#!/usr/bin/env python3
"""Generate and validate NC syntax without executing any NC programs."""

import os
import re
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
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


def cli(*args):
    return subprocess.run(
        ["tree-sitter", *args, "--lib-path", library, "--lang-name", "nc"],
        cwd=GRAMMAR,
        env=environment,
        capture_output=True,
        text=True,
        timeout=120,
    )


def parse_tree(path, edits=()):
    args = ["parse", "--xml", "--timeout", "5000000", str(path)]
    for edit in edits:
        args.extend(["--edits", edit])
    result = cli(*args)
    # A recovered tree deliberately returns status 1. Other failures, including
    # timeouts and missing XML output, must not count as successful recovery.
    start = result.stdout.find("<?xml")
    end = result.stdout.find("</sources>")
    if result.returncode not in (0, 1) or start < 0 or end < 0:
        raise AssertionError(f"Parse failed: {result.stdout}\n{result.stderr}")
    tree = ET.fromstring(result.stdout[start : end + len("</sources>")])
    source_tree = tree.find("source")
    assert source_tree is not None and len(source_tree) == 1, result.stdout
    return source_tree[0], result.returncode


# Every replacement is located in the current source, then converted to UTF-8
# byte offsets/counts for the CLI. Keep all edits in a sequence so later steps
# exercise reuse of already edited (including recovered) trees, not just the base.
edit_cases = [
    (
        "metadata strings versus interpolation",
        'test "茶 {not code ???}" {}\n',
        [
            ("{not code ???}", '{outer {inner} "quotes"}', False),
            ('test "茶', 'test """\n茶', True),
            ('quotes"}"', 'quotes"}\n"""', False),
            ('"quotes"', "'letters'", False),
        ],
    ),
    (
        "generic angle operators",
        'extern "native.c" as native { fn read() Box<int> = "read" }\n',
        [
            ("> =", ">=", True),
            (">=", "> =", False),
            ("<int>", "<Box<int>>", False),
            ("<Box<int>>", "< // 茶\n Box<int>\n>", False),
        ],
    ),
    (
        "comparison and shift operators",
        "while index < 26 { index = index + 1 }\nint bits = value >> 2\n",
        [
            ("index < 26", "index <= 26", False),
            ("index <= 26", "index > 26", False),
            ("value >> 2", "value > 2", False),
            ("value > 2", "value >> 2", False),
            ("index > 26", "index < // comment\n26", False),
        ],
    ),
    (
        "local declaration restrictions",
        "fn outer() { fn local() {} }\n",
        [
            ("fn local", "pub fn local", True),
            ("pub fn local", "fn local", False),
            ("local()", "local<T>()", True),
            ("local<T>()", "local()", False),
        ],
    ),
    (
        "parenthesized function paths",
        "int value = (helpers.identity)<int>(1)\n",
        [
            ("(helpers.identity)", "((helpers.identity))", False),
            ("((helpers.identity))", "factory().identity", False),
            ("factory().identity", "helpers.identity", False),
            ("helpers.identity", "(helpers.identity)", False),
        ],
    ),
    (
        "return/comment boundary",
        'fn stop() {\n  return 1\n  @println("🍪")\n}\n',
        [
            ("return 1", "return\n1", False),
            ("return\n1", "return // café\n1", False),
            ("return // café\n1", "return 1", False),
        ],
    ),
    (
        "break CRLF",
        "for i in [1] {\r\n  break 1\r\n}\r\n",
        [
            ("break 1", "break\r\n1", False),
            ("break\r\n1", "break 2", False),
        ],
    ),
    (
        "postfix newlines",
        "// 🍪\nvalue\n(1)\nitems\n[0]\nrecord\n.field\n",
        [
            ("value\n(1)", "value (1)", False),
            ("items\n[0]", "items[0]", False),
            ("record\n.field", "record.field", False),
            ("value (1)", "value\n(1)", False),
            ("items[0]", "items\n[0]", False),
        ],
    ),
    (
        "UTF-8 strings and interpolation",
        'str text = "café 🍪 {compute(1)}"\n',
        [
            ("café", "茶", False),
            ("🍪", "é🍰", False),
            ("compute(1)", "compute(1 + 2)", False),
            ("{compute(1 + 2)}", r"\{compute(1 + 2)}", False),
            (r"\{compute(1 + 2)}", "{compute(1 + 2)}", False),
            ('}"', "} ", True),
            ("} ", '}"', False),
        ],
    ),
    (
        "escape-decoded brace boundaries",
        'str text = "🍪 {value}"\n',
        [
            ("{value}", r"\u{7b}value}", False),
            (r"\u{7b}value}", r"\\{value}", False),
            (r"\\{value}", r"\u{005c}{value}", False),
            (r"\u{005c}{value}", "{value}", False),
        ],
    ),
    (
        "anonymous return parity",
        "fn work = fn() void! { return }\n",
        [
            ("void!", "!", True),
            ("!", "void!", False),
        ],
    ),
    (
        "empty initializer parity",
        "Empty<int> empty = Empty<int>{}\n",
        [
            ("= Empty<int>", "= Empty", False),
            ("= Empty", "= Empty<int>", False),
            ("{}", "{.value = 1}", False),
        ],
    ),
    (
        "multiline string delimiters",
        'str text = """\n🍪 {value}\n"""\n',
        [
            ("{value}", "{items[0].name}", False),
            ('\n"""\n', '\n""\n', True),
            ('\n""\n', '\n"""\n', False),
        ],
    ),
    (
        "comment toggles",
        "// café 🍪\nint value = compute(1)\nint after = 2\n",
        [
            ("int value", "// int value", False),
            ("// int value", "int value", False),
            ("compute(1)", "compute(1) // 茶", False),
            (" // 茶", "", False),
        ],
    ),
    (
        "nested generic calls",
        "int value = helpers.identity<Box<int>>(item)\n",
        [
            ("Box<int>", "Box<str[]>", False),
            (">>(item)", ">>\n(item)", False),
            (">>\n(item)", ">>\n(item", True),
            (">>\n(item", ">>\n(item)", False),
        ],
    ),
    (
        "generic declaration",
        "fn identity<type T>(T value) T { return value }\n",
        [
            ("type T", "type T, U", False),
            ("T value", "U value", False),
            ("T {", "U[] {", False),
            ("return value", "return [value]", False),
        ],
    ),
    (
        "composite types",
        "type Task = (fn([str]int?, int[]) fut int!)\n",
        [
            ("int[]", "Box<int>[3]", False),
            ("fut int!", "fut (int, str)!", False),
            ("[3]", "[3", True),
            ("[3", "[3]", False),
        ],
    ),
    (
        "postfix damage and repair",
        "// 茶\nint value = factory()(items[0]).field\n",
        [
            (".field", ".", True),
            (".\n", ".field\n", False),
            ("items[0]", "items[", True),
            ("items[", "items[$]", False),
            ("factory()", "factory<int>()", False),
        ],
    ),
    (
        "function body EOF",
        "fn work() int {\n  return compute(1)\n}\n",
        [
            ("\n}\n", "\n", True),
            ("compute(1)\n", "compute(1)\n}\n", False),
            ("compute(1)", "compute(1", True),
            ("compute(1", "compute(1)", False),
        ],
    ),
]
edit_count = 0
for name, source, replacements in edit_cases:
    original = build / "original.nc"
    edited = build / "edited.nc"
    original.write_bytes(source.encode("utf-8"))
    _, status = parse_tree(original)
    assert status == 0, f"{name}: invalid starting fixture"
    edits = []
    for step, (old, new, incomplete) in enumerate(replacements, 1):
        assert source.count(old) == 1, f"{name}, step {step}: ambiguous edit {old!r}"
        position = source.index(old)
        byte_position = len(source[:position].encode("utf-8"))
        removed = len(old.encode("utf-8"))
        edits.append(f"{byte_position} {removed} {new}")
        source = source[:position] + new + source[position + len(old) :]
        edited.write_bytes(source.encode("utf-8"))
        incremental, incremental_status = parse_tree(original, edits)
        fresh, fresh_status = parse_tree(edited)
        context = f"{name}, step {step}: {old!r} -> {new!r}"
        assert incremental_status == fresh_status == int(incomplete), context
        # XML preserves node kinds, fields, token text, and byte-based ranges.
        assert ET.tostring(incremental) == ET.tostring(fresh), (
            f"Incremental parse differs from a fresh parse: {context}"
        )
        edit_count += 1


# Capture spans, not just query validity: overlapping generic/specific captures
# are intentional. Positions are byte columns even after multibyte string text.
def point(source, offset):
    prefix = source[:offset].encode("utf-8")
    return (prefix.count(b"\n"), len(prefix.rsplit(b"\n", 1)[-1]))


def highlight_case(name, source, expected, forbidden=(), incomplete=False):
    path = build / "highlights.nc"
    path.write_bytes(source.encode("utf-8"))
    _, status = parse_tree(path)
    assert status == int(incomplete), f"{name}: unexpected highlighting fixture status"
    result = cli("query", "--captures", "queries/highlights.scm", str(path))
    assert result.returncode == 0, result.stderr
    captures = set()
    for match in re.finditer(
        r"capture:\s*\d+ - ([\w.]+), start: \((\d+), (\d+)\), "
        r"end: \((\d+), (\d+)\)",
        result.stdout,
    ):
        capture, sr, sc, er, ec = match.groups()
        captures.add((capture, (int(sr), int(sc)), (int(er), int(ec))))
    assert captures, f"{name}: could not read query captures: {result.stdout}"
    for required, assertions in [(True, expected), (False, forbidden)]:
        for assertion in assertions:
            capture, text, *occurrences = assertion
            occurrence = occurrences[0] if occurrences else 0
            offsets = [match.start() for match in re.finditer(re.escape(text), source)]
            assert occurrence < len(offsets), f"{name}: missing span {text!r}"
            offset = offsets[occurrence]
            span = (capture, point(source, offset), point(source, offset + len(text)))
            assert (span in captures) == required, (
                f"{name}: {'missing' if required else 'unexpected'} capture {span} "
                f"for {text!r}\n{result.stdout}"
            )
    return len(expected) + len(forbidden)


highlight_count = highlight_case(
    "declaration roles",
    """import { "models.nc" as models }
extern "native.c" as native { fn emit(str text) = "nc_emit" }
pub struct Box<type T> { T value }
enum Status { Ready }
type Count = uint
fn identity(int input) int { return input }
fn callback = fn() {}
Box<int> boxed = Box<int>{.value = 1}
outer: for i in [1] { break :outer }
""",
    [
        ("module", "models", 1),
        ("module", "native", 1),
        ("function", "emit"),
        ("variable.parameter", "text"),
        ("type.parameter", "T"),
        ("type", "Box"),
        ("property", "value"),
        ("property", "value", 1),
        ("type", "Box<int>", 0),
        ("constant", "Ready"),
        ("type", "Count"),
        ("type", "uint"),
        ("function", "identity"),
        ("variable.parameter", "input"),
        ("function", "callback"),
        ("keyword", "pub"),
        ("keyword", "return"),
        ("number", "1"),
        ("label", "outer"),
        ("label", "outer", 1),
    ],
)
highlight_count += highlight_case(
    "UTF-8 interpolation and literals",
    r'''str text = "café 🍪 {helpers.compute(42).field}\n \{literal}"
str multi = """
茶 {other(0xffu)}
"""
@println(text)
int converted = @as(int, 3)
char escaped = '\t'
float special = NaN
bool ready = true
int? absent = none
int last = items[$]
// fn fake() { @println("not code") }
''',
    [
        ("string", '"café 🍪 {helpers.compute(42).field}\\n \\{literal}"'),
        ("string", '"""\n茶 {other(0xffu)}\n"""'),
        ("function.call", "compute"),
        ("function.call", "other"),
        ("property", "field"),
        ("string.escape", r"\n"),
        ("string.escape", r"\{"),
        ("string.escape", r"\t"),
        ("character", r"'\t'"),
        ("function.builtin", "@println"),
        ("function.builtin", "as"),
        ("number", "42"),
        ("number", "0xffu"),
        ("number.float", "NaN"),
        ("boolean", "true"),
        ("constant.builtin", "none"),
        ("constant.builtin", "$"),
        ("operator", "?"),
        ("punctuation.delimiter", "."),
        ("punctuation.special", "{"),
        ("punctuation.special", "}"),
        ("comment", '// fn fake() { @println("not code") }'),
    ],
    [
        ("function", "fake"),
        ("keyword", "fn"),
        ("function.builtin", "@println", 1),
        ("string", '"not code"'),
        ("function.call", "literal"),
        ("variable", "literal"),
        ("punctuation.special", "{", 1),
        ("punctuation.special", "}", 1),
    ],
)
highlight_count += highlight_case(
    "escape-decoded literal braces",
    r"""str text = "\u{7b}literal} \\{alsoLiteral} \u{5C}{thirdLiteral} {real(1)}"
""",
    [
        ("string.escape", r"\u{7b}"),
        ("string.escape", r"\\{"),
        ("string.escape", r"\u{5C}{"),
        ("function.call", "real"),
    ],
    [
        ("variable", "literal"),
        ("variable", "alsoLiteral"),
        ("variable", "thirdLiteral"),
        ("punctuation.special", "{", 0),
        ("punctuation.special", "{", 1),
        ("punctuation.special", "{", 2),
    ],
)
highlight_count += highlight_case(
    "metadata doc comments and parenthesized calls",
    """/// API documentation
// Ordinary comment
import { "{not code ???}" as metadata }
extern "native{junk}.c" as native { fn emit() = "{raw symbol}" }
test "{not code ???}" {}
int result = ((helpers.identity))<int>(1)
fn outer() { fn local() {} }
""",
    [
        ("comment.documentation", "/// API documentation"),
        ("comment", "// Ordinary comment"),
        ("string", '"{not code ???}"'),
        ("string", '"native{junk}.c"'),
        ("string", '"{raw symbol}"'),
        ("function.call", "((helpers.identity))"),
        ("type.builtin", "int", 0),
        ("type.builtin", "int", 1),
        ("function", "outer"),
        ("function", "local"),
    ],
    [
        ("comment.documentation", "// Ordinary comment"),
        ("variable", "not"),
        ("variable", "junk"),
        ("variable", "raw"),
        ("punctuation.special", "{", 1),
        ("punctuation.special", "{", 2),
    ],
)
highlight_count += highlight_case(
    "recovered call and following binding",
    "@println(1\nint after = 2\n",
    [
        ("function.builtin", "@println"),
        ("number", "1"),
        ("type", "int", 1),
        ("variable", "after"),
        ("number", "2"),
    ],
    incomplete=True,
)
if "--compiler-parity" in sys.argv:
    # Build a syntax-only oracle separately from the standalone grammar tests.
    # Imports, @embed, externs, tests, and examples are never executed or loaded.
    compiler_target = build / "compiler"
    subprocess.run(
        ["cargo", "build", "--offline", "--lib", "--target-dir", str(compiler_target)],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=120,
    )
    oracle = build / ("parser-check.exe" if os.name == "nt" else "parser-check")
    subprocess.run(
        [
            "rustc",
            "--edition=2024",
            str(GRAMMAR / "scripts" / "parser-check.rs"),
            "--extern",
            f"ncc={compiler_target / 'debug' / 'libncc.rlib'}",
            "-L",
            f"dependency={compiler_target / 'debug' / 'deps'}",
            "-o",
            str(oracle),
        ],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=120,
    )
    cases = []
    for corpus in sorted((GRAMMAR / "test" / "corpus").glob("*.txt")):
        for match in re.finditer(
            r"^={3,}\n(.*?)\n={3,}\n(.*?)\n-{3,}\n(.*?)(?=^={3,}|\Z)",
            corpus.read_text(),
            re.MULTILINE | re.DOTALL,
        ):
            name, source, expected = match.groups()
            invalid = bool(re.search(r"\((?:ERROR|MISSING|UNEXPECTED)\b", expected))
            cases.append((f"{corpus.name}: {name}", source, not invalid))
    design = (ROOT / "docs" / "design.md").read_text()
    for match in re.finditer(r"^```nc\n(.*?)^```", design, re.MULTILINE | re.DOTALL):
        line = design[: match.start()].count("\n") + 1
        cases.append((f"design.md:{line}", match.group(1), None))
    paths = []
    for index, (_, source, _) in enumerate(cases):
        path = build / f"parity-{index}.nc"
        path.write_bytes(source.encode("utf-8"))
        paths.append(path)
    result = subprocess.run(
        [str(oracle)],
        input="".join(f"{path}\n" for path in paths),
        capture_output=True,
        text=True,
        check=True,
        timeout=120,
    )
    statuses = result.stdout.splitlines()
    assert len(statuses) == len(cases), result.stdout
    accepted_design = rejected_design = 0
    parity_failures = []
    for path, (name, _, expected), status in zip(paths, cases, statuses):
        assert status in ("ok", "error"), status
        accepted = status == "ok"
        if expected is not None:
            if accepted != expected:
                parity_failures.append(f"Compiler/corpus acceptance mismatch: {name}")
        elif not accepted:
            # Design snippets also include templates and intentionally invalid
            # examples. They are not valid-program fixtures for either parser.
            rejected_design += 1
            continue
        else:
            accepted_design += 1
        _, grammar_status = parse_tree(path)
        if (grammar_status == 0) != accepted:
            parity_failures.append(f"Compiler/grammar mismatch: {name}")
    assert not parity_failures, "\n".join(parity_failures)
    print(
        f"Compiler parity: {len(cases) - accepted_design - rejected_design} corpus cases, "
        f"{accepted_design} accepted design snippets; "
        f"{rejected_design} compiler-rejected design snippets excluded."
    )

print(
    f"Corpus, {len(examples)} NC examples, queries, {highlight_count} highlight "
    f"assertions, and {edit_count} incremental edits passed."
)
