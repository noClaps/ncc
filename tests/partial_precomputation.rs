use ncc::ast::{Expr, Item, Module, Stmt};
use std::{fs, path::Path, process::Command};

fn optimized(source: &str) -> Module {
    let path = Path::new("partial.nc");
    let module = ncc::parser::parse_at(ncc::lexer::lex(source).unwrap(), path).unwrap();
    let checked = ncc::sema::check(ncc::generics::specialize(module).unwrap(), path).unwrap();
    ncc::optimizer::optimize(checked).unwrap()
}

fn constant_output(module: &Module, expected: &str) -> bool {
    module.items.iter().any(|item| {
        matches!(item, Item::Statement(s) if matches!(s.unlocated(), Stmt::Expr(e) if matches!(e.unlocated(), Expr::Call { callee, args, .. } if matches!(callee.unlocated(), Expr::Name(n) if n == "@println") && matches!(args.as_slice(), [Expr::String(text)] if text == expected))))
    })
}

fn compare(source: &str, stdout: &str, stderr: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("partial.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run").arg(&input);
        if release {
            command.arg("--release");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, stdout.as_bytes(), "release={release}");
        assert_eq!(output.stderr, stderr.as_bytes(), "release={release}");
    }
}

#[test]
fn known_calls_cross_output_and_runtime_input_without_erasing_storage() {
    let source = r#"
mut int value = 0
fn step() int { value = value + 1; @print("step", value, "|"); return value }
int first = step()
@println(first, value)
str[] runtime = @args()
mut int count = 0
while count < 4 { count = count + 1 }
@println(count)
@println(value)
"#;
    let module = optimized(source);
    assert!(constant_output(&module, "11"));
    assert!(constant_output(&module, "4"));
    compare(source, "step1|11\n4\n1\n", "");
}

#[test]
fn unknown_effects_keep_order_and_allow_independent_regions() {
    let source = r#"
mut int value = 1
@eprint("effect|")
mut int i = 0
while i < 3 { i = i + 1 }
@println(i)
value = 7
@println(value)
@eprintln("end")
"#;
    let module = optimized(source);
    assert!(constant_output(&module, "3"));
    assert!(constant_output(&module, "7"));
    compare(source, "3\n7\n", "effect|end\n");
}

#[test]
fn tuple_declarations_calls_and_value_copies_precompute_in_partial_programs() {
    let source = r#"
str[] runtime = @args()
mut int value = 0
fn pair() (int[], int[]) { value = value + 1; return ([value], [value + 1]) }
mut int[] left, int[] right = pair()
left[0] = 8
@println(left, right, value)
@eprint("kept")
"#;
    assert!(constant_output(&optimized(source), "[8][2]1"));
    compare(source, "[8][2]1\n", "kept");
}

#[test]
fn mutable_and_immutable_captures_survive_runtime_barriers() {
    let source = r#"
mut int value = 1
int copied = value
fn read = fn() int { return value + copied }
@println(read())
@eprint("barrier")
value = 4
@println(read())
"#;
    let module = optimized(source);
    assert!(constant_output(&module, "2"));
    assert!(constant_output(&module, "5"));
    compare(source, "2\n5\n", "barrier");
}

#[test]
fn failed_transactions_do_not_publish_output_or_partial_mutations() {
    let source = r#"
mut int value = 0
fn unknown() int { value = value + 1; @print(value, "|"); return @as(int, @args().len) }
@println(value, unknown(), value)
mut int i = 0
while i < 2 { i = i + 1 }
@println(i)
@println(value)
"#;
    assert!(constant_output(&optimized(source), "2"));
    compare(source, "1|011\n2\n1\n", "");
}

#[test]
fn output_arguments_snapshot_before_later_known_call_mutations() {
    let source = r#"
str[] runtime = @args()
mut int[] values = [0]
fn step() int { values[0] = values[0] + 1; @print("inner|"); return values[0] }
@println(values, step(), values, step(), values)
@eprint("kept")
"#;
    assert!(constant_output(&optimized(source), "[0]1[1]2[2]"));
    compare(source, "inner|inner|[0]1[1]2[2]\n", "kept");
}

#[test]
fn side_effectful_index_reads_remain_runtime_without_choosing_semantics() {
    let source = r"
str[] runtime = @args()
mut int[] values = [1]
fn change() int { values[0] = 2; return 0 }
@println(values[change()])
mut int i = 0
while i < 3 { i = i + 1 }
@println(i)
";
    let module = optimized(source);
    assert!(!constant_output(&module, "1"));
    assert!(constant_output(&module, "3"));
    compare(source, "2\n3\n", "");
}

#[test]
fn live_async_storage_never_becomes_constant_after_main_thread_assignment() {
    let source = r"
mut int value = 0
fn worker() { value = 10 }
fut void pending = async worker()
value = 1
@println(value)
@println(6 * 7)
await pending
";
    let module = optimized(source);
    assert!(!constant_output(&module, "1"));
    assert!(constant_output(&module, "42"));
    let c = ncc::compile_source_with_options(source, Path::new("partial.nc"), true).unwrap();
    assert!(c.contains("nc_fn_worker"));
}

#[test]
fn immutable_snapshots_remain_known_after_mutation_barriers() {
    let source = r#"
mut int[] values = [1]
int[] snapshot = values
fn change() { values[0] = 8; @eprint("changed") }
change()
@println(snapshot)
@println(values)
"#;
    assert!(constant_output(&optimized(source), "[1]"));
    assert!(!constant_output(&optimized(source), "[8]"));
    compare(source, "[1]\n[8]\n", "changed");
}

#[test]
fn discarded_multi_bindings_keep_initializer_effects_once() {
    let source = r#"
str[] runtime = @args()
mut int value = 0
fn pair() (int, int[]) { value = value + 1; @print("pair|"); return (value, [value + 1]) }
int _, int[] values = pair()
@println(values, value)
"#;
    assert!(constant_output(&optimized(source), "[2]1"));
    compare(source, "pair|[2]1\n", "");
}

#[test]
fn pure_index_calls_fold_but_local_mutating_index_calls_do_not() {
    let source = r"
str[] runtime = @args()
fn index() int { mut int i = 0; i = i + 1; return i }
@println([4, 5][index()])
fn example() int {
    mut int[] values = [1]
    fn change = fn() int { values[0] = 2; return 0 }
    return values[change()]
}
@println(example())
";
    let module = optimized(source);
    assert!(constant_output(&module, "5"));
    assert!(!constant_output(&module, "1"));
    compare(source, "5\n2\n", "");
    let restored = r"
str[] runtime = @args()
fn example() int {
    mut int[] values = [1]
    fn change = fn() int { values[0] = 2; values = [1]; return 0 }
    return values[change()]
}
@println(example())
";
    assert!(!constant_output(&optimized(restored), "1"));
    compare(restored, "2\n", "");
}

#[test]
fn returned_shared_closure_storage_stays_runtime_but_independent_work_folds() {
    let source = r#"
str[] runtime = @args()
fn factory() (fn() int) {
    mut int value = 0
    @print("created|")
    return fn() int { value = value + 1; return value }
}
(fn() int) next = factory()
@println(next(), next())
mut int i = 0
while i < 3 { i = i + 1 }
@println(i)
"#;
    assert!(constant_output(&optimized(source), "3"));
    compare(source, "created|12\n3\n", "");
}

#[test]
fn nominal_void_and_present_optional_void_materialize_in_partial_programs() {
    let source = r"
type Unit = void
type Other = Unit
str[] runtime = @args()
fn unit() {}
Other value = @as(Other, @as(Unit, unit()))
Other[] values = [value]
Other? optional = value
Other? absent = none
Other! success = value
void? present = unit()
void got = present else { @println(0) }
@println(values.len, optional == absent, success == success)
@println(42)
";
    assert!(constant_output(&optimized(source), "42"));
    compare(source, "1falsetrue\n42\n", "");
}

#[test]
fn known_assertions_preserve_test_execution_and_false_assertions_stop_analysis() {
    for assertion in ["true", "false", "@args().len == 0"] {
        let source = format!(
            "mut int value = 1;test \"check\" {{ assert {assertion};value = 3 }};@println(value)"
        );
        let module = optimized(&source);
        assert!(
            module
                .items
                .iter()
                .any(|item| matches!(item, Item::Test { .. }))
        );
        assert_eq!(constant_output(&module, "3"), assertion == "true");
    }
}

#[test]
fn retained_runtime_failure_does_not_publish_later_computation() {
    let source = r#"
str[] runtime = @args()
@println("before")
mut int[] values = [1]
@println(values[2])
@println(6 * 7)
"#;
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
    }
}

#[test]
fn native_calls_remain_runtime_and_independent_known_calls_fold() {
    let source = r#"
extern "native.c" as native { fn read() int = "read_value" }
mut int value = native.read()
fn sum(int n) int { mut int result = 0; for i in [1, 2, 3] { result = result + @as(int, i) + 1 };return result + n }
@println(value, sum(4))
@println(sum(4))
"#;
    assert!(constant_output(
        &optimized(&source.replace("native.read()", "read()")),
        "10"
    ));
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("partial.nc");
    fs::write(&input, source).unwrap();
    fs::write(
        directory.path().join("native.c"),
        "long long read_value(void) { return 7; }\n",
    )
    .unwrap();
    for mode in ["--debug", "--release"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args(["run", mode])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"710\n10\n");
    }
}
