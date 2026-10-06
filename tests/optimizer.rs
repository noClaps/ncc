use std::fmt::Write as _;
use std::{fs, path::Path, process::Command};

fn fixture_has_tests(source: &str) -> bool {
    ncc::lexer::lex(source)
        .and_then(ncc::parser::parse)
        .is_ok_and(|module| {
            module
                .items
                .iter()
                .any(|item| matches!(item, ncc::ast::Item::Test { .. }))
        })
}

fn compile_fixture(
    source: &str,
    path: &Path,
    release: bool,
) -> Result<String, ncc::diagnostic::Diagnostics> {
    if fixture_has_tests(source) {
        ncc::compile_test_source_with_options(source, path, release)
    } else {
        ncc::compile_source_with_options(source, path, release)
    }
}

fn executable_fixture(source: &str) -> String {
    let module = ncc::parser::parse(ncc::lexer::lex(source).unwrap()).unwrap();
    let mut result = source.to_owned();
    if fixture_has_tests(source) {
        // Root intended fixture execution without moving global declaration scopes.
        for (index, item) in module.items.iter().enumerate().rev() {
            if let ncc::ast::Item::Statement(statement) = item {
                let (_, span) = statement.source().unwrap();
                result.insert_str(span.end, "\n}\n");
                result.insert_str(
                    span.start,
                    &format!("test \"fixture statement {index}\" {{\n"),
                );
            }
        }
    }
    result
}

fn folded(source: &str, names: &[&str], expected: &str) {
    let source = executable_fixture(source);
    let command = if fixture_has_tests(&source) {
        "test"
    } else {
        "run"
    };
    let unoptimised = compile_fixture(&source, Path::new("constant.nc"), false)
        .expect("unoptimised source is valid");
    let c = compile_fixture(&source, Path::new("constant.nc"), true).unwrap();
    for name in names {
        assert!(
            unoptimised.contains(&format!("nc_fn_{name}(")),
            "{name} was not present before optimisation"
        );
        assert!(
            !c.contains(&format!("nc_fn_{name}(")),
            "{name} was not evaluated"
        );
    }
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("constant.nc");
    fs::write(&input, &source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args([command, "--release"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    if !names.contains(&"signed") {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg(command)
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
}

// Decomposed literals must keep their original bytes during folding.
#[test]
fn string_char_array_boundaries_survive_mutation_and_concatenation() {
    folded(
        include_str!(
            "fixtures/unicode/string_char_array_boundaries_survive_mutation_and_concatenation.nc"
        ),
        &[
            "mutated",
            "joined",
            "inspect",
            "comparisons",
            "last",
            "unicode",
        ],
        "2|2|2|true\n2|2|2|true\nfalse|false|true|true|true|false\ntrue\nfalse\n\r\n\na\u{301}\ntrue\n",
    );
}

#[test]
fn string_boundaries_survive_interpolation_nested_formatting_and_errors() {
    folded(
        r#"
fn split() str { return "a" <> "\u{301}" }
fn message() bool! { throw split() }
fn caught() bool {
    return message() catch err {
            str text = @as(str, err)
            text == split() and text.len == 2
        }
}
fn formatting() str {
    str text = split()
    str converted = @as(str, text)
    str interpolated = "{text}"
    str nested = @as(str, (text, true))
    str error = @as(str, message())
    return "{converted.len}|{interpolated.len}|{nested.len}|{error.len}|{interpolated == text}"
}
@println(caught())
@println(formatting())
@println(message())
@println(@as(str, ('a', '\u{301}')).len)
"#,
        &["split", "message", "caught", "formatting"],
        "true\n2|2|10|9|true\nerror: a\u{301}\n6\n",
    );
}

#[test]
fn error_union_formatting_folds_only_the_active_payload() {
    folded(
        r#"
type Result = int!
struct Record { int! value }
enum Choice { Value(int!) }
fn number(bool fail) int! {
    if fail { true -> { throw "bad\u{0}🍪" } false -> { return 42 } }
}
fn render() str {
    int![] values = [number(false), number(true)]
    return "{values}"
}
fn nominal() str { return @as(str, @as(int!, @as(Result, number(false)))) }
fn nested() str {
    int!? absent = none
    int!? present = number(true)
    Record record = Record{.value = number(true)}
    [str]int! table = ["key": number(true)]
    return "{record}|{Choice.Value(number(false))}|{table}|{(number(false), number(true))}|{absent}|{present}"
}
fn blank() int! { throw "" }
@println(render())
@println(nominal())
@println(@as(str, number(true)))
@println(nested())
@println(blank())
"#,
        &["number", "render", "nominal", "nested", "blank"],
        "[42, error: bad\0🍪]\n42\nerror: bad\0🍪\nRecord{.value = error: bad\0🍪}|Choice.Value(42)|[key: error: bad\0🍪]|(42, error: bad\0🍪)|none|error: bad\0🍪\nerror: \n",
    );
}

#[test]
fn certified_evaluation_exceeds_former_limits() {
    let source = r#"
fn depth(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return 1 + depth(n - 1) } }
}
fn fuel(uint n) uint {
    mut uint i = 0
    while i < n { i = i + 1 }
    return i
}
@println(depth(600))
@println(fuel(100001))
test "unreachable failures" {
    assert true or (1 / 0 == 0)
    assert not (false and (1 / 0 == 0))
    int value = if true { true -> { 42 } false -> { 1 / 0 } }
    assert value == 42
}
"#;
    let executable = executable_fixture(source);
    let c = compile_fixture(&executable, Path::new("limits.nc"), true).unwrap();
    assert!(!c.contains("nc_fn_depth("), "certified recursion must fold");
    assert!(!c.contains("nc_fn_fuel("), "certified loop must fold");
    folded(source, &[], "600\n100001\n");
}

#[test]
fn pure_subexpressions_fold_in_tests_top_level_blocks_and_closures() {
    folded(
        r#"
fn total(uint n) uint {
    mut uint sum = 0
    for i in [1, 2, 3] { sum = sum + n + i }
    return sum
}
test "constant expressions" {
    assert total(2) == 9
    fn answer = fn() uint { return total(3) }
    assert answer() == 12
    mut uint[] values = [total(1)]
    values[0] = total(4)
    assert values[0] == 15
}
{
    uint n = total(5)
    @println(n)
}
"#,
        &["total"],
        "18\n",
    );
}

#[test]
fn top_level_pure_prefix_precomputes_array_and_numeric_loops() {
    let strings = "mut str[] buf=[];mut int i=0;while i<1024 {buf=buf<>[\"\"];i=i+1};@println(buf)";
    let mut expected = String::from("[");
    expected.push_str(&vec!["\"\""; 1024].join(", "));
    expected.push_str("]\n");
    for (source, expected) in [
        (strings, expected.as_str()),
        (
            "mut int sum=0;mut int i=0;while i<100 {sum=sum+i;i=i+1};@println(sum,\":\",i)",
            "4950:100\n",
        ),
        (
            "mut int sum=0;for i in [2,4,6] {sum=sum+@as(int,i)};@println(sum)",
            "3\n",
        ),
        (
            "mut float sum=0.0;mut uint i=0;while i<4 {sum=sum+0.5;i=i+1};@println(sum)",
            "2.0\n",
        ),
    ] {
        let debug = compile_fixture(source, Path::new("prefix.nc"), false).unwrap();
        let release = compile_fixture(source, Path::new("prefix.nc"), true).unwrap();
        let runtime_work = if source.contains("for i") {
            "__builtin_add_overflow"
        } else {
            "} goto "
        };
        assert!(debug.contains(runtime_work), "missing original loop");
        assert!(!release.contains(runtime_work), "loop was not precomputed");
        folded(source, &[], expected);
    }
}

#[test]
fn top_level_precomputation_preserves_copies_and_later_shared_storage() {
    let source = r#"
mut int[][] values = [[1]]
int[][] original = values
mut int i = 0
while i < 3 { values[0][0] = values[0][0] + 1;i = i + 1 }
fn read() int { return values[0][0] }
fn update = fn() int { values[0][0] = values[0][0] + 1;return read() }
@println(original, ":", read(), ":", update(), ":", values)
values[0][0] = 9
@println(update(), ":", values)
"#;
    let release = compile_fixture(source, Path::new("shared-prefix.nc"), true).unwrap();
    assert!(!release.contains("} goto "));
    folded(source, &[], "[[1]]:4:5:[[5]]\n10:[[10]]\n");
}

#[test]
fn top_level_precomputation_crosses_known_output_calls_and_closures_but_not_unknown_state() {
    for (source, expected) in [
        (
            "mut int i=0;@print(\"before:\");while i<3 {i=i+1};@println(i)",
            "before:3\n",
        ),
        (
            "mut int i=0;fn read=fn() int {return i};while i<3 {i=i+1};@println(read())",
            "3\n",
        ),
        (
            "mut int i=0;fn update() {i=1};update();while i<3 {i=i+1};@println(i)",
            "3\n",
        ),
        (
            "mut int i=@as(int,@args().len);while i<3 {i=i+1};@println(i)",
            "3\n",
        ),
        (
            "mut int i=0;while i<3 {@print(i);i=i+1};@println(i)",
            "0123\n",
        ),
    ] {
        let c = compile_fixture(source, Path::new("barrier.nc"), true).unwrap();
        assert_eq!(
            c.contains("} goto "),
            source.contains("@args()"),
            "{source}"
        );
        folded(source, &[], expected);
    }
}

#[test]
fn top_level_precomputation_keeps_runtime_suffix_and_escaped_captures() {
    let source = r"
mut int i=0
while i<3 {i=i+1}
fn read=fn() int {return i}
@println(read())
while i<5 {i=i+1}
@println(read())
";
    let c = compile_fixture(source, Path::new("escaped.nc"), true).unwrap();
    assert!(!c.contains("} goto "), "fully known captured storage folds");
    folded(source, &[], "3\n5\n");
    let source = r"
mut int i=0
while i<3 {i=i+1}
@println(i)
str[] args=@args()
while i<5 {i=i+1}
@println(i)
";
    // Do not resume after unknown input, even for independent computations.
    let c = compile_fixture(source, Path::new("unknown-suffix.nc"), true).unwrap();
    assert_eq!(c.matches("} goto ").count(), 1);
    folded(source, &[], "3\n5\n");
}

#[test]
fn top_level_precomputation_exceeds_former_budget_and_rolls_back_failures() {
    let source = "mut int i=0;while i<20000 {i=i+1};@println(i)";
    let c = compile_fixture(source, Path::new("budget.nc"), true).unwrap();
    assert!(!c.contains("} goto "), "certified region must precompute");
    folded(source, &[], "20000\n");
    for source in [
        "mut int[] values=[1];mut int i=0;while i<3 {i=i+1};values[2]=i;@println(\"unreached\")",
        "mut int i=0;@println(\"before\");mut int[] values=[1];values[2]=i;@println(\"unreached\")",
    ] {
        let directory = ncc::temp::Directory::new().unwrap();
        let input = directory.path().join("failure.nc");
        fs::write(&input, source).unwrap();
        for mode in ["--debug", "--release"] {
            let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
                .args(["run", mode])
                .arg(&input)
                .output()
                .unwrap();
            assert!(!output.status.success());
            let expected = if source.contains("before") {
                "before\n"
            } else {
                ""
            };
            assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
            assert!(String::from_utf8_lossy(&output.stderr).contains("index out of bounds"));
        }
    }
}

#[test]
fn top_level_assignments_and_control_flow_evaluate_reached_failures() {
    for (source, failing_expression) in [
        ("mut int[] values = [1]\nvalues[0] = 1 / 0", "1 / 0"),
        ("mut int[] values = [1]\nvalues[2] = 1 / 0", "1 / 0"),
        ("mut int[][] values = [[1]]\nvalues[2][0] = 1 / 0", "1 / 0"),
        ("mut [str]int values = []\nvalues[\"new\"] = 1 / 0", "1 / 0"),
        ("mut int value = 0\nvalue = 1 / 0", "1 / 0"),
        ("mut int[][] values = [[1]]\nvalues[0][0] = 1 / 0", "1 / 0"),
        (
            "mut int[] values = [1]\nvalues = [1, 2]\nvalues[1] = 1 / 0",
            "1 / 0",
        ),
        ("mut int[] values = [1]\n{ values[0] = 1 / 0 }", "1 / 0"),
        (
            "mut int value = 0\nif true { true -> { value = 1 / 0 } false -> { value = 2 } }",
            "1 / 0",
        ),
        (
            "mut int counter = 1\nwhile counter < 3 { counter = counter + 1 }\ncounter = counter / (counter - 3)",
            "counter / (counter - 3)",
        ),
    ] {
        let path = Path::new("top-level.nc");
        for release in [false, true] {
            if !release && failing_expression != "1 / 0" {
                compile_fixture(source, path, release).unwrap();
                continue;
            }
            let error = compile_fixture(source, path, release).unwrap_err();
            assert!(
                error.to_string().contains("constant evaluation failed"),
                "{source}: {error}"
            );
            let diagnostic = &error.0[0];
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{source}: {error}");
            assert_eq!(
                &source[diagnostic.span.clone()],
                failing_expression,
                "{source}: {error}"
            );
        }
    }
}

#[test]
fn top_level_evaluation_preserves_scopes_copies_and_unreachable_branches() {
    folded(
        r#"
(int, int) seed = (1, 2)
mut int[] values = [seed[0]]
int[] original = values
if false {
    true -> { values[0] = 1 / 0 }
    false -> { values[0] = seed[1] }
}
mut int sum = 0
for i in [1, 2] { sum = sum + values[0] }
{
    mut int[] values = [3]
    values[0] = 4
}
values[0] = sum
mut int count = 1
count = 2
mut int count = count + 1
count = count + 1
@println(values, ":", original, ":", count)
"#,
        &[],
        "[4]:[1]:4\n",
    );
}

#[test]
fn top_level_evaluation_tracks_immutable_closure_and_tuple_bindings() {
    let source = r"
(int, int) captured = (2, 3)
fn operation = fn() int { return captured[0] + captured[1] }
mut int value = 0
value = operation()
value = value / (value - 5)
";
    let path = Path::new("captured-top-level.nc");
    compile_fixture(source, path, false).unwrap();
    let error = compile_fixture(source, path, true).unwrap_err();
    assert!(
        error.to_string().contains("constant evaluation failed"),
        "{error}"
    );
    assert_eq!(error.0[0].path.as_deref(), Some(path));
    assert_eq!(&source[error.0[0].span.clone()], "value / (value - 5)");
}

#[test]
fn top_level_evaluation_does_not_execute_effects_or_assume_runtime_state() {
    folded(
        r#"
mut int value = 1
fn change() int { @print("effect:");value = 4;return 3 }
value = change()
@println(value)
fn runtime() int[] { @print("array:");return [7] }
mut int[] values = runtime()
values[0] = value
@println(values)
"#,
        &[],
        "effect:3\narray:[3]\n",
    );
}

#[test]
fn top_level_output_analysis_preserves_known_state_and_argument_effects() {
    for (source, failing_expression) in [
        (
            "mut int value = 1;@println(\"hello\");value = 1 / 0",
            "1 / 0",
        ),
        (
            "fn output() { @print(\"hello\") };mut int value = 1;output();value = 1 / 0",
            "1 / 0",
        ),
        (
            "mut int value = 0;fn update = fn() int { @print(\"effect\");value = value + 1;return value };@println(update());@println(update());value = value / (value - 2)",
            "value / (value - 2)",
        ),
        (
            "mut int value = 0;fn update = fn() int { value = 2;return value };@println(update());value = 1 / (value - 2)",
            "1 / (value - 2)",
        ),
        (
            "mut int value = 0;while value < 2 { @println(value);value = value + 1 };value = 1 / (value - 2)",
            "1 / (value - 2)",
        ),
        ("mut int value = 0;@println(1 / 0);value = 2", "1 / 0"),
    ] {
        let path = Path::new("after-output.nc");
        for release in [false, true] {
            if !release && failing_expression != "1 / 0" {
                ncc::compile_source_with_options(source, path, release).unwrap();
                continue;
            }
            let error = ncc::compile_source_with_options(source, path, release).unwrap_err();
            assert!(
                error.to_string().contains("constant evaluation failed"),
                "{source}: {error}"
            );
            assert_eq!(error.0[0].path.as_deref(), Some(path));
            assert_eq!(&source[error.0[0].span.clone()], failing_expression);
        }
    }
    folded(
        r#"
mut int count = 0
fn increment = fn() int { @print("effect:");count = count + 1;return count }
@println(increment(), ":", increment())
@println(count)
"#,
        &[],
        "effect:effect:1:2\n2\n",
    );
}

#[test]
fn output_with_unknown_arguments_does_not_assume_later_execution() {
    for source in [
        "mut int value = 0;@println(@args());value = 1 / value",
        "mut int value = 0;@println(@env());value = 1 / value",
        "fn unknown() { @println(@args()) };mut int value = 0;unknown();value = 1 / value",
    ] {
        ncc::compile_source_with_options(source, Path::new("unknown-output.nc"), true).unwrap();
    }
}

fn test_analysis_reached_failure(source: &str, failing_expression: &str) {
    let path = Path::new("test-analysis.nc");
    ncc::compile_test_source_with_options(source, path, false).unwrap();
    let Err(error) = ncc::compile_test_source_with_options(source, path, true) else {
        panic!("release test analysis did not reject {failing_expression}");
    };
    assert!(
        error.to_string().contains("constant evaluation failed"),
        "{source}: {error}"
    );
    let diagnostic = &error.0[0];
    assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
    assert_eq!(
        &source[diagnostic.span.clone()],
        failing_expression,
        "{error}"
    );
}

#[test]
fn test_analysis_tracks_known_true_assertion_side_effects() {
    let source = r#"
mut int value = 0
fn advance() bool { value = value + 1;return value == 1 }
test "assertion effects" {
    assert advance()
    value = 1 / (value - 1)
}
"#;
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("ignored-test.nc"), release).unwrap();
    }
    test_analysis_reached_failure(source, "1 / (value - 1)");
}

#[test]
fn test_analysis_repeats_named_calls_without_inheriting_caller_shadows() {
    test_analysis_reached_failure(
        r#"
mut int value = 0
fn next() int { value = value + 1;return value }
test "lexical global scope" {
    {
        mut int value = 99
        assert next() == 1
        assert value == 99
    }
    assert next() == 2
    value = 1 / (value - 2)
}
"#,
        "1 / (value - 2)",
    );
}

#[test]
fn test_analysis_flows_across_tests_and_intervening_global_mutations() {
    test_analysis_reached_failure(
        r#"
mut int value = 0
test "first" {
    value = value + 1
    assert value == 1
}
value = value + 2
test "second" {
    assert value == 3
    value = 1 / (value - 3)
}
"#,
        "1 / (value - 3)",
    );
}

#[test]
fn test_analysis_cleans_up_shadowed_test_locals() {
    test_analysis_reached_failure(
        r#"
mut int value = 2
test "shadow" {
    mut int value = 9
    value = value + 1
    int local = value
    assert local == 10
}
test "outer binding" {
    int local = value
    assert local == 2
    value = 1 / (local - 2)
}
"#,
        "1 / (local - 2)",
    );
}

#[test]
fn test_analysis_preserves_assertions_output_and_mutations_at_runtime() {
    let source = r#"
mut int value = 0
fn advance() bool { @print("effect:");value = value + 1;return value == 1 }
test "first" {
    @println("before:", value)
    assert advance()
    mut int value = 9
    value = value + 1
    assert value == 10
    @println("local:", value)
}
value = value + 2
test "second" {
    assert value == 3
    @println("global:", value)
}
"#;
    let c = ncc::compile_test_source_with_options(source, Path::new("retained-tests.nc"), true)
        .unwrap();
    assert_eq!(
        c.matches("nc_panic(\"assertion failed\")").count(),
        3,
        "{c}"
    );
    folded(source, &[], "before:0\neffect:local:10\nglobal:3\n");
}

#[test]
fn test_analysis_false_assertion_stops_before_variable_dependent_arithmetic() {
    let source = r#"
mut int value = 0
fn reject() bool { value = value + 1;return false }
test "fails at runtime" {
    @println("before assertion")
    assert reject()
    value = 1 / (value - 1)
}
value = 1 / (value - 1)
test "not reached" { assert value == 0 }
"#;
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("false-assertion.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let c = ncc::compile_test_source_with_options(source, &input, release).unwrap();
        assert!(c.contains("assertion failed"), "{c}");
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "before assertion\n"
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("assertion failed"), "{stderr}");
        assert!(!stderr.contains("division by zero"), "{stderr}");
        assert!(!stderr.contains("constant evaluation failed"), "{stderr}");
    }
}

#[test]
fn test_analysis_unknown_assertions_do_not_assume_later_execution() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("unknown-test.nc");
    fs::write(
        directory.path().join("unknown.c"),
        "#include <stdbool.h>\nbool ready(void) { return true; }\n",
    )
    .unwrap();
    for source in [
        r#"mut int value = 1;test "unknown args" { assert @args().len == 0;value = 1 / (value - 1) };test "later" { value = 1 / (value - 1) }"#,
        r#"extern "unknown.c" as native { fn ready() bool = "ready" };mut int value = 1;test "unknown native" { assert native.ready();value = 1 / (value - 1) };test "later" { value = 1 / (value - 1) }"#,
        r#"fn ready() bool { return true };mut int value = 1;test "unknown future" { fut bool work = async ready();assert await work;value = 1 / (value - 1) };test "later" { value = 1 / (value - 1) }"#,
    ] {
        for release in [false, true] {
            ncc::compile_test_source_with_options(source, &input, release).unwrap();
        }
    }
}

#[test]
fn test_analysis_diagnoses_reached_output_arguments_without_executing_output() {
    let source = r#"
mut int value = 0
fn advance() bool { @println("assertion effect");value = 2;return true }
test "reached output" {
    @println("before assertion")
    assert advance()
    @println(1 / (value - 2))
}
"#;
    test_analysis_reached_failure(source, "1 / (value - 2)");
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("silent-analysis.nc");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args(["test", "--release"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("constant evaluation failed"), "{stderr}");
    assert!(!stderr.contains("assertion effect"), "{stderr}");
}

#[test]
fn infinite_loops_remain_runtime_but_their_contents_can_fold() {
    let source = r"
fn sum(int n) int { return n + 1 }
while true { @println(sum(2)) }
@println(1 / 0)
int unreachable = 1 / 0
";
    for release in [false, true] {
        let output =
            ncc::compile_source_with_diagnostics(source, Path::new("forever.nc"), release).unwrap();
        assert!(output.c.matches("goto ").count() >= 2, "{}", output.c);
        assert!(
            output
                .warnings
                .0
                .iter()
                .any(|d| d.message.contains("infinite loop"))
        );
        assert_eq!(
            output
                .warnings
                .0
                .iter()
                .filter(|d| d.message == "unreachable code")
                .count(),
            2
        );
        if release {
            assert!(!output.c.contains("nc_fn_sum("));
            assert!(output.c.contains("3LL"));
        }
    }
}

#[test]
fn infinite_loop_proofs_use_value_block_types_and_builtin_argument_order() {
    for source in [
        "while true { int value = if true { true -> { break 1 } false -> { break 2 } } };_ = 1 / 0",
        "mut int divisor = 0;fn maybe_spin(bool spin) int { if spin { true -> { while true {} } false -> { return 1 } } };@println(maybe_spin(true), 1 / divisor)",
        "mut int divisor = 0;@println(@args(), 1 / divisor)",
    ] {
        for release in [false, true] {
            ncc::compile_source_with_options(source, Path::new("unreached-arithmetic.nc"), release)
                .unwrap();
        }
    }
    let source = "test \"spin\" { while true {} };test \"unreachable\" { @println(1 / 0) }";
    for release in [false, true] {
        let output = ncc::compile_test_source_with_diagnostics(
            source,
            Path::new("infinite-test.nc"),
            release,
        )
        .unwrap();
        assert!(
            output
                .warnings
                .0
                .iter()
                .any(|d| d.message == "unreachable code")
        );
    }
}

#[test]
fn normal_compilation_ignores_tests_during_top_level_analysis() {
    let source = "mut int value = 1;test \"ignored\" { value = 2;assert false };@println(\"hello\");value = 1 / (value - 1)";
    let error =
        ncc::compile_source_with_options(source, Path::new("ignored-test.nc"), true).unwrap_err();
    assert_eq!(&source[error.0[0].span.clone()], "1 / (value - 1)");
}

#[test]
fn float_string_folding_matches_c_for_boundaries_and_sampled_bit_patterns() {
    let mut values = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        0.0001,
        0.00001,
        1e16,
        1e17,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::from_bits(0x000f_ffff_ffff_ffff),
    ];
    let mut bits = 0x5eed_u64;
    for _ in 0..256 {
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        let value = f64::from_bits(bits);
        if value.is_finite() {
            values.push(value);
        }
    }
    let mut source = String::from("fn render(float n) str { return @as(str, n) }\n");
    for value in values {
        writeln!(source, "@println(render({value:.340}))").unwrap();
    }
    let c = compile_fixture(&source, Path::new("floats.nc"), true).unwrap();
    assert!(
        !c.contains("nc_fn_render("),
        "float conversion was not folded"
    );
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("floats.nc");
    fs::write(&input, source).unwrap();
    let outputs = ["--debug", "--release"].map(|mode| {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("test")
            .arg(mode)
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    });
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn fixed_and_dynamic_array_concatenations_fold_without_aliasing_values() {
    folded(
        r"
fn fixed() int[3] {
    int[0] empty = []
    int[1] first = [1]
    int[2] second = [2, 3]
    return empty <> first <> second <> empty
}
fn dynamic() int[] {
    int[2] first = [1, 2]
    int[] second = [3, 4]
    int[] empty = []
    return empty <> first <> second <> empty
}
fn converted() int[][] {
    mut int[][1] left = [[1, 2]]
    mut int[][1] right = [[3, 4]]
    mut int[][2] joined = left <> right
    mut int[][] copy = @as(int[][], joined)
    left[0][0] = 8
    right[0][0] = 9
    joined[0][0] = 6
    copy[1][0] = 7
    return joined <> copy <> left <> right
}
fn empty() int[0] {
    int[0] first = []
    return first <> first
}
@println(fixed())
@println(dynamic())
@println(converted())
@println(empty())
",
        &["fixed", "dynamic", "converted", "empty"],
        "[1, 2, 3]\n[1, 2, 3, 4]\n[[6, 2], [3, 4], [1, 2], [7, 4], [8, 2], [9, 4]]\n[]\n",
    );
}

#[test]
fn nonfinite_float_arithmetic_folds_and_materializes() {
    folded(
        r"
fn values() float[] {
    float positive = 1.0 / 0.0
    float negative = -1.0 / 0.0
    float nan = 0.0 / 0.0
    float large = 2.0 ** 1023.0
    return [positive, negative, nan, large * 2.0,
        -large * 2.0, positive + 1.0, positive + negative,
        positive - positive, positive * 0.0, positive / positive,
        2.0 ** 1024.0, (-1.0) ** 0.5,
        0.0 ** -1.0, nan + 1.0, -nan]
}
@println(values())
",
        &["values"],
        "[inf, -inf, NaN, inf, -inf, inf, NaN, NaN, NaN, NaN, inf, NaN, inf, NaN, NaN]\n",
    );
}

#[test]
fn nonfinite_float_container_formatting_folds() {
    folded(
        r#"
struct Record { float value }
enum Choice { Value(float) }
fn convert(float value) str { return @as(str, value) }
fn render() str {
    float nan = 0.0 / 0.0
    float positive = 1.0 / 0.0
    float negative = -1.0 / 0.0
    float? optional = nan
    float! success = positive
    float?[3] values = [nan, none, negative]
    return "{nan}|{positive}|{negative}|{[nan, positive, negative]}|{(nan, positive, negative)}|{[positive: nan]}|{Record{.value = negative}}|{Choice.Value(nan)}|{optional}|{success}|{values}"
}
@println(render())
@println(convert(0.0 / 0.0))
@println(convert(1.0 / 0.0))
@println(convert(-1.0 / 0.0))
"#,
        &["render", "convert"],
        "NaN|inf|-inf|[NaN, inf, -inf]|(NaN, inf, -inf)|[inf: NaN]|Record{.value = -inf}|Choice.Value(NaN)|NaN|inf|[NaN, none, -inf]\nNaN\ninf\n-inf\n",
    );
}

#[test]
fn nonfinite_float_comparisons_and_membership_fold_with_ieee_equality() {
    folded(
        r"
struct Record { float value }
enum Choice { Value(float) }
fn scalar(float nan, float positive, float negative) bool[] {
    return [nan == nan, nan != nan, nan < 0.0, nan <= nan, nan > 0.0,
        nan >= nan, positive == positive, negative == negative,
        negative < positive, positive > 1.0, positive == negative, 0.0 == -0.0]
}
fn containers(float nan, float positive) bool[] {
    float? optional = nan
    float! success = nan
    float? absent = none
    return [[nan] == [nan], (nan, 1) == (nan, 1),
        [nan: 1] == [nan: 1], [1: nan] == [1: nan],
        Record{.value = nan} == Record{.value = nan},
        Choice.Value(nan) == Choice.Value(nan), optional == optional,
        success == success, [optional] == [optional], [success] == [success],
        absent == absent, [positive] == [positive], [0.0] == [-0.0],
        [nan] != [nan]]
}
fn membership(float nan, float positive) bool[] {
    return [nan in [nan], nan in [nan: 1], [nan] in [[nan]],
        (nan, 1) in [(nan, 1)], positive in [nan, positive],
        positive in [positive: 1], -0.0 in [0.0], -0.0 in [0.0: 1]]
}
@println(scalar(0.0 / 0.0, 1.0 / 0.0, -1.0 / 0.0))
@println(containers(0.0 / 0.0, 1.0 / 0.0))
@println(membership(0.0 / 0.0, 1.0 / 0.0))
",
        &["scalar", "containers", "membership"],
        "[false, true, false, false, false, false, true, true, true, true, false, true]\n[false, false, false, false, false, false, false, false, false, false, true, true, true, true]\n[false, false, false, false, true, true, true, true]\n",
    );
}

#[test]
fn numeric_byte_encodings_fold_without_losing_ieee_bits_or_capture_values() {
    let mut source = String::from(
        r"
fn encode_signed(int value) byte[] { return @as(byte[], value) }
fn encode_unsigned(uint value) byte[] { return @as(byte[], value) }
fn captured_bytes(float value) byte[] {
    (float, uint) pair = (value, 258u)
    fn callback = fn() byte[] { return @as(byte[], pair[0]) }
    return callback()
}
",
    );
    let mut expected = String::new();
    let mut cases = Vec::new();
    for value in [i64::MIN, -258, -1, 0, 1, 258, i64::MAX] {
        cases.push((format!("encode_signed({value})"), value.to_le_bytes()));
    }
    for value in [0u64, 1, 258, 1 << 63, u64::MAX] {
        cases.push((format!("encode_unsigned({value}u)"), value.to_le_bytes()));
    }
    for value in [
        0.0f64,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -2.5,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
    ] {
        // NC source literals have no exponent notation. Enough decimal places
        // retain even the smallest subnormal exactly when both parsers round it.
        let literal = format!("{value:.1074}");
        cases.push((format!("captured_bytes({literal})"), value.to_le_bytes()));
    }
    for literal in ["inf", "-inf"] {
        let value = if literal == "inf" {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
        cases.push((format!("captured_bytes({literal})"), value.to_le_bytes()));
    }
    for (call, bytes) in cases {
        writeln!(source, "@println({call})").unwrap();
        writeln!(
            expected,
            "[{}]",
            bytes.map(|byte| byte.to_string()).join(", ")
        )
        .unwrap();
    }
    folded(
        &source,
        &["encode_signed", "encode_unsigned", "captured_bytes"],
        &expected,
    );
}

#[test]
fn every_byte_char_conversion_folds_to_matching_utf8_bytes() {
    let mut source = String::from(
        r"
fn utf8_bytes(byte value) byte[] {
    (byte, bool) captured = (value, true)
    fn convert = fn() byte[] { return @as(byte[], @as(char, captured[0])) }
    return convert()
}
",
    );
    let mut expected = String::new();
    for value in 0u8..=255 {
        writeln!(source, "@println(utf8_bytes({value}))").unwrap();
        let mut buffer = [0; 4];
        let encoded = char::from(value).encode_utf8(&mut buffer);
        let bytes = encoded
            .as_bytes()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(expected, "[{bytes}]").unwrap();
    }
    folded(&source, &["utf8_bytes"], &expected);
}

#[test]
fn finite_float_integer_casts_fold_at_exact_representable_boundaries() {
    folded(
        r#"
fn truncate_signed(float value) int { return @as(int, value) }
fn truncate_unsigned(float value) uint { return @as(uint, value) }
@println(truncate_signed(-9223372036854775808.0))
@println(truncate_signed(-9223372036854774784.0))
@println(truncate_signed(9223372036854774784.0))
@println(truncate_unsigned(18446744073709549568.0))
@println(truncate_unsigned(9223372036854775808.0))
@println(truncate_signed(-0.0), ":", truncate_unsigned(-0.0))
@println(truncate_signed(-1.75), ":", truncate_signed(1.75))
@println(truncate_unsigned(1.75), ":", truncate_signed(-0.75))
"#,
        &["truncate_signed", "truncate_unsigned"],
        "-9223372036854775808\n-9223372036854774784\n9223372036854774784\n\
         18446744073709549568\n9223372036854775808\n0:0\n-1:1\n1:0\n",
    );
}

#[test]
fn negative_fractional_float_to_uint_is_rejected_before_constant_truncation() {
    let tiny = format!("-0.{}5", "0".repeat(323));
    let path = Path::new("negative-cast.nc");
    for value in ["-0.75", "-0.5", "-0.0001", tiny.as_str()] {
        for (body, failing_expression) in [
            ("return @as(uint, value)", "@as(uint, value)"),
            (
                r"
(float, bool) captured = (value, true)
fn convert = fn() uint { return @as(uint, captured[0]) }
return convert()
",
                "@as(uint, captured[0])",
            ),
        ] {
            let source = format!(
                "fn cast_negative(float value) uint {{ {body} }};@println(cast_negative({value}))",
            );
            compile_fixture(&source, path, false).unwrap();
            let error = compile_fixture(&source, path, true).unwrap_err();
            assert!(
                error.to_string().contains("constant evaluation failed"),
                "{value}: {error}",
            );
            let diagnostic = &error.0[0];
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{value}: {error}");
            assert_eq!(
                &source[diagnostic.span.clone()],
                failing_expression,
                "{value}: {error}",
            );
        }
    }
}

#[test]
fn finite_out_of_range_numeric_casts_are_rejected_during_folding() {
    for (from, to, value) in [
        ("float", "int", "9223372036854775808.0"),
        ("float", "int", "-9223372036854777856.0"),
        ("float", "uint", "18446744073709551616.0"),
        ("float", "uint", "-1.0"),
        ("int", "uint", "-1"),
        ("uint", "int", "9223372036854775808u"),
        ("uint", "int", "18446744073709551615u"),
    ] {
        let source = format!(
            "fn cast({from} value) {to} {{ return @as({to}, value) }};@println(cast({value}))",
        );
        compile_fixture(&source, Path::new("cast.nc"), false).unwrap();
        let error = compile_fixture(&source, Path::new("cast.nc"), true).unwrap_err();
        assert!(
            error.to_string().contains("constant evaluation failed"),
            "{from} -> {to}, {value}: {error}",
        );
    }
}

#[test]
fn nonfinite_float_integer_casts_remain_rejected_during_folding() {
    for value in ["0.0 / 0.0", "1.0 / 0.0", "-1.0 / 0.0"] {
        for ty in ["int", "uint"] {
            let source =
                format!("fn cast(float n) {ty} {{ return @as({ty}, n) }};@println(cast({value}))");
            let error = compile_fixture(&source, Path::new("bad.nc"), true).unwrap_err();
            assert!(
                error.to_string().contains("constant evaluation failed"),
                "{ty} cast of {value}: {error}"
            );
        }
    }
}

#[test]
fn composite_string_conversions_fold_with_runtime_formatting() {
    folded(
        r#"
type Text = str
struct Record { str name int[] values }
enum Choice { Empty Data(str, int?[]) }
fn array() str { return @as(str, ["a", "b\u{0}🍪"]) }
fn alias() str { Text value = "plain";return @as(str, [@as(str, value)]) }
fn optional() str { int?[3] values = [none, 2, none];return @as(str, values) }
fn tuple() str { return @as(str, ("hi", [true, false], 9u)) }
fn map() str { return @as(str, ["key": [1, 2]]) }
fn record() str { return @as(str, Record{.values = [3, 4], .name = "x"}) }
fn variant() str { return @as(str, Choice.Data("quoted", [none, 1])) }
fn empty() str { return @as(str, Choice.Empty) }
@println(array())
@println(alias())
@println(optional())
@println(tuple())
@println(map())
@println(record())
@println(variant())
@println(empty())
"#,
        &[
            "array", "alias", "optional", "tuple", "map", "record", "variant", "empty",
        ],
        "[\"a\", \"b\0🍪\"]\n[\"plain\"]\n[none, 2, none]\n(hi, [true, false], 9)\n[key: [1, 2]]\nRecord{.name = x, .values = [3, 4]}\nChoice.Data(\"quoted\", [none, 1])\nChoice.Empty\n",
    );
}

#[test]
fn nominal_void_constants_keep_alias_chains_in_containers_and_errors() {
    folded(
        r"
type Unit = void
type Second = Unit
fn unit() {}
fn wrapped() Second { return @as(Second, @as(Unit, unit())) }
fn values() Second[] { return [wrapped(), wrapped()] }
fn optional() Second? { return wrapped() }
fn checked() Second! { return wrapped() }
Second value = wrapped()
Second[] array = values()
Second? present = optional()
Second unwrapped = present else { wrapped() }
Second success = try checked()
@println(array.len)
",
        &["unit", "wrapped", "values", "optional", "checked"],
        "2\n",
    );
}

#[test]
fn pure_void_calls_fold_without_hiding_effects() {
    folded(
        r#"
fn noop() {}
fn early(bool stop) { if stop { true -> { return } _ -> {} } }
fn checked(bool fail) void! { if fail { true -> { throw "failed" } _ -> {} };noop() }
fn forwarded() void! { return noop() }
fn invoke((fn() void) callback) { callback() }
fn total() int! {
    noop();early(false);early(true)
    invoke(fn() { return })
    try checked(false);try forwarded()
    return 42
}
noop()
@println(try total())
fn recovered() { checked(true) catch message { @println(message) } }
recovered()
fn effect() { @print("effect:") }
fn effect_forwarded() void! { return effect() }
fn runtime() int! { try effect_forwarded();noop();return 7 }
@println(try runtime())
"#,
        &["noop", "early", "checked", "forwarded", "invoke", "total"],
        "42\nfailed\neffect:7\n",
    );
    let source = "fn loop() { while true {} };loop()";
    let c = compile_fixture(source, Path::new("void.nc"), true).unwrap();
    assert!(c.contains("nc_fn_loop("));
}

#[test]
fn pure_subexpressions_fold_inside_effectful_functions() {
    folded(
        r#"
fn total(uint n) uint {
    mut uint result = 0
    mut uint i = 0
    while i < n { result = result + i;i = i + 1 }
    return result
}
fn runtime(uint input) uint {
    @print("effect:", input, ":")
    uint constant = total(10)
    return constant + input
}
@println(runtime(7))
"#,
        &["total"],
        "effect:7:52\n",
    );
    folded(
        "fn pure() int { return 42 };fn runtime() { fut int work = async pure();@println(await work) };runtime()",
        &[],
        "42\n",
    );
    folded(
        "fn runtime(bool fail) int { @print(\"effect:\");return if fail { true -> { 1 / 0 } false -> { 9 } } };@println(runtime(false))",
        &[],
        "effect:9\n",
    );
    folded(
        "fn runtime() int { @print(\"effect:\");mut int n = 1;int result = if true { true -> { n = 2;3 } false -> { 4 } };return n + result };@println(runtime())",
        &[],
        "effect:5\n",
    );
}

#[test]
fn thrown_errors_catch_try_and_early_returns_fold() {
    folded(
        r#"
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
fn forwarded(int n) int! { return try checked(n) }
fn recovered(int n) int {
    mut int changes = 0
    int value = forwarded(n) catch message { changes = 10;break @as(int, @as(str, message).len) }
    return value + changes
}
fn early() int { int n = checked(0) catch _ { return 42 };return n }
fn optional() int { int? n = none;return n else { return 7 } }
fn conditional() int { int n = if true { true -> { return 8 } false -> { break 0 } };return n }
@println(recovered(0), recovered(3), early(), optional())
@println(conditional())
int! stored = checked(0)
@println(stored catch _ { 9 })
"#,
        &[
            "checked",
            "forwarded",
            "recovered",
            "early",
            "optional",
            "conditional",
        ],
        "143427\n8\n9\n",
    );
    folded(
        "fn effect() int! { @print(\"effect:\");throw \"bad\" };@println(effect() catch _ { 5 })",
        &[],
        "effect:5\n",
    );
}

#[test]
fn optional_else_and_error_catch_fold_lazy_fallbacks_and_local_control_flow() {
    folded(
        r#"
fn checked(bool fail) int! {
    if fail { true -> { throw "bad" } false -> { return 5 } }
}
fn optional_case(int? input, bool early) int {
    int base, int bonus = (7, 2)
    fn fallback = fn() int { return base + bonus }
    mut int changes = 1
    int value = input else {
        changes = changes + 10
        if early { true -> { return 100 + changes + fallback() } false -> {} }
        break fallback()
    }
    return changes + value
}
fn error_case(int! input, bool early) int {
    int base, int bonus = (7, 2)
    fn fallback = fn() int { return base + bonus }
    mut int changes = 1
    int value = input catch message {
        changes = changes + 10
        int recovered = fallback() + @as(int, @as(str, message).len)
        if early { true -> { return 100 + changes + recovered } false -> {} }
        break recovered
    }
    return changes + value
}
fn lazy_optional(int? input) int { return input else (1 / 0) }
fn lazy_error(int! input) int { return input catch _ { break 1 / 0 } }
@println(optional_case(5, false), ":", optional_case(5, true))
@println(optional_case(none, false), ":", optional_case(none, true))
@println(error_case(checked(false), false), ":", error_case(checked(false), true))
@println(error_case(checked(true), false), ":", error_case(checked(true), true))
@println(lazy_optional(5), ":", lazy_error(checked(false)))
"#,
        &[
            "checked",
            "optional_case",
            "error_case",
            "lazy_optional",
            "lazy_error",
        ],
        "6:6\n20:120\n6:6\n23:123\n5:5\n",
    );
}

#[test]
fn nested_fallback_loop_jumps_fold_without_evaluating_abandoned_assignment_targets() {
    folded(
        r#"
fn failed() int! { throw "outer" }
fn assignment_jumps() (int, int, int) {
    mut int[] values = [0]
    mut int changes = 0
    mut int target_calls = 0
    fn target() uint { target_calls = target_calls + 1;return 0 }
    rows: for i in [10, 20, 30] {
        for j in [0] {
            values[target()] = failed() catch _ {
                changes = changes + 10
                int? absent = none
                int recovered = absent else {
                    changes = changes + 1
                    if i {
                        0 -> { continue :rows }
                        2 -> { break :rows }
                        _ -> { break 7 }
                    }
                }
                changes = changes + 100
                break recovered
            }
        }
        changes = changes + 1000
    }
    return values[0], changes, target_calls
}
@println(assignment_jumps())
"#,
        &["assignment_jumps"],
        "(7, 1133, 1)\n",
    );
}

#[test]
fn fallback_jumps_restore_shadowed_bindings_before_loop_continuation() {
    folded(
        r"
fn cleanup() int {
    mut int value = 7
    for i in [1, 2, 3] {
        mut int value = 99
        int? missing = none
        int skipped = missing else { continue }
        value = skipped
    }
    return value
}
@println(cleanup())
",
        &["cleanup"],
        "7\n",
    );
}

#[test]
fn nested_fallback_try_and_return_fold_with_shared_mutations_and_skipped_tails() {
    folded(
        r#"
fn failed() int! { throw "outer" }
fn nested_recovery(bool fail, bool early) (int, int) {
    mut int changes = 0
    fn step() int! {
        changes = changes + 100
        if fail { true -> { throw "inner" } false -> { return 7 } }
    }
    fn recover() int! {
        int value = failed() catch outer {
            changes = changes + 1
            int? absent = none
            int recovered = absent else {
                changes = changes + 10
                int n = try step()
                if early { true -> { return n + changes } false -> {} }
                changes = changes + 1000
                break n
            }
            changes = changes + 10000
            break recovered
        }
        changes = changes + 100000
        return value + changes
    }
    int result = recover() catch message {
        if @as(str, message) { "inner" -> { break -1 } _ -> { break -2 } }
    }
    return result, changes
}
@println(nested_recovery(false, false))
@println(nested_recovery(false, true))
@println(nested_recovery(true, false))
"#,
        &["nested_recovery"],
        "(111118, 111111)\n(118, 111)\n(-1, 111)\n",
    );
}

#[test]
fn optional_else_and_error_catch_fold_independent_extracted_array_copies() {
    folded(
        r#"
fn checked(bool fail) int[]! {
    if fail { true -> { throw "bad" } false -> { return [1, 2] } }
}
fn optional_copies(int[]? input) (int[], int[], int[]) {
    int[] original = input else { break [3, 4] }
    mut int[] copy = original
    copy[0] = 99
    int[] again = input else { break [3, 4] }
    return original, copy, again
}
fn error_copies(int[]! input) (int[], int[], int[]) {
    int[] original = input catch _ { break [3, 4] }
    mut int[] copy = original
    copy[0] = 99
    int[] again = input catch _ { break [3, 4] }
    return original, copy, again
}
@println(optional_copies([1, 2]))
@println(optional_copies(none))
@println(error_copies(checked(false)))
@println(error_copies(checked(true)))
"#,
        &["checked", "optional_copies", "error_copies"],
        "([1, 2], [99, 2], [1, 2])\n([3, 4], [99, 4], [3, 4])\n([1, 2], [99, 2], [1, 2])\n([3, 4], [99, 4], [3, 4])\n",
    );
}

#[test]
fn value_branches_preserve_mutations_of_surrounding_locals() {
    folded(
        r"
fn branch() int {
    mut int n = 0
    int value = if true { true -> { n = 5;break 1 } false -> { break 0 } }
    return n + value
}
fn fallback() int {
    mut int n = 0
    int? absent = none
    int value = absent else { n = 7;break 2 }
    return n + value
}
@println(branch())
@println(fallback())
",
        &["branch", "fallback"],
        "6\n9\n",
    );
}

#[test]
fn pure_by_value_closures_fold_without_conflating_captured_environments() {
    folded(
        r"
fn make(int n) (fn(int) int) { return fn(int x) int { return n + x } }
fn apply((fn(int) int) f, int n) int { return f(n) }
fn compute() int {
    int n = 4
    fn captured(int x) int { return n * x }
    return captured(3) + apply(make(10), 2) + apply(make(20), 2)
}
fn collection() int {
    int[] values = [1, 2, 3]
    fn sum() int { mut int total = 0;for i in values { total = total + values[i] };return total }
    return sum()
}
@println(compute())
@println(collection())
",
        &["compute", "collection", "make", "apply"],
        "46\n6\n",
    );
    folded(
        "fn make(int n) (fn(int) int) { return fn(int x) int { @print(\"effect:\");return n + x } }\n(fn(int) int) closure = make(4)\n@println(closure(2))",
        &[],
        "effect:6\n",
    );
}

#[test]
fn composite_parameter_shadow_copies_fold_without_changing_callers() {
    folded(
        r"
struct Record { int[] values }
fn edit(Record record, (int[], int) pair) int {
    mut Record record = record
    mut (int[], int) pair = pair
    record.values[0] = record.values[0] + 10
    pair[0][0] = pair[0][0] + 20
    pair[1] = pair[1] + 30
    return record.values[0] + pair[0][0] + pair[1]
}
fn copies() (int, int, int[], (int[], int)) {
    Record record = Record{.values = [1, 2]}
    (int[], int) pair = ([3, 4], 5)
    int first = edit(record, pair)
    int second = edit(record, pair)
    return (first, second, record.values, pair)
}
@println(copies())
",
        &["copies", "edit"],
        "(69, 69, [1, 2], ([3, 4], 5))\n",
    );
}

#[test]
fn generic_first_element_length_folds_across_unrelated_types() {
    folded(
        r#"
fn get_len_of_first<T>(T[] arr) uint { return arr[0].len }
struct Rectangle {
    uint len
    uint wid
}
@println(get_len_of_first<str>(["a" <> "\u{301}", ""]))
@println(get_len_of_first<str>(["", "hello"]))
@println(get_len_of_first<int[]>([[1, 2, 3], []]))
@println(get_len_of_first<int[]>([[], [4, 5]]))
@println(get_len_of_first<[char]int>([['a': 1, 'b': 2], []]))
@println(get_len_of_first<[char]int>([[], ['c': 3]]))
@println(get_len_of_first<Rectangle>([Rectangle{.len = 7, .wid = 4}]))
@println(get_len_of_first<Rectangle>([Rectangle{.len = 0, .wid = 9}]))
"#,
        &[
            "specialized_0_get_len_of_first",
            "specialized_1_get_len_of_first",
            "specialized_2_get_len_of_first",
            "specialized_3_get_len_of_first",
        ],
        "2\n0\n3\n0\n2\n0\n7\n0\n",
    );
}

#[test]
fn returned_callbacks_and_specialized_generic_function_types_fold() {
    folded(
        r#"
fn identity<type T>(T value) T { return value }
fn apply<type T, type U>((fn(T) U) callback, T value) U { return callback(value) }
fn make((int, int) pair) (fn(int) int) {
    return fn(int value) int { return pair[0] * value + pair[1] }
}
fn suffix(str ending) (fn(str) str) {
    return fn(str value) str { return value <> ending }
}
struct Callbacks { (fn(int) int)[] operations }
fn callbacks() int {
    (int, int) pair = (2, 3)
    (fn(int) int) first = apply<(int, int), (fn(int) int)>(make, pair)
    Callbacks stored = Callbacks{.operations = [
        identity<(fn(int) int)>(first),
        identity<(fn(int) int)>(make((4, 5)))
    ]}
    int a = apply<int, int>(stored.operations[0], 6)
    int b = apply<int, int>(stored.operations[1], 6)
    int c = stored.operations[0](7)
    return a + b + c
}
fn text_callback() str {
    (fn(str) str) callback = identity<(fn(str) str)>(suffix("!"))
    return apply<str, str>(callback, "done")
}
@println(callbacks())
@println(text_callback())
"#,
        &["callbacks", "text_callback", "make", "suffix"],
        "61\ndone!\n",
    );
}

#[test]
fn optional_and_error_equality_compares_only_active_payloads() {
    folded(
        r#"
fn result(bool fail) int[]! { if fail { true -> { throw "bad\u{0}value" } false -> { return [1, 2] } } }
int[]! success = result(false)
int[]! failure = result(true)
int[]? absent = none
int[]? present = [1, 2]
@println(success == result(false))
@println(failure == result(true))
@println(success != failure)
@println(absent == absent, absent != present, present == present)
"#,
        &[],
        "true\ntrue\ntrue\ntruetruetrue\n",
    );
}

#[test]
fn specified_cast_table_matches_in_debug_and_release() {
    let mut source = String::from("enum E { Value(int) };struct S { int value }\n");
    let mut expected = String::new();
    for (i, (from, to, value, output)) in [
        ("int[2]", "int[]", "[1, 2]", "[1, 2]"),
        ("int[2]", "str", "[1, 2]", "[1, 2]"),
        ("bool", "int", "true", "1"),
        ("bool", "uint", "false", "0"),
        ("bool", "str", "true", "true"),
        ("byte", "char", "233", "é"),
        ("byte", "int", "255", "255"),
        ("byte", "uint", "255", "255"),
        ("byte", "str", "255", "255"),
        ("char", "byte[]", "'é'", "[195, 169]"),
        ("char", "str", "'🍪'", "🍪"),
        ("E", "str", "E.Value(2)", "E.Value(2)"),
        ("[str]int", "str", "[\"x\": 3]", "[x: 3]"),
        ("int", "byte[]", "258", "[2, 1, 0, 0, 0, 0, 0, 0]"),
        ("int", "uint", "7", "7"),
        ("int", "float", "7", "7.0"),
        ("int", "str", "-7", "-7"),
        ("uint", "byte[]", "258", "[2, 1, 0, 0, 0, 0, 0, 0]"),
        ("uint", "int", "7", "7"),
        ("uint", "float", "7", "7.0"),
        (
            "uint",
            "str",
            "18446744073709551615",
            "18446744073709551615",
        ),
        ("float", "byte[]", "1.0", "[0, 0, 0, 0, 0, 0, 240, 63]"),
        ("float", "int", "-7.9", "-7"),
        ("float", "uint", "7.9", "7"),
        ("float", "str", "7.0", "7.0"),
        ("str", "char[]", "\"a🍪\"", "[a, 🍪]"),
        ("str", "byte[]", "\"é\"", "[195, 169]"),
        ("S", "str", "S{.value = 9}", "S{.value = 9}"),
        ("(str, int)", "str", "(\"x\", 4)", "(x, 4)"),
    ]
    .iter()
    .enumerate()
    {
        write!(source, "fn cast_{i}({from} value) {to} {{ return @as({to}, value) }}\n@println(cast_{i}({value}))\n").unwrap();
        expected.push_str(output);
        expected.push('\n');
    }
    folded(&source, &[], &expected);
}

#[test]
fn signed_unsigned_casts_fold_without_losing_integer_precision() {
    let mut source = String::from(
        "fn as_unsigned(int value) uint { return @as(uint, value) }\n\
         fn as_signed(uint value) int { return @as(int, value) }\n",
    );
    let mut expected = String::new();
    for value in [
        0_i64,
        1,
        255,
        9_007_199_254_740_991,
        9_007_199_254_740_992,
        9_007_199_254_740_993,
        i64::MAX - 1,
        i64::MAX,
    ] {
        writeln!(source, "@println(as_unsigned({value}))\n@println(as_signed({value}u))\n@println(as_signed(as_unsigned({value})))").unwrap();
        for _ in 0..3 {
            writeln!(expected, "{value}").unwrap();
        }
    }
    folded(&source, &["as_unsigned", "as_signed"], &expected);
}

#[test]
fn enum_constructors_are_first_class_and_async_callable() {
    folded(
        r"
enum Data { Value(int[]) Empty }
(fn(int[]) Data) construct = Data.Value
fn apply((fn(int[]) Data) f) Data { return f([3]) }
mut int[] values = [1, 2]
Data stored = construct(values)
values[0] = 99
@println(stored, apply(construct))
fut Data work = async construct([4])
@println(await work)
",
        &[],
        "Data.Value([1, 2])Data.Value([3])\nData.Value([4])\n",
    );
    for source in [
        "@println(int)",
        "struct S { int n };@println(S)",
        "enum E { A };E e = E",
    ] {
        assert!(compile_fixture(source, Path::new("types.nc"), false).is_err());
    }
}

#[test]
fn async_builtins_evaluate_arguments_once_and_keep_runtime_process_state() {
    folded(
        r"
mut int calls = 0
fn next() int { calls = calls + 1;return calls }
fut void printed = async @println(next(), next())
await printed
@println(calls)
mut int[] values = [1, 2]
fut void snapshot = async @println(values)
values[0] = 99
await snapshot
fut (str, str) platform = async @target()
@println(await platform)
fut str[] arguments = async @args()
str[] args = await arguments
@println(args.len > 0)
fut [str]str environment = async @env()
[str]str env = await environment
@println(env == @env())
",
        &[],
        "12\n2\n[1, 2]\n(macos, arm64)\ntrue\ntrue\n",
    );
}

#[test]
fn builtin_arguments_follow_function_call_evaluation_order() {
    folded(
        r#"
mut int[] values = [1]
fn update() str { values[0] = 2;@print("effect:");return "done" }
@println(values, update(), values)
"#,
        &[],
        "effect:[1]done[2]\n",
    );
}

#[test]
fn nominal_composite_casts_preserve_representation_and_value_copies() {
    folded(
        r#"
type Text = str
type List = int[]
type Pair = (str, int)
type Table = [str]int
struct Record { int[] values }
type Wrapped = Record
type Maybe = int?
fn text() str { return @as(str, @as(Text, "a\u{0}b")) }
fn list() int[] { return @as(int[], @as(List, [1, 2])) }
fn pair() (str, int) { return @as((str, int), @as(Pair, ("x", 3))) }
fn table() [str]int { return @as([str]int, @as(Table, ["x": 4])) }
fn record() Record { return @as(Record, @as(Wrapped, Record{.values = [5]})) }
@println(text(), list(), pair(), table(), record())
mut int[] copy = list()
copy[0] = 99
@println(list())
Text original = @as(Text, "same")
@println(@as(str, @as(Text, original)))
@println(@as(int?, @as(Maybe, none)) else 9)
@println(@as(int?, @as(Maybe, 7)) else 9)
@println(@as(int[], @as(List, [])))
"#,
        &["text", "list", "pair", "table", "record"],
        "a\0b[1, 2](x, 3)[x: 4]Record{.values = [5]}\n[1, 2]\nsame\n9\n7\n[]\n",
    );
}

#[test]
fn embedded_nuls_and_unicode_escapes_agree_at_runtime() {
    folded(
        r#"
fn text() str {
    mut str value = "a\u{0}🍪"
    value[0] = '\u{0}'
    return value <> "\u{0}z"
}
fn octet(byte n) char { return @as(char, n) }
@println(text())
@println(text().len)
@println(@as(byte[], text()))
@println("\u{0}" in text())
@println("a\u{0}b" == "a\u{0}c")
@println(octet(0))
@println(octet(255))
@println("\u{7b}literal}")
@println('\e' == '\u{00001B}')
"#,
        &["text", "octet"],
        "\0\0🍪\0z\n5\n[0, 0, 240, 159, 141, 170, 0, 122]\ntrue\nfalse\n\0\nÿ\n{literal}\ntrue\n",
    );
}

#[test]
fn target_is_a_compile_time_value() {
    folded(
        "fn platform() (str, str) { return @target() };@println(platform())",
        &["platform"],
        "(macos, arm64)\n",
    );
}

#[test]
fn nested_assignment_indices_are_evaluated_once_during_folding() {
    folded(
        r"
fn nested_places() (int, int) {
    mut int calls = 0
    fn index() int { calls = calls + 1;return 0 }
    mut int[][] grid = [[1]]
    grid[index()][index()] = 7
    return calls, grid[0][0]
}
@println(nested_places())
",
        &["nested_places"],
        "(2, 7)\n",
    );
}

#[test]
fn composite_rhs_snapshot_survives_source_and_tuple_ancestor_changes_during_folding() {
    folded(
        r#"
struct Payload { [str]int[] rows }
fn snapshot() (int, int[], int[], int) {
    mut (Payload[], int) state = (
        [Payload{.rows = ["k": [1, 2]]}, Payload{.rows = ["k": [3]]}], 10
    )
    mut int trace = 0
    fn rhs() Payload { trace = trace * 10 + 1;return state[0][0] }
    fn target() int {
        trace = trace * 10 + 2
        state[0][0].rows["k"][0] = 99
        state = ([Payload{.rows = ["k": [7]]}, Payload{.rows = ["k": [8]]}], 20)
        return 1
    }
    state[0][target()] = rhs()
    return trace, state[0][0].rows["k"], state[0][1].rows["k"], state[1]
}
@println(snapshot())
"#,
        &["snapshot"],
        "(12, [7], [1, 2], 20)\n",
    );
}

#[test]
fn nested_traversal_snapshots_are_independent_during_folding() {
    folded(
        r"
fn snapshots() (uint[], int) {
    mut int[][] rows = [[1, 2], [3]]
    mut uint[] visited = []
    mut int total = 0
    for i in rows {
        for j in rows[i] {
            visited = visited <> [i * 10 + j]
            if i == 0 and j == 0 {
                true -> { rows = [[10], [20, 30, 40], [50]] }
                false -> {}
            }
            if i == 1 {
                true -> { total = total + rows[i][j] }
                false -> {}
            }
        }
    }
    return visited, total
}
@println(snapshots())
",
        &["snapshots"],
        "([0, 1, 10, 11, 12], 90)\n",
    );
}

#[test]
fn mixed_nested_assignment_places_preserve_folding_effect_order() {
    folded(
        r#"
struct Bucket { int[] values str text }
fn mixed_places() (int, int, str, int) {
    mut int trace = 0
    fn index(int marker) int { trace = trace * 10 + marker;return 0 }
    fn key() str { trace = trace * 10 + 1;return "item" }
    fn replacement() int { trace = trace * 10 + 3;return 9 }
    mut [str]Bucket buckets = ["item": Bucket{.values = [1], .text = "X"}]
    buckets[key()].values[index(2)] = replacement()
    buckets[key()].text[index(4)] = 'Z'
    mut (Bucket, int) pair = (Bucket{.values = [1], .text = "Y"}, 2)
    pair[0].values[index(5)] = 8
    return trace, buckets["item"].values[0], buckets["item"].text, pair[0].values[0]
}
fn deep_places() (int, int, int, int) {
    mut int calls = 0
    fn index() int { calls = calls + 1;return 0 }
    mut int[][][] cube = [[[1, 2]]]
    int[][][] original = cube
    cube[index()][index()][index()] = 9
    cube[index()][index()][$] = 8
    return calls, cube[0][0][0], cube[0][0][1], original[0][0][0]
}
@println(mixed_places())
@println(deep_places())
"#,
        &["mixed_places", "deep_places"],
        "(312145, 9, Z, 8)\n(5, 9, 8, 1)\n",
    );
}

#[test]
fn nested_map_insertions_still_fold_and_evaluate_rhs_before_target() {
    folded(
        r#"
struct Holder { [str]int entries }
fn insert() (int, int) {
    mut int trace = 0
    fn index() int { trace = trace * 10 + 1;return 0 }
    fn replacement() int { trace = trace * 10 + 2;return 7 }
    mut Holder[] holders = [Holder{.entries = []}]
    holders[index()].entries["new"] = replacement()
    return trace, holders[0].entries["new"]
}
@println(insert())
"#,
        &["insert"],
        "(21, 7)\n",
    );
}

#[test]
fn assignment_binding_replacements_fold_without_stale_storage() {
    folded(
        r#"
struct Bucket { int[] values }
fn replacements() (int[], int[][], str, int) {
    mut int[] values = [1]
    fn replace() int { values = [2, 3];return 7 }
    values[1] = replace()
    mut int[][] rows = [[1]]
    fn index() int { rows = [[2, 3]];return 1 }
    rows[0][index()] = 8
    mut str text = "x"
    fn character() char { text = "ab";return 'Z' }
    text[$] = character()
    mut [str]Bucket buckets = ["item": Bucket{.values = [1]}]
    fn insert() int { buckets = ["item": Bucket{.values = [2]}, "new": Bucket{.values = [3]}];return 0 }
    buckets["item"].values[insert()] = 9
    return values, rows, text, buckets["item"].values[0]
}
@println(replacements())
"#,
        &["replacements"],
        "([2, 7], [[2, 8]], aZ, 9)\n",
    );
}

#[test]
fn later_assignment_indices_can_repair_missing_ancestors() {
    folded(
        r#"
struct Bucket { int[] values }
fn repairs() (int[][], int[]) {
    mut int[][] rows = []
    fn repair() int { rows = [[1]];return 0 }
    rows[0][repair()] = 7
    int[][] original = rows
    rows = []
    rows[0][[repair()][$]] = 8
    mut [str]Bucket buckets = []
    fn restore() int { buckets = ["item": Bucket{.values = [1]}];return 0 }
    buckets["item"].values[restore()] = 9
    return original <> rows, buckets["item"].values
}
@println(repairs())
"#,
        &["repairs"],
        "([[7], [8]], [9])\n",
    );
    let source = "mut int[][] rows = [];fn fail() int { return 1 / 0 };rows[0][fail()] = 7";
    let error = compile_fixture(source, Path::new("index-failure.nc"), true).unwrap_err();
    assert!(error.to_string().contains("constant evaluation failed"));
    assert_eq!(&source[error.0[0].span.clone()], "1 / 0");
}

#[test]
fn loops_labels_and_local_places_are_evaluated() {
    folded(
        r#"
struct State { int sum int[] values }
fn compute() int {
    mut State s = State{.sum = 0, .values = [1, 2, 3]}
    for i in s.values { s.values[i] = s.values[i] + 1;s.sum = s.sum + s.values[i] }
    s.values[$] = 10
    mut [str]int counts = ["a": 2]
    counts["b"] = 3
    for key in counts { s.sum = s.sum + counts[key] }
    outer: for i in [1, 2, 3] {
        for j in [1, 2, 3] {
            if i == 1 { true -> { continue :outer } false -> {} }
            if j == 1 { true -> { break :outer } false -> {} }
            s.sum = s.sum + 1
        }
    }
    done: if true { true -> { break :done } false -> {} }
    return s.sum + s.values[$]
}
fn text() str {
    mut str s = "a🍪c"
    for i in s { if i == 1 { true -> { s[i] = '界' } false -> {} } }
    s[$] = 'd'
    return s
}
@println(compute())
@println(text())
"#,
        &["compute", "text"],
        "25\na界d\n",
    );
}

#[test]
fn specified_for_indices_keys_and_empty_containers_fold() {
    folded(
        r#"
fn iteration() str {
    (int[], str, [str]int) inputs = ([9, 4, 7], "a🍪z", ["red": 10, "blue": 20])
    fn collect() str {
        int[] values, str text, [str]int entries = inputs
        mut uint array_indices = 0
        mut int array_values = 0
        for i in values {
            uint index = i
            array_indices = array_indices + index
            array_values = array_values + values[index]
        }
        mut uint string_indices = 0
        mut str characters = ""
        for i in text {
            uint index = i
            string_indices = string_indices + index
            characters = characters <> @as(str, text[index])
        }
        mut uint key_lengths = 0
        mut int map_values = 0
        for key in entries {
            str typed_key = key
            key_lengths = key_lengths + typed_key.len
            map_values = map_values + entries[typed_key]
        }
        [int]int numbers = [-2: 6, 5: 9]
        mut int numeric_keys = 0
        mut int numeric_values = 0
        for key in numbers {
            int typed_key = key
            numeric_keys = numeric_keys + typed_key
            numeric_values = numeric_values + numbers[typed_key]
        }
        int[] empty_array = []
        str empty_string = ""
        [str]int empty_map = []
        mut uint empty_visits = 0
        for i in empty_array { empty_visits = empty_visits + 1 }
        for i in empty_string { empty_visits = empty_visits + 1 }
        for key in empty_map { empty_visits = empty_visits + 1 }
        return "{(array_indices, array_values, string_indices, characters, key_lengths, map_values, numeric_keys, numeric_values, empty_visits)}"
    }
    return collect()
}
@println(iteration())
"#,
        &["iteration"],
        "(3, 20, 3, a🍪z, 7, 30, 3, 15, 0)\n",
    );
}

#[test]
fn original_array_indices_fold_across_append_and_length_changing_replacement() {
    folded(
        r"
fn append_values() int[] {
    mut int[] values = [1, 2, 3, 4, 5]
    uint initial_count = values.len
    for i in values {
        if i >= initial_count { true -> { return [-1] } false -> {} }
        values = values <> [values[i] * 10]
    }
    return values
}
fn grow_values() (uint, uint, int, uint) {
    mut int[] values = [1, 2, 3]
    uint initial_count = values.len
    mut uint visits = 0
    mut uint indices = 0
    mut int total = 0
    for i in values {
        if i >= initial_count { true -> { return (0, 0, -1, 0) } false -> {} }
        values = [10, 20, 30] <> values
        visits = visits + 1
        indices = indices + i
        total = total + values[i]
    }
    return (visits, indices, total, values.len)
}
fn shrink_values() (uint, uint, int) {
    mut int[] values = [1, 2, 3, 4]
    uint initial_count = values.len
    mut uint visits = 0
    mut uint indices = 0
    mut int total = 0
    for i in values {
        if i >= initial_count { true -> { return (0, 0, -1) } false -> {} }
        values = [9]
        visits = visits + 1
        indices = indices + i
        if i < values.len { true -> { total = total + values[i] } false -> {} }
    }
    return (visits, indices, total)
}
@println(append_values())
@println(grow_values())
@println(shrink_values())
",
        &["append_values", "grow_values", "shrink_values"],
        "[1, 2, 3, 4, 5, 10, 20, 30, 40, 50]\n(3, 3, 60, 12)\n(4, 6, 9)\n",
    );
}

#[test]
fn original_map_keys_and_string_indices_fold_across_binding_mutation() {
    folded(
        r#"
fn insert_entries() (uint, int) {
    mut [str]int entries = ["a": 1, "b": 2]
    mut uint visits = 0
    mut int total = 0
    for key in entries {
        if key != "a" and key != "b" { true -> { return (0, -1) } false -> {} }
        entries["a"] = 10
        entries["b"] = 20
        entries["new"] = 30
        visits = visits + 1
        total = total + entries[key]
    }
    return (visits, total)
}
fn replace_entries() (uint, int) {
    mut [str]int entries = ["a": 1, "b": 2, "c": 3]
    mut uint visits = 0
    mut int total = 0
    for key in entries {
        if key != "a" and key != "b" and key != "c" {
            true -> { return (0, -1) } false -> {}
        }
        entries = ["a": 10, "b": 20, "c": 30, "new": 40]
        visits = visits + 1
        total = total + entries[key]
    }
    return (visits, total)
}
fn append_text() str {
    mut str text = "a🍪z"
    uint initial_count = text.len
    for i in text {
        if i >= initial_count { true -> { return "bad" } false -> {} }
        text = text <> @as(str, text[i])
    }
    return text
}
fn replace_text() str {
    mut str text = "abc"
    uint initial_count = text.len
    mut str read = ""
    for i in text {
        if i >= initial_count { true -> { return "bad" } false -> {} }
        text = "X界Z" <> text
        read = read <> @as(str, text[i])
    }
    return "{read}:{text.len}"
}
fn shrink_text() (uint, uint, str) {
    mut str text = "abc"
    uint initial_count = text.len
    mut uint visits = 0
    mut uint indices = 0
    mut str read = ""
    for i in text {
        if i >= initial_count { true -> { return (0, 0, "bad") } false -> {} }
        text = "Q"
        visits = visits + 1
        indices = indices + i
        if i < text.len { true -> { read = read <> @as(str, text[i]) } false -> {} }
    }
    return (visits, indices, read)
}
@println(insert_entries())
@println(replace_entries())
@println(append_text())
@println(replace_text())
@println(shrink_text())
"#,
        &[
            "insert_entries",
            "replace_entries",
            "append_text",
            "replace_text",
            "shrink_text",
        ],
        "(2, 30)\n(3, 60)\na🍪za🍪z\nX界Z:12\n(3, 3, Q)\n",
    );
}

#[test]
fn specified_nested_loop_jumps_returns_and_condition_effects_fold() {
    folded(
        r"
fn for_labels() int {
    mut int total = 0
    rows: for i in [10, 20, 30, 40] {
        mut int j = 0
        while j < 4 {
            j = j + 1
            if j == 1 { true -> { continue } false -> {} }
            if i == 1 { true -> { continue :rows } false -> {} }
            if i == 2 and j == 3 { true -> { break :rows } false -> {} }
            if j == 4 { true -> { break } false -> {} }
            total = total + @as(int, i) * 10 + j
        }
        total = total + 100
    }
    return total
}
fn while_labels() int {
    mut int round = 0
    mut int total = 0
    rounds: while round < 4 {
        round = round + 1
        for j in [10, 20, 30, 40] {
            if j == 0 { true -> { continue } false -> {} }
            if round == 1 { true -> { continue :rounds } false -> {} }
            if round == 3 and j == 2 { true -> { break :rounds } false -> {} }
            if j == 2 { true -> { break } false -> {} }
            total = total + round * 10 + @as(int, j)
        }
        total = total + 100
    }
    return total
}
fn find(bool found) int {
    for i in [10, 20, 30] {
        mut int j = 0
        while j < 3 {
            j = j + 1
            if found and i == 1 and j == 2 {
                true -> { return @as(int, i) * 10 + j }
                false -> {}
            }
        }
    }
    return -1
}
fn reevaluate() (int, int, int) {
    mut int step = 0
    mut int checks = 0
    mut int total = 0
    fn condition() bool {
        checks = checks + 1
        return step < 3
    }
    while condition() {
        step = step + 1
        total = total + step * 10
    }
    return (step, checks, total)
}
@println(for_labels())
@println(while_labels())
@println(find(true))
@println(find(false))
@println(reevaluate())
",
        &["for_labels", "while_labels", "find", "reevaluate"],
        "127\n152\n12\n-1\n(3, 4, 60)\n",
    );
}

// Raw decomposed Unicode must match runtime bytes without normalization.
#[test]
fn typed_operations_match_runtime_semantics() {
    folded(
        include_str!("fixtures/unicode/typed_operations_match_runtime_semantics.nc"),
        &[
            "choose",
            "fallback",
            "branch",
            "length",
            "first",
            "equal",
            "bytes",
            "shift",
            "bits",
            "minimum",
            "nominal_power",
        ],
        "42\n7\n5\n2\n2\n🍪\ntrue\n[2, 1, 0, 0, 0, 0, 0, 0]\n12\n18446744073709551615\n-9223372036854775808\n1\n",
    );
    for source in [
        "fn f(uint n) uint { return n - 1 };@println(f(0))",
        "fn f(byte n) byte { return n << 8 };@println(f(1))",
        "fn f(float n) uint { return @as(uint, n) };@println(f(-0.5))",
        "fn f(int n) int { return -n };@println(f(-9223372036854775808))",
    ] {
        let error = compile_fixture(source, Path::new("bad.nc"), true).unwrap_err();
        assert!(
            error.to_string().contains("constant evaluation failed"),
            "{error}"
        );
    }
}

#[test]
fn folding_preserves_nested_optional_and_container_promotions() {
    folded(
        r#"
struct Wrapped { int? value }
enum Choice { Value(int?) }
fn optional(int? n) int? { return n }
fn nested(int? n) int?? { return n }
fn present(int?? n) bool { int? inner = n else { return false };return true }
fn field(Wrapped n) int { return n.value else 10 }
fn entry([str]int? n) int { return n["a"] else 10 }
fn variant(Choice c) int { if c { Choice.Value(n) -> { return n else 10 } } }
int? global = 5
@println(global == optional(5))
@println(present(nested(none)))
@println(field(Wrapped{.value = 8}))
@println(entry(["a":9]))
@println(variant(Choice.Value(7)))
"#,
        &["optional", "nested", "present", "field", "entry", "variant"],
        "true\ntrue\n8\n9\n7\n",
    );
}

#[test]
fn partial_tuple_destructuring_preserves_context_and_values() {
    folded(
        r"
int first, (uint, byte) rest = (1, 2, 3)
int _, (int?, uint[]) optional = (0, none, [])
fn sum((int, int, int) values) int {
    int a, (int, int) b = values
    return a + b[0] + b[1]
}
fn nested() int {
    (int, (int, int)) a, int b = (1, 2, 3, 4)
    return a[0] + a[1][0] + a[1][1] + b
}
@println(first)
@println(rest)
@println(sum((4, 5, 6)))
@println(nested())
@println(optional)
",
        &["sum", "nested"],
        "1\n(2, 3)\n15\n10\n(none, [])\n",
    );
}

#[test]
fn tuple_bindings_shadow_and_discard_without_stale_constants() {
    folded(
        r#"
int first = 99
int first, str second = (2, "three")
fn sum((int, int) pair) int { int a, int b = pair;return a + b }
int _, int last = (3, 4)
@println(first)
@println(second)
@println(last)
@println(sum((5,6)))
"#,
        &["sum"],
        "2\nthree\n4\n11\n",
    );
}

#[test]
fn fibonacci_uses_each_numeric_types_range() {
    folded(
        r"
fn signed(int n) int { if n { 0, 1 -> { return n } _ -> { return signed(n-1) + signed(n-2) } } }
fn unsigned(uint n) uint { if n { 0, 1 -> { return n } _ -> { return unsigned(n-1) + unsigned(n-2) } } }
fn octet(byte n) byte { if n { 0, 1 -> { return n } _ -> { return octet(n-1) + octet(n-2) } } }
fn real(float n) float { if n { 0.0, 1.0 -> { return n } _ -> { return real(n-1.0) + real(n-2.0) } } }
@println(signed(92))
@println(unsigned(93))
@println(octet(13))
@println(real(20.0))
",
        &["signed", "unsigned", "octet", "real"],
        "7540113804746346429\n12200160415121876738\n233\n6765.0\n",
    );
    for (ty, maximum) in [
        ("int", "9223372036854775807"),
        ("uint", "18446744073709551615"),
        ("byte", "255"),
    ] {
        let source =
            format!("fn overflow({ty} n) {ty} {{ return n + 1 }};@println(overflow({maximum}))");
        let error = compile_fixture(&source, Path::new("overflow.nc"), true).unwrap_err();
        assert!(
            error.to_string().contains("constant evaluation failed"),
            "{error}"
        );
    }
}

#[test]
fn scalar_nominal_and_composite_values_are_not_type_whitelisted() {
    folded(
        r#"
type Count = uint
struct Pair { int first str second }
enum Choice { Empty Number(uint) }
fn truth(bool value) bool { return not value }
fn character(char value) char { return value }
fn text(str value) str { return value <> "!" }
fn count(Count value) Count { return @as(Count, @as(uint, value) + 1) }
fn array(int[] value) int[] { return value <> [3] }
fn tuple((int, str) value) (int, str) { return value }
fn mapping([str]int value) [str]int { return value }
fn record(Pair value) Pair { return value }
fn variant(uint value) Choice { return Choice.Number(value) }
fn optional(int value) int? { return value }
fn nothing() int? { return none }
fn okay(int value) int! { return value }
fn empty() int[] { return [] }
fn add(int value) int { return value + 1 }
fn callback((fn(int) int) operation, int value) int { return operation(value) }
@println(truth(false))
@println(character('🍪'))
@println(text("yes"))
@println(@as(uint, count(4)))
@println(array([1,2]))
@println(tuple((1,"two")))
@println(mapping(["one":1]))
@println(record(Pair{.first=1,.second="two"}))
@println(variant(7))
@println(optional(8))
@println(nothing())
int value = try okay(9)
@println(value)
@println(empty())
@println(callback(add, 10))
"#,
        &[
            "truth",
            "character",
            "text",
            "count",
            "array",
            "tuple",
            "mapping",
            "record",
            "variant",
            "optional",
            "nothing",
            "empty",
            "callback",
        ],
        "true\n🍪\nyes!\n5\n[1, 2, 3]\n(1, two)\n[one: 1]\nPair{.first = 1, .second = two}\nChoice.Number(7)\n8\nnone\n9\n[]\n11\n",
    );
}

#[test]
fn arithmetic_shifts_fold_with_signed_boundary_semantics() {
    folded(
        r"
fn right(int value, int count) int { return value >> count }
fn left(int value, int count) int { return value << count }
@println(right(-3, 1))
@println(right(-4, 1))
@println(right(-9223372036854775808, 0))
@println(right(-9223372036854775808, 1))
@println(right(-9223372036854775807, 1))
@println(right(-9223372036854775808, 63))
@println(right(-1, 63))
@println(right(9223372036854775807, 63))
@println(left(-3, 1))
@println(left(-1, 63))
",
        &["right", "left"],
        "-2\n-2\n-9223372036854775808\n-4611686018427387904\n-4611686018427387904\n-1\n-1\n0\n-6\n-9223372036854775808\n",
    );
}

#[test]
fn shared_mutable_closures_fold_without_reusing_stateful_calls() {
    folded(
        r"
struct Counter { (fn() int) read (fn(int) void) write (fn() int) next }
fn counter(int initial) Counter {
    mut int n = initial
    return Counter{
        .read = fn() int { return n },
        .write = fn(int value) { n = value },
        .next = fn() int { n = n + 1;return n }
    }
}
fn apply((fn() int) callback) int { return callback() }
fn compute() int {
    Counter first = counter(0)
    Counter alias = first
    Counter second = counter(0)
    int a = apply(first.next)
    int b = apply(alias.next)
    int c = apply(second.next)
    first.write(10)
    int d = first.read()
    first.write(20)
    return a * 10000 + b * 1000 + c * 100 + d + first.read()
}
fn branches() int {
    mut int a, int b = (1, 2)
    fn add() { a = a + b }
    if true { true -> { add() } false -> {} }
    { mut int a = 100;fn local() { a = a + 1 };local() }
    b = 4
    add()
    return a + b
}
fn composites() int {
    mut int[] values = [1]
    fn update() { values[0] = values[0] + 1 }
    int[] copy = values
    update()
    values = [10]
    update()
    return values[0] + copy[0]
}
@println(compute())
@println(branches())
@println(composites())
",
        &["compute", "branches", "composites", "counter", "apply"],
        "12130\n11\n12\n",
    );
}

#[test]
fn shared_closures_preserve_runtime_state_and_effects() {
    folded(
        r#"
fn make() (fn() int) {
    mut int n = 0
    return fn() int { n = n + 1;return n }
}
(fn() int) global = make()
@println(global())
@println(global())
fn effectful() int {
    mut int n = 0
    fn next() int { @print("effect:");n = n + 1;return n }
    return next() + next()
}
@println(effectful())
"#,
        &[],
        "1\n2\neffect:effect:3\n",
    );
}

#[test]
fn shared_cells_survive_loop_scopes_errors_and_container_callbacks() {
    folded(
        r#"
fn loops() int {
    mut (fn() int)[] callbacks = []
    for i in [1, 2, 3] {
        mut int n = @as(int, i)
        callbacks = callbacks <> [fn() int { n = n + 1;return n }]
    }
    return callbacks[0]() * 100 + callbacks[0]() * 10 + callbacks[1]()
}
fn errors() int {
    mut int n = 0
    fn fail() int! { n = n + 1;throw "failure" }
    int a = fail() catch _ { n }
    int b = fail() catch _ { n }
    return a * 10 + b
}
struct Box { (fn() int) next }
fn step(Box box) int { return box.next() }
fn boxed() int {
    mut int n = 0
    Box box = Box{.next = fn() int { n = n + 1;return n }}
    return step(box) * 10 + step(box)
}
fn pattern() int {
    mut int expected = 2
    fn bump() { expected = expected + 1 }
    bump()
    return if 3 { expected -> { 7 } _ -> { 0 } }
}
@println(loops())
@println(errors())
@println(boxed())
@println(pattern())
"#,
        &["loops", "errors", "boxed", "step", "pattern"],
        "122\n12\n12\n7\n",
    );
}

#[test]
fn exhaustive_byte_patterns_fold_every_value_without_a_wildcard() {
    let mut source = String::from("fn classify(byte value) int { if value {\n");
    for value in 0..=255 {
        let literal = match value % 3 {
            0 => format!("0x{value:02X}"),
            1 => format!("0b{value:08b}"),
            _ => value.to_string(),
        };
        writeln!(source, "{literal} -> {{ return {} }}", 255 - value).unwrap();
    }
    source.push_str("} }\n");
    let mut expected = String::new();
    for value in 0..=255 {
        writeln!(source, "@println(classify({value}))").unwrap();
        writeln!(expected, "{}", 255 - value).unwrap();
    }
    folded(&source, &["classify"], &expected);
}

#[test]
fn exact_array_patterns_fold_empty_fixed_and_dynamic_lengths() {
    folded(
        r"
fn empty(int[0] values) int {
    if values { [] -> { return 7 } }
}
fn dynamic(int[] values) int {
    if values {
        [] -> { return 0 }
        [1] -> { return 1 }
        [1, 2] -> { return 2 }
        [1, a, b] -> { return a * 10 + b }
        _ -> { return -1 }
    }
}
fn fixed(int[2] values) int {
    if values {
        [1, 2] -> { return 12 }
        [1, n] -> { return n }
        [a, b] -> { return a * 10 + b }
    }
}
@println(empty([]))
@println(dynamic([]))
@println(dynamic([1]))
@println(dynamic([2]))
@println(dynamic([1, 2]))
@println(dynamic([1, 3]))
@println(dynamic([1, 2, 3]))
@println(dynamic([1, 2, 3, 4]))
@println(fixed([1, 2]))
@println(fixed([1, 9]))
@println(fixed([4, 5]))
",
        &["empty", "dynamic", "fixed"],
        "7\n0\n1\n-1\n2\n-1\n23\n-1\n12\n9\n45\n",
    );
}

#[test]
fn exact_string_and_composite_patterns_fold_with_nuls_and_unicode() {
    folded(
        r#"
struct Record { str name int count }
fn text(str value) int {
    if value {
        "" -> { return 0 }
        "a" -> { return 1 }
        "a\u{0}🍪" -> { return 2 }
        "界🍪" -> { return 3 }
        _ -> { return -1 }
    }
}
fn tuple((str, int) value) int {
    if value {
        ("a\u{0}🍪", 2) -> { return 20 }
        ("a\u{0}🍪", n) -> { return n }
        (_, _) -> { return -1 }
    }
}
fn record(Record value) int {
    if value {
        Record{.name = "界🍪", .count = 2} -> { return 20 }
        Record{.name = "界🍪", .count = n} -> { return n }
        Record{.name = _, .count = _} -> { return -1 }
    }
}
@println(text(""))
@println(text("a"))
@println(text("a\u{0}🍪"))
@println(text("a\u{0}界"))
@println(text("a\u{0}🍪z"))
@println(text("a\u{0}"))
@println(text("界🍪"))
@println(text("🍪界"))
@println(tuple(("a\u{0}🍪", 2)))
@println(tuple(("a\u{0}🍪", 7)))
@println(tuple(("a\u{0}界", 2)))
@println(record(Record{.name = "界🍪", .count = 2}))
@println(record(Record{.name = "界🍪", .count = 7}))
@println(record(Record{.name = "🍪界", .count = 2}))
"#,
        &["text", "tuple", "record"],
        "0\n1\n2\n-1\n-1\n-1\n3\n-1\n20\n7\n-1\n20\n7\n-1\n",
    );
}

#[test]
fn float_patterns_fold_with_ieee_equality() {
    folded(
        r#"
fn classify(float value) str {
    return if value {
        NaN -> { "unreachable" }
        inf -> { "positive" }
        -inf -> { "negative" }
        0.0 -> { "zero" }
        _ -> { "other" }
    }
}
@println(classify(NaN))
@println(classify(inf))
@println(classify(-inf))
@println(classify(0.0))
@println(classify(-0.0))
@println(classify(1.0))
"#,
        &["classify"],
        "other\npositive\nnegative\nzero\nzero\nother\n",
    );
}

#[test]
fn enum_payload_patterns_fold_specific_before_irrefutable_branches() {
    folded(
        r"
enum Choice { Number(int) Empty }
fn choose(Choice value) int {
    if value {
        Choice.Number(0) -> { return 100 }
        Choice.Number(n) -> { return n }
        Choice.Empty -> { return -1 }
    }
}
@println(choose(Choice.Number(0)))
@println(choose(Choice.Number(7)))
@println(choose(Choice.Number(-3)))
@println(choose(Choice.Empty))
",
        &["choose"],
        "100\n7\n-3\n-1\n",
    );
}

#[test]
fn pattern_only_captures_fold_with_current_values_and_lexical_bindings() {
    folded(
        r"
int expected = 2
fn global(int value) bool {
    return if value { expected -> { true } _ -> { false } }
}
fn local() int {
    mut int expected = 1
    fn matches(int[] value) bool {
        return if value { [expected, _] -> { true } _ -> { false } }
    }
    expected = 3
    int a = if matches([3, 9]) { true -> { 10 } false -> { 0 } }
    int b = if matches([1, 9]) { true -> { 100 } false -> { 1 } }
    return a + b
}
@println(global(1))
@println(global(2))
@println(local())
",
        &["global", "local"],
        "false\ntrue\n11\n",
    );
}
