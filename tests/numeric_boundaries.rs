use std::{fmt::Write as _, fs, path::Path, process::Command};

#[test]
fn nontrivial_integer_limits_match_with_constant_and_runtime_operands() {
    let cases = [
        ("byte", "254", "+", "1", "255"),
        ("byte", "255", "-", "254", "1"),
        ("byte", "17", "*", "15", "255"),
        ("byte", "255", "/", "2", "127"),
        ("byte", "255", "%", "128", "127"),
        ("byte", "15", "**", "2", "225"),
        ("byte", "3", "**", "5", "243"),
        (
            "int",
            "9223372036854775806",
            "+",
            "1",
            "9223372036854775807",
        ),
        (
            "int",
            "-9223372036854775807",
            "-",
            "1",
            "-9223372036854775808",
        ),
        (
            "int",
            "-9223372036854775808",
            "+",
            "9223372036854775807",
            "-1",
        ),
        (
            "int",
            "3037000499",
            "*",
            "3037000499",
            "9223372030926249001",
        ),
        (
            "int",
            "-4611686018427387904",
            "*",
            "2",
            "-9223372036854775808",
        ),
        (
            "int",
            "9223372036854775807",
            "/",
            "2",
            "4611686018427387903",
        ),
        ("int", "9223372036854775807", "%", "2", "1"),
        ("int", "-2", "**", "63", "-9223372036854775808"),
        (
            "uint",
            "18446744073709551614u",
            "+",
            "1u",
            "18446744073709551615u",
        ),
        (
            "uint",
            "18446744073709551615u",
            "-",
            "18446744073709551614u",
            "1u",
        ),
        (
            "uint",
            "4294967295u",
            "*",
            "4294967295u",
            "18446744065119617025u",
        ),
        (
            "uint",
            "18446744073709551615u",
            "/",
            "2u",
            "9223372036854775807u",
        ),
        (
            "uint",
            "18446744073709551615u",
            "%",
            "9223372036854775808u",
            "9223372036854775807u",
        ),
        ("uint", "3u", "**", "40u", "12157665459056928801u"),
    ];
    check_operations(&cases);
}

#[test]
fn valid_shifts_reach_high_bits_and_signed_lower_limit() {
    check_operations(&[
        ("byte", "1", "<<", "7", "128"),
        ("byte", "127", "<<", "1", "254"),
        ("byte", "255", ">>", "7", "1"),
        ("int", "-1", "<<", "63", "-9223372036854775808"),
        (
            "int",
            "-4611686018427387904",
            "<<",
            "1",
            "-9223372036854775808",
        ),
        (
            "int",
            "4611686018427387903",
            "<<",
            "1",
            "9223372036854775806",
        ),
        ("int", "9223372036854775807", ">>", "63", "0"),
        ("uint", "1u", "<<", "63u", "9223372036854775808u"),
        (
            "uint",
            "9223372036854775807u",
            "<<",
            "1u",
            "18446744073709551614u",
        ),
        ("uint", "18446744073709551615u", ">>", "63u", "1u"),
    ]);
}

type Operation<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

fn check_operations(cases: &[Operation<'_>]) {
    for index in ["0u", "@args().len - 1u"] {
        let mut source = format!("test \"numeric limits\" {{\nuint index = {index}\n");
        for (number, (kind, left, operator, right, expected)) in cases.iter().enumerate() {
            writeln!(
                source,
                "{kind}[] left{number} = [{left}]\n\
                 {kind}[] right{number} = [{right}]\n\
                 assert left{number}[index] {operator} right{number}[index] == {expected}"
            )
            .unwrap();
        }
        source.push_str("@println(\"checked\")\n}\n");
        success(&source);
    }
}

#[test]
fn float_extremes_underflow_and_overflow_remain_valid_values() {
    // This decimal rounds to the least positive binary64 subnormal (2^-1074).
    let smallest = format!("0.{}5", "0".repeat(323));
    for offset in ["0.0", "@as(float, @args().len) - 1.0"] {
        success(&format!(
            "test \"binary64 extremes\" {{\nfloat offset = {offset}\n\
             float tiny = {smallest} + offset\n\
             float two = 2.0 + offset\n\
             float large = two ** 1023.0\n\
             assert tiny > 0.0\n\
             assert tiny / two == 0.0\n\
             assert -tiny / two == -0.0\n\
             assert (tiny * two) / two == tiny\n\
             assert @as(int, tiny) == 0\n\
             assert @as(uint, tiny) == 0u\n\
             assert @as(int, -tiny) == 0\n\
             assert large < inf and large > 0.0\n\
             assert large * two == inf\n\
             assert -large * two == -inf\n\
             assert @as(str, large * two) == \"inf\"\n\
             assert @as(str, -large * two) == \"-inf\"\n\
             @println(\"checked\")\n}}\n"
        ));
    }
}

#[test]
fn adjacent_integer_overflows_fail_without_running_following_effects() {
    for (kind, left, operator, right) in [
        ("byte", "255", "+", "1"),
        ("byte", "0", "-", "1"),
        ("byte", "16", "*", "16"),
        ("byte", "16", "**", "2"),
        ("byte", "128", "<<", "1"),
        ("int", "9223372036854775807", "+", "1"),
        ("int", "-9223372036854775808", "+", "-1"),
        ("int", "-9223372036854775808", "-", "1"),
        ("int", "3037000500", "*", "3037000500"),
        ("int", "-4611686018427387905", "*", "2"),
        ("int", "2", "**", "63"),
        ("int", "4611686018427387904", "<<", "1"),
        ("uint", "18446744073709551615u", "+", "1u"),
        ("uint", "0u", "-", "1u"),
        ("uint", "4294967296u", "*", "4294967296u"),
        ("uint", "3u", "**", "41u"),
        ("uint", "9223372036854775808u", "<<", "1u"),
    ] {
        let declarations = format!(
            "{kind}[] left_values = [{left}]\n{kind}[] right_values = [{right}]\n\
             fn left() {kind} {{ @eprintln(\"left\");return left_values[@args().len - 1u] }}\n\
             fn right() {kind} {{ @eprintln(\"right\");return right_values[@args().len - 1u] }}\n"
        );
        runtime_failure(
            &format!("{declarations}_ = left() {operator} right()\n@eprintln(\"after\")"),
            "left\nright\npanic: integer overflow",
        );
        constant_failure(
            &format!("{kind} a = {left}\n{kind} b = {right}\n_ = a {operator} b"),
            "overflow",
        );
    }
}

#[test]
fn negating_signed_minimum_fails_but_its_neighbor_is_representable() {
    for index in ["0u", "@args().len - 1u"] {
        success(&format!(
            "test \"signed negation\" {{\nint[] values = [-9223372036854775807]\n\
             assert -values[{index}] == 9223372036854775807\n@println(\"checked\")\n}}"
        ));
    }
    runtime_failure(
        "int[] values = [-9223372036854775808]\n\
         fn operand() int { @eprintln(\"operand\");return values[@args().len - 1u] }\n\
         _ = -operand()\n@eprintln(\"after\")",
        "operand\npanic: integer overflow",
    );
    constant_failure("int value = -9223372036854775808\n_ = -value", "overflow");
}

#[test]
fn checked_conversion_neighbors_succeed_with_constant_and_runtime_values() {
    let cases = [
        ("int", "uint", "0", "0u"),
        ("int", "uint", "9223372036854775807", "9223372036854775807u"),
        ("uint", "int", "9223372036854775807u", "9223372036854775807"),
        (
            "float",
            "int",
            "-9223372036854775808.0",
            "-9223372036854775808",
        ),
        (
            "float",
            "int",
            "9223372036854774784.0",
            "9223372036854774784",
        ),
        (
            "float",
            "uint",
            "18446744073709549568.0",
            "18446744073709549568u",
        ),
        (
            "float",
            "uint",
            "9223372036854775808.0",
            "9223372036854775808u",
        ),
        ("float", "uint", "-0.0", "0u"),
        ("float", "uint", "0.75", "0u"),
        ("float", "int", "-0.75", "0"),
        ("float", "int", "-1.75", "-1"),
    ];
    for index in ["0u", "@args().len - 1u"] {
        let mut source = format!("test \"conversion neighbors\" {{\nuint index = {index}\n");
        for (number, (from, to, value, expected)) in cases.iter().enumerate() {
            writeln!(
                source,
                "{from}[] values{number} = [{value}]\n\
                 assert @as({to}, values{number}[index]) == {expected}"
            )
            .unwrap();
        }
        source.push_str("@println(\"checked\")\n}\n");
        success(&source);
    }
}

#[test]
fn checked_conversions_fail_for_adjacent_finite_and_nonfinite_values() {
    let tiny_negative = format!("-0.{}5", "0".repeat(323));
    let cases = [
        ("int", "uint", "-9223372036854775808"),
        ("uint", "int", "9223372036854775808u"),
        ("float", "int", "9223372036854775808.0"),
        ("float", "int", "-9223372036854777856.0"),
        ("float", "uint", "18446744073709551616.0"),
        ("float", "uint", tiny_negative.as_str()),
        ("float", "int", "NaN"),
        ("float", "uint", "NaN"),
        ("float", "int", "inf"),
        ("float", "uint", "inf"),
        ("float", "int", "-inf"),
        ("float", "uint", "-inf"),
    ];
    for (from, to, value) in cases {
        runtime_failure(
            &format!(
                "{from}[] values = [{value}]\n\
                 fn operand() {from} {{ @eprintln(\"operand\");return values[@args().len - 1u] }}\n\
                 _ = @as({to}, operand())\n@eprintln(\"after\")"
            ),
            "operand\npanic: cast out of range",
        );
        constant_failure(
            &format!("{from} value = {value}\n_ = @as({to}, value)"),
            "cast out of range",
        );
    }
}

fn success(source: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("numeric.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let output = execute(&input, "test", release);
        assert!(
            output.status.success(),
            "release={release}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"checked\n", "release={release}: {source}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

fn runtime_failure(source: &str, prefix: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("numeric.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        // A frontend rejection must not masquerade as a runtime check.
        ncc::compile_source_with_options(source, &input, release)
            .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
        let output = execute(&input, "run", release);
        assert_eq!(output.status.code(), Some(1), "{output:?}\n{source}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with(prefix),
            "release={release}: {stderr}\n{source}"
        );
        assert!(!stderr.contains("after"), "{stderr}");
    }
}

fn constant_failure(source: &str, detail: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("constant.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        // The failure phase is unresolved: accept a checked evaluation error or
        // a runtime panic, but never successful execution or an unrelated error.
        match ncc::compile_source_with_options(source, &input, release) {
            Ok(_) => {
                let output = execute(&input, "run", release);
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert_eq!(output.status.code(), Some(1), "{output:?}\n{source}");
                assert!(stderr.starts_with("panic:"), "{stderr}\n{source}");
                assert!(stderr.contains(detail), "{stderr}\n{source}");
                assert!(output.stdout.is_empty(), "{output:?}");
            }
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("constant evaluation failed"),
                    "{message}\n{source}"
                );
                assert!(message.contains(detail), "{message}\n{source}");
            }
        }
    }
}

fn execute(input: &Path, mode: &str, release: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command.arg(mode);
    if release {
        command.arg("--release");
    }
    command.arg(input).output().unwrap()
}
