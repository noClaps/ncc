use std::{fs, path::Path, process::Command};

#[derive(Clone, Copy)]
enum Tables {
    Absent,
    Present,
}

fn check(source: &str, expected: &str, tables: Tables, argument: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("unicode.nc");
    fs::write(&input, source).unwrap();
    for (mode, release) in [("-d", false), ("-r", true)] {
        let c = ncc::compile_source_with_options(source, Path::new("unicode.nc"), release)
            .unwrap_or_else(|error| panic!("{mode}: {error}\n{source}"));
        for marker in ["nc_unicode_ranges", "nc_grapheme_next"] {
            assert_eq!(
                c.contains(marker),
                matches!(tables, Tables::Present),
                "{mode}: unexpected dependency {marker}\n{source}"
            );
        }
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("run")
            .arg(mode)
            .arg(&input)
            .args(["--", argument])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "{mode}"
        );
    }
}

#[test]
fn composite_printing_flattens_bytes_without_unicode_tables() {
    check(
        r#"
str text = @args()[1]
int n = @as(int, @args().len)
str[] texts = [text, "\u{0}尾"]
(int, str) pair = (n, text)
struct Row { str text int count }
Row row = Row{.text = text, .count = n}
enum Choice { Empty Some(str, int) }
Choice choice = Choice.Some(text, n)
[int]str mapping = [n: text]
@println(texts, "|", pair, "|", row, "|", choice, "|", mapping)
"#,
        "[\"é\", \"\0尾\"]|(2, é)|Row{.text = é, .count = 2}|Choice.Some(\"é\", 2)|[2: é]\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn inactive_and_nested_payload_printing_avoids_unicode_tables() {
    check(
        r#"
int n = @as(int, @args().len)
fn number(int n) int! {
    if n { 0 -> { throw "bad\u{0}尾" } _ -> { return n } }
}
int! success = number(n)
int! failure = number(n - 2)
int!? absent = none
int!? present = failure
struct Result { int! value }
enum Choice { Empty Value(int!?) }
@println([success, failure], "|", absent, "|", present)
@println(Result{.value = failure}, "|", Choice.Empty, "|", Choice.Value(present))
"#,
        "[2, error: bad\0尾]|none|error: bad\0尾\nResult{.value = error: bad\0尾}|Choice.Empty|Choice.Value(error: bad\0尾)\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn recursive_output_helpers_avoid_segmentation() {
    check(
        r#"
int n = @as(int, @args().len)
struct Node { int value Node[] children }
Node leaf = Node{.value = n, .children = []}
Node root = Node{.value = n + 1, .children = [leaf]}
enum Tree { Leaf(int) Branch(Tree[]) }
Tree tree = Tree.Branch([Tree.Leaf(n)])
@println(root, "|", tree)
"#,
        "Node{.value = 3, .children = [Node{.value = 2, .children = []}]}|Tree.Branch([Tree.Leaf(2)])\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn byte_array_casts_flatten_unicode_and_nuls_without_segmentation() {
    check(
        r#"
str text = @args()[1]
byte b = @as(byte, @args().len + 126u)
char c = @as(char, b)
@println(@as(byte[], text))
@println(@as(byte[], "\u{0}尾"))
@println(@as(byte[], c))
"#,
        "[195, 169]\n[0, 229, 176, 190]\n[194, 128]\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn single_interpolations_and_empty_concatenations_preserve_evaluation() {
    check(
        r#"
mut int count = @as(int, @args().len)
fn next() int { count = count + 1;@print("called:");return count }
str first = "{next()}"
str second = "" <> @as(str, next())
str third = @as(str, next()) <> ""
@println([first, second, third], ":", count)
"#,
        "called:called:called:[\"3\", \"4\", \"5\"]:5\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn print_argument_snapshots_and_order_do_not_depend_on_segmentation() {
    check(
        r#"
mut int[] values = [@as(int, @args().len)]
fn change() int { values[0] = 9;@print("argument:");return 7 }
@println(values, ":", change(), ":", values)
"#,
        "argument:[2]:7:[9]\n",
        Tables::Absent,
        "é",
    );
}

#[test]
fn output_and_value_helpers_keep_separate_character_boundary_contracts() {
    let mark = "\u{301}";
    for (declaration, value, rendered) in [
        ("char[] value = [mark]", "value", format!("[{mark}]")),
        (
            "enum Mark { Empty Value(char) };Mark value = Mark.Value(mark)",
            "value",
            format!("Mark.Value({mark})"),
        ),
        (
            "struct Marked { char value };Marked value = Marked{.value = mark}",
            "value",
            format!("Marked{{.value = {mark}}}"),
        ),
    ] {
        for print_first in [true, false] {
            let print = format!("@println({value})");
            let conversion = format!("str converted = @as(str, {value})");
            let order = if print_first {
                format!("{print}\n{conversion}")
            } else {
                format!("{conversion}\n{print}")
            };

            // The runtime branch keeps both formatting helpers present in release.
            let source = format!(
                "char mark = if @args().len == 2 {{ true -> {{ '{mark}' }} _ -> {{ 'x' }} }}\n{declaration}\n{order}\n@println(converted, \":\", converted.len)"
            );
            let expected = format!("{rendered}\n{rendered}:{}\n", rendered.chars().count());
            check(&source, &expected, Tables::Present, "é");
        }
    }
}

#[test]
fn character_observing_string_operations_keep_unicode_support() {
    check(
        r#"
mut str text = @args()[1]
@println(text.len, ":", text[0], ":", text[$])
text[0] = 'X'
for index in text { @print(index) }
@println()
@println("b" in text, ":", text == "Xb")
str joined = text <> "!"
@println(joined, ":", joined.len)
"#,
        "2:a\u{301}:b\n01\ntrue:true\nXb!:3\n",
        Tables::Present,
        "a\u{301}b",
    );
}

#[test]
fn character_array_casts_still_segment_runtime_strings() {
    check(
        "str text = @args()[1];@println(@as(char[], text))",
        "[a\u{301}, b]\n",
        Tables::Present,
        "a\u{301}b",
    );
}

#[test]
fn reduced_strings_example_needs_no_unicode_tables() {
    let source = r#"
mut str[] buf = []
mut int i = 0
while i < 1024 { buf = buf <> [""];i = i + 1 }
fn from_int(int n) {
    mut int i = 0
    while i < n { buf[i & 1023] = "{i}";i = i + 1 }
}
from_int(1024)
@println(buf)
"#;
    let expected = format!(
        "[{}]\n",
        (0..1024)
            .map(|index| format!("\"{index}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    check(source, &expected, Tables::Absent, "é");
}
