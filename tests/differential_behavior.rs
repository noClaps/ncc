use std::{fs, process::Command, process::Output};

fn execute(source: &str, command: &str) -> [Output; 2] {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("behavior.nc");
    fs::write(&input, source).unwrap();
    ["--debug", "--release"].map(|mode| {
        Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args([command, mode])
            .arg(&input)
            .output()
            .unwrap()
    })
}

fn compare(source: &str, command: &str, code: i32, stdout: &[u8], stderr: &[u8]) {
    let [debug, release] = execute(source, command);
    assert_eq!(debug.status.code(), release.status.code(), "{source}");
    assert_eq!(debug.stdout, release.stdout, "{source}");
    assert_eq!(debug.stderr, release.stderr, "{source}");
    for output in [debug, release] {
        assert_eq!(output.status.code(), Some(code), "{output:?}\n{source}");
        assert_eq!(output.stdout, stdout, "{output:?}\n{source}");
        assert_eq!(output.stderr, stderr, "{output:?}\n{source}");
    }
}

fn success(body: &str, stdout: &[u8], stderr: &[u8]) {
    // Exercise whole-program folding, partial folding after unknown input, and
    // the real CLI test path rather than synthetic conformance dependency roots.
    for seed in ["0", "@as(int, @args().len) - 1"] {
        let source = format!("int seed = {seed}\n{body}");
        compare(&source, "run", 0, stdout, stderr);
        let source = format!("int seed = {seed}\ntest \"behavior\" {{\n{body}\n}}\n");
        compare(&source, "test", 0, stdout, stderr);
    }
}

#[test]
fn callable_selection_and_composite_argument_snapshots_precede_later_mutations() {
    success(
        r#"
mut int[] values = [seed + 1]
fn first(int[] left, int[] right) int {
    @eprintln("first:", left[0], ":", right[0])
    return left[0] + right[0]
}
fn second(int[] left, int[] right) int { return 100 + left[0] + right[0] }
mut (fn(int[], int[]) int) operation = first
fn choose() (fn(int[], int[]) int) { @eprintln("choose");return operation }
fn replace() int[] {
    @eprintln("replace")
    operation = second
    values[0] = 9
    return values
}
@println(choose()(values, replace()))
@println(operation(values, values))
@println(values)
"#,
        b"10\n118\n[9]\n",
        b"choose\nreplace\nfirst:1:9\n",
    );
}

#[test]
fn assignment_rhs_copies_survive_nested_target_replacement_and_index_effects() {
    success(
        r#"
mut int[][] values = [[seed + 1], [2]]
fn rhs() int[] { @eprintln("rhs");return values[0] }
fn outer() uint { @eprintln("outer");values = [[7], [8]];return 1u }
values[outer()] = rhs()
@println(values)
fn element() int { @eprintln("element");return values[1][0] + 4 }
fn inner() uint { @eprintln("inner");values[1] = [20];return 0u }
values[1][inner()] = element()
@println(values)
"#,
        b"[[7], [1]]\n[[7], [5]]\n",
        b"rhs\nouter\nelement\ninner\n",
    );
}

#[test]
fn short_circuit_fallbacks_and_output_arguments_keep_effect_order_and_nuls() {
    success(
        r#"
mut int count = seed
fn bump() bool { count = count + 1;@eprintln("bump:", count);return true }
fn absent() int? { @eprintln("absent");return none }
fn failed() int! { @eprintln("failed");throw "reason" }
fn advance() int { count = count + 10;return count }
_ = false and bump()
_ = true or bump()
_ = true and bump()
_ = false or bump()
int? some = 3
int present = some else advance()
int recovered = absent() else advance()
int caught = failed() catch message { @eprintln(message);break advance() }
@println(present, ":", recovered, ":", caught, ":", count)
@println(count, ":", advance(), ":", count, "\u{0}🍪")
@eprint("end\u{0}")
"#,
        b"3:12:22:22\n22:32:32\0\xf0\x9f\x8d\xaa\n",
        b"bump:1\nbump:2\nabsent\nfailed\nreason\nend\0",
    );
}

#[test]
fn immutable_tuple_and_by_value_closure_snapshots_do_not_follow_mutable_storage() {
    success(
        r#"
mut int[] values = [seed + 1]
(int[], int) snapshot = (values, seed + 2)
(fn() int) captured = fn() int { return snapshot[0][0] + snapshot[1] }
values[0] = 9
@println(captured(), ":", snapshot[0], ":", values)
fn factory(int initial) (fn() int) {
    mut int value = initial
    return fn() int { value = value + 1;return value }
}
(fn() int) left = factory(seed)
(fn() int) right = factory(seed + 10)
@println(left(), ":", right(), ":", left())
"#,
        b"3:[1]:[9]\n1:11:2\n",
        b"",
    );
}

#[test]
fn runtime_failure_precedence_preserves_output_and_skips_later_effects() {
    for (operation, trace, panic) in [
        ("values[index()] = rhs()", "rhs\n", "division by zero"),
        (
            "values[index()] = clear()",
            "clear\nindex\n",
            "array index out of bounds",
        ),
        (
            "_ = missing() catch message { @eprintln(\"caught\");break 0 }",
            "missing\n",
            "map key not found",
        ),
    ] {
        let body = format!(
            r#"
uint zero = @args().len - 1u
mut int[] values = [1]
fn rhs() int {{ @eprintln("rhs");return 1 / @as(int, zero) }}
fn index() uint {{ @eprintln("index");return 2u }}
fn clear() int {{ @eprintln("clear");values = [];return 7 }}
fn missing() int! {{
    @eprintln("missing")
    [str]int table = []
    return table[@args()[zero]]
}}
@print("before\u{{0}}")
@eprintln("before")
{operation}
@println("after")
@eprintln("after")
"#
        );
        let stderr = format!("before\n{trace}panic: {panic}\n");
        compare(&body, "run", 1, b"before\0", stderr.as_bytes());
        compare(
            &format!("test \"failure\" {{\n{body}\n}}"),
            "test",
            1,
            b"before\0",
            stderr.as_bytes(),
        );
    }
}

#[test]
fn failed_test_assertion_stops_shared_state_analysis_and_later_tests() {
    let source = r#"
mut int count = 0
fn advance() int { count = count + 1;return count }
test "first" { assert advance() == 1;@println(count) }
test "failure" {
    @println(advance())
    assert count == 1
    @println("after")
}
test "unreached" { @println(advance()) }
"#;
    compare(source, "test", 1, b"1\n2\n", b"panic: assertion failed\n");
}
