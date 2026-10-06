use std::{fmt::Write as _, fs, path::Path, process::Command};

type Operation<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

#[test]
fn signed_quotients_and_remainders_truncate_toward_zero() {
    let mut cases = Vec::new();
    for (left, right, quotient, remainder) in [
        ("7", "3", "2", "1"),
        ("-7", "3", "-2", "-1"),
        ("7", "-3", "-2", "1"),
        ("-7", "-3", "2", "-1"),
        ("6", "3", "2", "0"),
        ("-6", "3", "-2", "0"),
        ("6", "-3", "-2", "0"),
        ("-6", "-3", "2", "0"),
        ("1", "3", "0", "1"),
        ("-1", "3", "0", "-1"),
        ("1", "-3", "0", "1"),
        ("-1", "-3", "0", "-1"),
        ("0", "1", "0", "0"),
        ("0", "-1", "0", "0"),
        ("-9223372036854775808", "1", "-9223372036854775808", "0"),
        ("-9223372036854775808", "2", "-4611686018427387904", "0"),
        ("-9223372036854775808", "3", "-3074457345618258602", "-2"),
        ("-9223372036854775808", "-3", "3074457345618258602", "-2"),
        ("-9223372036854775808", "-9223372036854775808", "1", "0"),
        ("-9223372036854775808", "9223372036854775807", "-1", "-1"),
        ("-9223372036854775807", "-1", "9223372036854775807", "0"),
        ("9223372036854775807", "1", "9223372036854775807", "0"),
        ("9223372036854775807", "-1", "-9223372036854775807", "0"),
        ("9223372036854775807", "2", "4611686018427387903", "1"),
        ("9223372036854775807", "-2", "-4611686018427387903", "1"),
        (
            "9223372036854775807",
            "-9223372036854775808",
            "0",
            "9223372036854775807",
        ),
    ] {
        cases.push(("int", left, "/", right, quotient));
        cases.push(("int", left, "%", right, remainder));
    }
    // INT_MIN % -1 is deliberately absent: its contract is unresolved.
    check_values(&cases);
}

#[test]
fn negative_signed_powers_follow_the_documented_base_exception() {
    let mut cases = Vec::new();
    for exponent in ["-1", "-2", "-3", "-9223372036854775808"] {
        for (base, expected) in [
            ("0", "0"),
            ("1", "1"),
            ("-1", "-1"),
            ("2", "0"),
            ("-2", "0"),
            ("9223372036854775807", "0"),
            ("-9223372036854775808", "0"),
        ] {
            // The documented -1 exception is not parity-dependent.
            cases.push(("int", base, "**", exponent, expected));
        }
    }
    check_values(&cases);
}

#[test]
fn zero_to_zero_is_one_for_every_numeric_type() {
    check_values(&[
        ("byte", "0", "**", "0", "1"),
        ("int", "0", "**", "0", "1"),
        ("uint", "0u", "**", "0u", "1u"),
        ("float", "0.0", "**", "0.0", "1.0"),
    ]);
}

#[test]
fn known_zero_to_zero_emits_a_located_compiler_warning() {
    let path = Path::new("numeric.nc");
    let mut failures = Vec::new();
    for (kind, zero) in [
        ("byte", "0"),
        ("int", "0"),
        ("uint", "0u"),
        ("float", "0.0"),
    ] {
        for bindings in [false, true] {
            let source = known_expression(kind, zero, "**", zero, bindings);
            for release in [false, true] {
                let compiled = ncc::compile_source_with_diagnostics(&source, path, release)
                    .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
                let found = compiled.warnings.0.iter().any(|diagnostic| {
                    let message = diagnostic.message.to_lowercase();
                    message.contains('0')
                        && (message.contains("power") || message.contains("exponent"))
                        && diagnostic.path.as_deref() == Some(path)
                        && diagnostic.span.start < diagnostic.span.end
                        && diagnostic.span.end <= source.len()
                });
                let rendered = compiled.warnings.render_warnings(&source, path);
                if !found || !rendered.contains("numeric.nc:") || !rendered.contains("warning:") {
                    failures.push(format!("release={release}: {rendered}\n{source}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "missing located warnings:\n{}",
        failures.join("\n")
    );
}

#[test]
fn successful_runtime_operands_execute_once_left_to_right() {
    for (kind, left, operator, right, expected) in [
        ("int", "-7", "/", "3", "-2"),
        ("int", "-7", "%", "-3", "-1"),
        ("int", "-1", "**", "-2", "-1"),
        ("int", "-1", "**", "-9223372036854775808", "-1"),
        ("int", "2", "**", "-9223372036854775808", "0"),
        ("byte", "0", "**", "0", "1"),
        ("int", "0", "**", "0", "1"),
        ("uint", "0u", "**", "0u", "1"),
        ("float", "0.0", "**", "0.0", "1.0"),
        ("byte", "1", "<<", "7", "128"),
        ("byte", "255", ">>", "7", "1"),
        ("int", "-3", "<<", "1", "-6"),
        ("int", "-3", ">>", "1", "-2"),
        ("uint", "1u", "<<", "63u", "9223372036854775808"),
        ("uint", "18446744073709551615u", ">>", "63u", "1"),
    ] {
        let source = format!(
            "{}@println(left() {operator} right())\n@eprintln(\"after\")\n",
            runtime_operands(kind, left, right)
        );
        with_input(&source, |input| {
            for release in [false, true] {
                let output = execute(input, "run", release);
                assert!(
                    output.status.success(),
                    "release={release}: {output:?}\n{source}"
                );
                assert_eq!(
                    output.stdout,
                    format!("{expected}\n").as_bytes(),
                    "release={release}: {source}"
                );
                assert_eq!(
                    output.stderr, b"left\nright\nafter\n",
                    "release={release}: {source}"
                );
            }
        });
    }
}

#[test]
fn known_integer_zero_divisors_require_compile_time_diagnostics() {
    check_static_failures(&zero_divisors(), "zero");
}

#[test]
fn known_invalid_shift_counts_require_compile_time_diagnostics() {
    check_static_failures(&invalid_shifts(), "shift");
}

#[test]
fn signed_minimum_divided_by_negative_one_panics_after_both_operands() {
    check_runtime_failures(
        &[("int", "-9223372036854775808", "/", "-1", "")],
        "overflow",
    );
}

#[test]
fn runtime_integer_zero_divisors_panic_after_both_operands() {
    check_runtime_failures(&zero_divisors(), "zero");
}

#[test]
fn runtime_invalid_shift_counts_panic_after_both_operands() {
    check_runtime_failures(&invalid_shifts(), "shift");
}

fn check_values(cases: &[Operation<'_>]) {
    for runtime in [false, true] {
        let index = if runtime { "@args().len - 1u" } else { "0u" };
        let mut source = format!("test \"numeric contracts\" {{\nuint index = {index}\n");
        for (number, (kind, left, operator, right, expected)) in cases.iter().enumerate() {
            writeln!(
                source,
                "{kind}[] left{number} = [{left}]\n{kind}[] right{number} = [{right}]\n\
                 assert left{number}[index] {operator} right{number}[index] == {expected}"
            )
            .unwrap();
            if !runtime {
                let literal_left = literal_operand(kind, left);
                let literal_right = literal_operand(kind, right);
                writeln!(
                    source,
                    "{kind} a{number} = {left}\n{kind} b{number} = {right}\n\
                     {kind} literal{number} = {literal_left} {operator} {literal_right}\n\
                     assert literal{number} == {expected}\n\
                     assert a{number} {operator} b{number} == {expected}"
                )
                .unwrap();
            }
        }
        source.push_str("@println(\"checked\")\n}\n");
        with_input(&source, |input| {
            for release in [false, true] {
                let output = execute(input, "test", release);
                assert!(
                    output.status.success(),
                    "release={release}: {output:?}\n{source}"
                );
                assert_eq!(output.stdout, b"checked\n", "release={release}: {source}");
            }
        });
    }
}

fn zero_divisors() -> Vec<Operation<'static>> {
    let mut cases = Vec::new();
    for (kind, one, zero) in [("byte", "1", "0"), ("int", "1", "0"), ("uint", "1u", "0u")] {
        for operator in ["/", "%"] {
            cases.push((kind, one, operator, zero, ""));
        }
    }
    cases
}

fn invalid_shifts() -> Vec<Operation<'static>> {
    let mut cases = Vec::new();
    for (kind, one, counts) in [
        ("byte", "1", &["8", "9", "255"][..]),
        (
            "int",
            "1",
            &[
                "-1",
                "-9223372036854775808",
                "64",
                "65",
                "9223372036854775807",
            ][..],
        ),
        ("uint", "1u", &["64u", "65u", "18446744073709551615u"][..]),
    ] {
        for operator in ["<<", ">>"] {
            for count in counts {
                cases.push((kind, one, operator, *count, ""));
            }
        }
    }
    cases
}

fn known_expression(kind: &str, left: &str, operator: &str, right: &str, bindings: bool) -> String {
    if bindings {
        format!(
            "{kind} left = {left}\n{kind} right = {right}\n{kind} result = left {operator} right\n"
        )
    } else {
        let left = literal_operand(kind, left);
        let right = literal_operand(kind, right);
        format!("{kind} result = {left} {operator} {right}\n")
    }
}

fn literal_operand(kind: &str, value: &str) -> String {
    if kind == "byte" {
        format!("@as(byte, {value})")
    } else {
        value.to_owned()
    }
}

fn check_static_failures(cases: &[Operation<'_>], detail: &str) {
    let path = Path::new("numeric.nc");
    let mut failures = Vec::new();
    for (kind, left, operator, right, _) in cases {
        for bindings in [false, true] {
            let source = known_expression(kind, left, operator, right, bindings);
            for release in [false, true] {
                match ncc::compile_source_with_options(&source, path, release) {
                    Ok(_) => failures.push(format!(
                        "release={release}: accepted invalid expression\n{source}"
                    )),
                    Err(error) => {
                        let message = error.to_string();
                        if !message.to_lowercase().contains(detail) {
                            failures.push(format!(
                                "release={release}: unrelated diagnostic: {message}\n{source}"
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn runtime_operands(kind: &str, left: &str, right: &str) -> String {
    format!(
        "{kind}[] left_values = [{left}]\n{kind}[] right_values = [{right}]\n\
         fn left() {kind} {{ @eprintln(\"left\");return left_values[@args().len - 1u] }}\n\
         fn right() {kind} {{ @eprintln(\"right\");return right_values[@args().len - 1u] }}\n"
    )
}

fn check_runtime_failures(cases: &[Operation<'_>], detail: &str) {
    for (kind, left, operator, right, _) in cases {
        let source = format!(
            "{}_ = left() {operator} right()\n@eprintln(\"after\")\n",
            runtime_operands(kind, left, right)
        );
        with_input(&source, |input| {
            for release in [false, true] {
                // Reject frontend failures: these inputs must reach runtime checks.
                ncc::compile_source_with_options(&source, input, release)
                    .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
                let output = execute(input, "run", release);
                assert_eq!(
                    output.status.code(),
                    Some(1),
                    "release={release}: {output:?}\n{source}"
                );
                assert!(output.stdout.is_empty(), "{output:?}");
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.starts_with("left\nright\npanic:"),
                    "release={release}: {stderr}\n{source}"
                );
                assert!(stderr.to_lowercase().contains(detail), "{stderr}");
                assert!(!stderr.contains("after"), "{stderr}");
            }
        });
    }
}

fn with_input(source: &str, action: impl FnOnce(&Path)) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("numeric.nc");
    fs::write(&input, source).unwrap();
    action(&input);
}

fn execute(input: &Path, mode: &str, release: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command.arg(mode);
    if release {
        command.arg("--release");
    }
    command.arg(input).output().unwrap()
}
