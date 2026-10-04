use std::{fs, path::Path, process::Command};

use ncc::ast::{Expr, Item, Module, Stmt};

fn optimized(source: &str) -> Module {
    let path = Path::new("output.nc");
    let module = ncc::parser::parse_at(ncc::lexer::lex(source).unwrap(), path).unwrap();
    let checked = ncc::sema::check(ncc::generics::specialize(module).unwrap(), path).unwrap();
    ncc::optimizer::optimize(checked).unwrap()
}

fn recorded(module: &Module) -> Option<Vec<(&str, &str)>> {
    module
        .items
        .iter()
        .map(|item| {
            let Item::Statement(statement) = item else {
                return None;
            };
            let Stmt::Expr(expression) = statement.unlocated() else {
                return None;
            };
            let Expr::Call { callee, args, .. } = expression.unlocated() else {
                return None;
            };
            let Expr::Name(name) = callee.unlocated() else {
                return None;
            };
            let [Expr::String(bytes)] = args.as_slice() else {
                return None;
            };
            Some((name.as_str(), bytes.as_str()))
        })
        .collect()
}

fn compare_output(source: &str, expected: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("output.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "release={release}");
    }
}

#[test]
fn original_strings_example_with_reduced_work_has_only_final_output() {
    let source = include_str!("../nc-tests/strings.nc").replace("100000000", "4096");
    let values: Vec<_> = (3072..4096).map(|value| format!("\"{value}\"")).collect();
    let expected = format!("[{}]", values.join(", "));
    let module = optimized(&source);
    assert_eq!(
        recorded(&module),
        Some(vec![("@println", expected.as_str())])
    );
    let c = ncc::compile_source_with_options(&source, Path::new("strings.nc"), true).unwrap();
    assert!(!c.contains("nc_fn_from_int"));
    assert!(!c.contains("nc_var_buf"));
    assert!(!c.contains("nc_unicode_ranges"));
    assert!(!c.contains("} goto "));
    compare_output(&source, format!("{expected}\n").as_bytes());
}

#[test]
fn string_buffer_mutation_becomes_only_final_output() {
    let source = r#"
mut str[] buf = []
mut int i = 0
while i < 8 {
    buf = buf <> [""]
    i = i + 1
}
fn from_int(int n) {
    mut int i = 0
    while i < n {
        buf[i & 7] = "{i}"
        i = i + 1
    }
}
from_int(32)
@println(buf)
"#;
    let expected = r#"["24", "25", "26", "27", "28", "29", "30", "31"]"#;
    let module = optimized(source);
    assert_eq!(recorded(&module).unwrap(), [("@println", expected)]);
    let c = ncc::compile_source_with_options(source, Path::new("output.nc"), true).unwrap();
    for marker in ["nc_fn_from_int", "nc_g_buf", "nc_unicode_ranges", "while ("] {
        assert!(!c.contains(marker), "computation retained: {marker}");
    }
    compare_output(source, format!("{expected}\n").as_bytes());
}

#[test]
fn nested_prints_snapshot_arguments_and_repeat_global_effects() {
    let source = r#"
mut int[] values = [0]
fn step() int {
    values[0] = values[0] + 1
    @print("inner", values[0], "|")
    return values[0]
}
fn shadow() {
    int[] values = [99]
    @println(values, step())
}
@println(values, step(), values, step())
shadow()
@print()
@println()
"#;
    let module = optimized(source);
    assert_eq!(
        recorded(&module).unwrap(),
        [
            ("@print", "inner1|"),
            ("@print", "inner2|"),
            ("@println", "[0]1[1]2"),
            ("@print", "inner3|"),
            ("@println", "[99]3"),
            ("@print", ""),
            ("@println", ""),
        ]
    );
    compare_output(source, b"inner1|inner2|[0]1[1]2\ninner3|[99]3\n\n");
}

#[test]
fn output_flattens_boundaries_and_formats_active_payloads() {
    let source = r#"
struct Record { int value str text }
enum Choice { Value(str) }
fn failed() int! { throw "bad" }
fn success() int! { return 7 }
str text = "a" <> "\u{301}" <> "\u{0}"
int? absent = none
@println(text, "|", text.len)
@println([text], (true, 2), Record{.value = 3, .text = text}, Choice.Value(text))
@println(failed(), "|", success(), "|", absent)
@println(0.0 / 0.0, "|", 1.0 / 0.0, "|", -1.0 / 0.0)
"#;
    let module = optimized(source);
    assert!(recorded(&module).is_some());
    compare_output(
        source,
        "a\u{301}\0|3\n[\"a\u{301}\0\"](true, 2)Record{.value = 3, .text = a\u{301}\0}Choice.Value(\"a\u{301}\0\")\nerror: bad|7|none\nNaN|inf|-inf\n".as_bytes(),
    );
}

#[test]
fn unknown_state_and_unsupported_effects_roll_back_all_recorded_output() {
    for tail in [
        "@println(@args())",
        "@println(@env())",
        "@eprintln(\"stderr\")",
        "fn escaped = fn() { @eprint(\"unknown effect\") };escaped()",
        "fut str[] future = async @args();@println(await future)",
        "mut int missing = 0;int invalid = [1][@as(uint, missing + 2)];@println(invalid)",
    ] {
        let source = format!(
            "mut int value = 0\nwhile value < 3 {{ value = value + 1 }}\n@println(value)\n{tail}"
        );
        let module = optimized(&source);
        assert!(recorded(&module).is_none(), "transaction committed: {tail}");
        let global = module.items.iter().find_map(|item| match item {
            Item::Global(v) if v.binding_names().contains(&"value") => Some(v),
            _ => None,
        });
        assert!(global.is_some(), "global storage removed: {tail}");
        // The old initial call-free prefix remains available after rollback.
        assert!(matches!(global.unwrap().value.unlocated(), Expr::Int(n) if n == "3"));
    }
}

#[test]
fn tests_are_not_removed_by_whole_program_precomputation() {
    let module = optimized("mut int value = 1;@println(value);test \"kept\" { assert value == 1 }");
    assert!(
        module
            .items
            .iter()
            .any(|item| matches!(item, Item::Test { .. }))
    );
    assert!(recorded(&module).is_none());
}

#[test]
fn closures_and_loop_prints_are_recorded_in_execution_order() {
    let source = r"
mut int value = 0
fn read = fn() int { return value }
for i in [0, 0, 0] {
    @print(read())
    value = value + 1
}
@println(read())
";
    let module = optimized(source);
    assert_eq!(
        recorded(&module).unwrap(),
        [
            ("@print", "0"),
            ("@print", "1"),
            ("@print", "2"),
            ("@println", "3")
        ]
    );
    compare_output(source, b"0123\n");
    assert!(
        optimized("mut int value = 0;while value < 3 { value = value + 1 }")
            .items
            .is_empty()
    );
}

#[test]
fn native_calls_are_unknown_and_never_executed() {
    let module = optimized(
        r#"
extern "not-executed.c" as native { fn input() int = "input" }
@println("before")
int value = input()
@println(value)
"#,
    );
    assert!(recorded(&module).is_none());
    assert!(
        module
            .items
            .iter()
            .any(|item| matches!(item, Item::Extern { .. }))
    );
}

#[test]
fn runtime_failure_preserves_prior_output_and_stops_later_output() {
    let source = r#"
@println("before")
mut int[] values = [1]
mut uint index = 2
@println(values[index])
@println("after")
"#;
    assert!(recorded(&optimized(source)).is_none());
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
        assert_eq!(output.stdout, b"before\n");
        assert_ne!(output.stderr, [] as [u8; 0]);
    }
}

#[test]
fn finite_float_output_uses_the_evaluators_exact_formatting() {
    let source = "@println((0.1, -0.0, 1.0, 0.00001, 100000000000000000.0))";
    assert!(recorded(&optimized(source)).is_some());
    compare_output(
        source,
        b"(0.10000000000000001, -0.0, 1.0, 1.0000000000000001e-05, 1e+17)\n",
    );
}

#[test]
fn known_arithmetic_failure_keeps_existing_diagnostics() {
    let source = "@println(\"before\");mut int n = 0;@println(1 / n)";
    let error = ncc::compile_source_with_options(source, Path::new("output.nc"), true).unwrap_err();
    assert!(error.to_string().contains("constant evaluation failed"));
}
