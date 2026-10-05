use std::path::Path;

fn rejects(source: &str, expected: &str) {
    for release in [false, true] {
        let error =
            ncc::compile_source_with_options(source, Path::new("negative_operands.nc"), release)
                .expect_err(&format!("release={release}: {source}"));
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\nexpected {expected:?}: {error}"
        );
    }
}

#[test]
fn mixed_numeric_bindings_require_explicit_conversion_for_binary_operators() {
    let types = [
        ("byte", "1"),
        ("int", "1"),
        ("uint", "1u"),
        ("float", "1.0"),
    ];
    for (left, left_value) in types {
        for (right, right_value) in types {
            if left == right {
                continue;
            }
            let operators = if left == "float" || right == "float" {
                "+ - * / % ** == != < <= > >="
            } else {
                "+ - * / % ** == != < <= > >= & | ^ << >>"
            };
            for operator in operators.split_whitespace() {
                rejects(
                    &format!(
                        "{left} a = {left_value}\n{right} b = {right_value}\n_ = a {operator} b"
                    ),
                    &format!("expected `{left}`, found `{right}`"),
                );
            }
        }
    }
}

#[test]
fn mixed_logical_bindings_do_not_coerce_to_boolean() {
    for (ty, value) in [
        ("int", "1"),
        ("uint", "1u"),
        ("byte", "1"),
        ("str", "\"x\""),
    ] {
        for operator in ["and", "or"] {
            rejects(
                &format!("bool a = true\n{ty} b = {value}\n_ = a {operator} b"),
                &format!("expected `bool`, found `{ty}`"),
            );
            rejects(
                &format!("{ty} a = {value}\nbool b = true\n_ = a {operator} b"),
                &format!("expected `{ty}`, found `bool`"),
            );
        }
    }
}

#[test]
fn container_concatenation_requires_compatible_elements_keys_and_values() {
    for (left, left_value, right, right_value) in [
        ("int[]", "[1]", "float[]", "[1.0]"),
        ("int[1]", "[1]", "bool[2]", "[true, false]"),
        ("int[]", "[]", "bool[0]", "[]"),
        ("[str]int", "[\"x\": 1]", "[str]float", "[\"x\": 1.0]"),
        ("[str]int", "[\"x\": 1]", "[char]int", "['x': 1]"),
        ("[str]int", "[:]", "[str]bool", "[:]"),
    ] {
        for (a, av, b, bv) in [
            (left, left_value, right, right_value),
            (right, right_value, left, left_value),
        ] {
            rejects(
                &format!("{a} a = {av}\n{b} b = {bv}\n_ = a <> b"),
                "expected",
            );
        }
    }
}

#[test]
fn equality_does_not_erase_tuple_structure_or_nominal_identity() {
    for operator in ["==", "!="] {
        for (left, right) in [
            ("(int, bool) a = (1, true)", "(int, int) b = (1, 1)"),
            (
                "(int, bool) a = (1, true)",
                "(int, bool, str) b = (1, true, \"x\")",
            ),
            ("First a = 1", "Second b = 1"),
            ("First[] a = [1]", "Second[] b = [1]"),
        ] {
            rejects(
                &format!(
                    "type First = int\ntype Second = int\n{left}\n{right}\n_ = a {operator} b"
                ),
                "expected",
            );
        }
    }
}

#[test]
fn explicit_numeric_conversion_and_compatible_concatenation_still_compile() {
    let source = r#"
int signed = 1
uint unsigned = 2u
float real = 3.0
_ = signed + @as(int, unsigned)
_ = real + @as(float, signed)
_ = unsigned << @as(uint, signed)
int[1] first = [1]
int[2] second = [2, 3]
int[] dynamic = [4]
int[3] fixed = first <> second
int[] joined = fixed <> dynamic
[str]int left = ["key": 1]
[str]int right = ["key": 2]
_ = left <> right
_ = 1 in joined
_ = 'x' in "text"
"#;
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("operands_control.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}"));
    }
}
