use std::{fmt::Write as _, fs, process::Command};

#[test]
fn nonnegative_integer_arithmetic_covers_each_operator_and_operand_width() {
    for kind in ["byte", "int", "uint"] {
        for (left, right, sum, difference, product, quotient, remainder, power) in [
            (5, 2, 7, 3, 10, 2, 1, 25),
            (3, 4, 7, -1, 12, 0, 3, 81),
            (7, 1, 8, 6, 7, 7, 0, 7),
            (0, 3, 3, -3, 0, 0, 0, 0),
            (9, 0, 9, 9, 0, 0, 0, 1),
        ] {
            let mut assertions = format!(
                "assert a + b == {sum}\nassert a * b == {product}\nassert a ** b == {power}\n"
            );
            if difference >= 0 || kind == "int" {
                writeln!(assertions, "assert a - b == {difference}").unwrap();
            }
            if right != 0 {
                writeln!(
                    assertions,
                    "assert a / b == {quotient}\nassert a % b == {remainder}"
                )
                .unwrap();
            }
            for offset in ["0", "@as(int, @args().len) - 1"] {
                success(&format!(
                    "test \"{kind} arithmetic\" {{\nint offset = {offset}\n\
                     {kind} a = @as({kind}, {left} + offset)\n\
                     {kind} b = @as({kind}, {right} + offset)\n{assertions}\
                     @println(\"checked\")\n}}\n"
                ));
            }
        }
    }
}

#[test]
fn finite_float_arithmetic_uses_exact_binary_fraction_oracles() {
    for (left, right, sum, difference, product, quotient) in [
        ("5.0", "2.0", "7.0", "3.0", "10.0", "2.5"),
        ("-5.0", "2.0", "-3.0", "-7.0", "-10.0", "-2.5"),
        ("1.5", "0.5", "2.0", "1.0", "0.75", "3.0"),
        ("0.0", "-2.0", "-2.0", "2.0", "0.0", "0.0"),
    ] {
        for offset in ["0.0", "@as(float, @args().len) - 1.0"] {
            success(&format!(
                "test \"finite float arithmetic\" {{\nfloat offset = {offset}\n\
                 float a = {left} + offset\nfloat b = {right} + offset\n\
                 assert a + b == {sum}\nassert a - b == {difference}\n\
                 assert a * b == {product}\nassert a / b == {quotient}\n\
                 assert 2.0 ** (3.0 + offset) == 8.0\n\
                 assert 1.5 ** (2.0 + offset) == 2.25\n\
                 @println(\"checked\")\n}}\n"
            ));
        }
    }
}

#[test]
fn boolean_conversions_cover_both_values_and_all_specified_destinations() {
    for source in ["false", "true", "@args().len == 0", "@args().len == 1"] {
        let (number, text) = if source == "true" || source == "@args().len == 1" {
            (1, "true")
        } else {
            (0, "false")
        };
        success(&format!(
            "test \"Boolean conversions\" {{\nbool value = {source}\n\
             assert @as(int, value) == {number}\n\
             assert @as(uint, value) == {number}u\n\
             assert @as(str, value) == \"{text}\"\n\
             @println(\"checked\")\n}}\n"
        ));
    }
}

#[test]
fn numeric_conversions_preserve_exact_shared_values_and_truncate_fractions() {
    for offset in ["0", "@as(int, @args().len) - 1"] {
        success(&format!(
            r#"test "numeric conversions" {{
    int offset = {offset}
    for index in [0, 1, 255, 65535, 4294967295, 9007199254740992] {{
        int value = [0, 1, 255, 65535, 4294967295, 9007199254740992][index] + offset
        uint unsigned = @as(uint, value)
        float signed_float = @as(float, value)
        float unsigned_float = @as(float, unsigned)
        assert @as(int, unsigned) == value
        assert signed_float == unsigned_float
        assert @as(int, signed_float) == value
        assert @as(uint, unsigned_float) == unsigned
        assert @as(str, value) == @as(str, unsigned)
    }}
    float delta = @as(float, offset)
    assert @as(int, 7.75 + delta) == 7
    assert @as(int, -7.75 + delta) == -7
    assert @as(int, -0.75 + delta) == 0
    assert @as(uint, 7.75 + delta) == 7u
    assert @as(uint, 0.75 + delta) == 0u
    @println("checked")
}}
"#
        ));
    }
}

#[test]
fn inclusion_covers_nominal_generic_and_structural_values_without_map_order() {
    for seed in ["1", "@as(int, @args().len)"] {
        success(&format!(
            r#"type Count = int
struct Box<type T> {{ T value }}
enum Choice<type T> {{ Empty Value(T) }}
fn contains<type T>(T value, T[] values) bool {{ return value in values }}
test "composite inclusion" {{
    int seed = {seed}
    Count count = @as(Count, seed)
    Count missing = @as(Count, seed + 1)
    Count[] counts = [count, count]
    Count[] empty = []
    assert contains<Count>(count, counts)
    assert not contains<Count>(missing, counts)
    assert not contains<Count>(count, empty)
    Count[2] fixed = [count, count]
    assert count in fixed and not (missing in fixed)
    Box<Count[]> boxed = Box<Count[]>{{.value = [count]}}
    Box<Count[]> other = Box<Count[]>{{.value = [missing]}}
    assert contains<Box<Count[]>>(boxed, [boxed])
    assert not contains<Box<Count[]>>(other, [boxed])
    Choice<Count[]> present = Choice.Value([count])
    Choice<Count[]> absent = Choice.Empty
    assert contains<Choice<Count[]>>(present, [absent, present])
    assert contains<Choice<Count[]>>(absent, [present, absent])
    assert not contains<Choice<Count[]>>(absent, [present])
    (Count[], str) pair = ([count], "x")
    (Count[], str) wrong = ([missing], "x")
    assert contains<(Count[], str)>(pair, [pair])
    assert not contains<(Count[], str)>(wrong, [pair])
    [str]Count entry = ["x": count]
    [str]Count same = ["x": count]
    [str]Count different = ["x": missing]
    assert contains<[str]Count>(same, [entry])
    assert not contains<[str]Count>(different, [entry])
    Count? optional = count
    Count? none_value = none
    assert contains<Count?>(optional, [none_value, optional])
    assert contains<Count?>(none_value, [none_value, optional])
    assert not contains<Count?>(none_value, [optional])
    [char]Count keyed = ['a': count, 'b': missing]
    char key = @as(char, @as(byte, seed + 96))
    assert key in keyed and 'b' in keyed and not ('c' in keyed)
    str text = "abc"
    assert "" in text and "" in ""
    assert 'a' in text and "bc" in text and not ("ca" in text)
    @println("checked")
}}
"#
        ));
    }
}

fn success(source: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("operators.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"checked\n", "release={release}");
    }
}
