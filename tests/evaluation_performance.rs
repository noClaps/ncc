use std::{fmt::Write as _, fs, path::Path, process::Command};

#[test]
fn many_independent_float_expressions_and_immutable_captures() {
    let mut source = String::from(
        r#"
(float, float) pair = (0.25, 2.0)
fn scale = fn(float value) float { return (value + pair[0]) * pair[1] }
test "independent expressions" {
    // Keep top-level evaluation from consuming the entire test transaction.
    assert @args().len > 0
"#,
    );
    // Exercise both static arithmetic analysis and independent optimizer attempts.
    // No wall-clock threshold: this must also work on slow CI machines.
    for value in 0..432 {
        let expected = 2 * value;
        writeln!(
            source,
            "assert ({value}.0 + 0.25) * 2.0 == {expected}.5\nassert scale({value}.0) == {expected}.5"
        )
        .unwrap();
    }
    source.push_str("@println(\"expressions preserved\")\n}\n");
    run_both(&source, b"expressions preserved\n");
}

#[test]
fn independent_calls_keep_mutable_cells_and_closure_memoization_private() {
    let mut source = String::from(
        r#"
fn make(int initial) (fn() int) {
    mut int value = initial
    return fn() int { value = value + 1; return value }
}
fn independent(int initial) int {
    (fn() int) first = make(initial)
    (fn() int) second = make(initial + 10)
    return first() * 100 + first() * 10 + second()
}
test "private evaluator state" {
    assert @args().len > 0
"#,
    );
    for initial in 0..96 {
        let expected = (initial + 1) * 100 + (initial + 2) * 10 + initial + 11;
        writeln!(source, "assert independent({initial}) == {expected}").unwrap();
        writeln!(source, "assert independent({initial}) == {expected}").unwrap();
    }
    source.push_str("@println(\"cells preserved\")\n}\n");
    run_both(&source, b"cells preserved\n");
}

#[test]
fn failed_and_short_circuited_attempts_do_not_poison_later_constants() {
    run_both(
        r#"
fn divide(int divisor) int { return 12 / divisor }
fn early(bool stop) int {
    if stop { true -> { return 7 } false -> {} }
    return 9
}
test "attempt isolation" {
    assert @args().len > 0
    if @args().len > 10 {
        true -> { assert divide(0) == 0 }
        false -> { assert divide(3) == 4 }
    }
    assert true or (divide(0) == 0)
    assert early(true) == 7
    assert early(false) == 9
    assert divide(2) == 6
    assert divide(3) == 4
    @println("failures isolated")
}
"#,
        b"failures isolated\n",
    );
}

#[test]
fn indexed_immutable_initializers_preserve_failure_source_locations() {
    let path = Path::new("evaluation-locations.nc");
    for (declarations, failing) in [
        ("(int, int) pair = (3, 0)\n", "pair[0] / pair[1]"),
        (
            "int seed = 2\nint zero = seed - 2\nfn read = fn() int { return 12 / zero }\n",
            "12 / zero",
        ),
    ] {
        let mut source = String::new();
        for value in 0..128 {
            writeln!(source, "int value{value} = {value} + 1").unwrap();
        }
        source.push_str(declarations);
        if failing.starts_with("pair") {
            writeln!(source, "_ = {failing}").unwrap();
        } else {
            source.push_str("_ = read()\n");
        }
        let start = source.find(failing).unwrap();
        for release in [false, true] {
            let error = ncc::compile_source_with_options(&source, path, release).unwrap_err();
            assert_eq!(error.0.len(), 1, "release={release}: {error}");
            let diagnostic = &error.0[0];
            assert!(diagnostic.message.contains("division by zero"), "{error}");
            assert_eq!(diagnostic.path.as_deref(), Some(path), "release={release}");
            assert_eq!(
                diagnostic.span,
                start..start + failing.len(),
                "release={release}: {error}"
            );
        }
    }
}

fn run_both(source: &str, expected: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("evaluation.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
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
