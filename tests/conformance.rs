use std::fmt::Write as _;
use std::{fs, path::Path, process::Command};

#[test]
fn map_concatenation_overwrites_collisions_without_aliasing_operands() {
    success(
        r#"
test "map collisions" {
    mut [str]int[] left = ["shared": [1], "left": [2]]
    mut [str]int[] right = ["shared": [3, 4], "right": [5]]
    mut [str]int[] joined = left <> right
    assert joined.len == 3
    assert joined["shared"] == [3, 4]
    assert joined["left"] == [2]
    assert joined["right"] == [5]
    joined["shared"][0] = 9
    assert right["shared"] == [3, 4]
    left["left"][0] = 8
    right["right"][0] = 7
    assert joined["left"] == [2]
    assert joined["right"] == [5]
    [str]int[] empty = []
    assert left <> empty == left
    assert empty <> right == right
    [str]int[] last = ["shared": [6]]
    assert (left <> right <> last)["shared"] == [6]
}
"#,
        "",
    );
}

#[test]
fn nonfinite_float_literals_arithmetic_and_formatting() {
    success(
        r#"
fn runtime(float n) float { @print("") return n }
float zero = runtime(0.0)
float one = runtime(1.0)
float huge = runtime(10.0) ** 200.0
@println(NaN, ":", inf, ":", -inf)
@println(one / zero, ":", -one / zero, ":", zero / zero)
@println(huge * huge, ":", inf - inf, ":", inf * zero)
@println(one % zero, ":", (-one) ** 0.5, ":", inf + -inf)
@println(@as(str, -NaN), ":", "{inf}:{-inf}")
@println([NaN, inf, -inf])
test "IEEE comparisons" {
    float invalid = zero / zero
    assert invalid != invalid
    assert not (invalid == invalid)
    assert not (invalid < one)
    assert not (invalid <= one)
    assert not (invalid > one)
    assert not (invalid >= one)
    assert one / zero == inf
    assert -one / zero == -inf
    assert -inf < one
    assert inf > one
    assert not (invalid in [invalid])
    assert inf in [invalid, inf]
    assert [invalid] != [invalid]
}
"#,
        "NaN:inf:-inf\ninf:-inf:NaN\ninf:NaN:NaN\nNaN:NaN:NaN\nNaN:inf:-inf\n[NaN, inf, -inf]\n",
    );
}

#[test]
fn nonfinite_float_integer_casts_fail_at_runtime() {
    for value in ["NaN", "inf", "-inf"] {
        for ty in ["int", "uint"] {
            runtime_failure(
                &format!(
                    "fn runtime() float {{ @print(\"\") return {value} }} _ = @as({ty}, runtime())"
                ),
                "cast out of range",
            );
        }
    }
}

#[test]
fn numeric_byte_encodings_preserve_boundaries_and_ieee_bits() {
    let mut source = String::from(
        "fn signed(int value) int { @print(\"\") return value }\n\
         fn unsigned(uint value) uint { @print(\"\") return value }\n\
         fn floating(float value) float { @print(\"\") return value }\n",
    );
    source.push_str("test \"numeric bytes\" {\n");
    let mut cases = Vec::new();
    for value in [i64::MIN, -258, -1, 0, 1, 258, i64::MAX] {
        cases.push((format!("signed({value})"), value.to_le_bytes()));
    }
    for value in [0u64, 1, 258, 1 << 63, u64::MAX] {
        cases.push((format!("unsigned({value}u)"), value.to_le_bytes()));
    }
    for (literal, value) in [
        ("0.0", 0.0f64),
        ("-0.0", -0.0),
        ("1.0", 1.0),
        ("-1.0", -1.0),
        ("0.5", 0.5),
        ("-2.5", -2.5),
        ("inf", f64::INFINITY),
        ("-inf", f64::NEG_INFINITY),
    ] {
        cases.push((format!("floating({literal})"), value.to_le_bytes()));
    }
    for (index, (value, bytes)) in cases.into_iter().enumerate() {
        let expected = bytes.map(|byte| byte.to_string()).join(", ");
        write!(
            source,
            "byte[8] expected_{index} = [{expected}]\n\
             mut byte[] encoded_{index} = @as(byte[], {value})\n\
             assert encoded_{index} == expected_{index}\n\
             assert encoded_{index}.len == 8\n\
             encoded_{index}[0] = 99\n\
             assert @as(byte[], {value}) == expected_{index}\n"
        )
        .unwrap();
    }
    source.push_str("}\n@println(\"numeric bytes\")\n");
    success(&source, "numeric bytes\n");
}

#[test]
fn every_byte_converts_to_a_char_with_matching_utf8_bytes() {
    let characters = (0u8..=255)
        .map(|value| format!("'\\u{{{value:x}}}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let encodings = (0u8..=255)
        .map(|value| {
            let mut buffer = [0; 4];
            let encoded = char::from(value).encode_utf8(&mut buffer);
            let bytes = encoded
                .as_bytes()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{bytes}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    success(
        &format!(
            r#"
fn runtime(byte value) byte {{ @print("") return value }}
test "all byte characters" {{
    char[] characters = [{characters}]
    byte[][] encodings = [{encodings}]
    mut uint index = @args().len - 1
    while index < 256 {{
        byte value = runtime(@as(byte, index))
        char converted = @as(char, value)
        assert converted == characters[index]
        str text = @as(str, converted)
        assert text.len == 1
        assert text[0] == converted
        mut byte[] encoded = @as(byte[], converted)
        byte[] copied = @as(byte[], text)
        assert encoded == encodings[index]
        assert copied == encoded
        encoded[0] = 255
        assert copied == encodings[index]
        index = index + 1
    }}
}}
@println("byte characters")
"#,
        ),
        "byte characters\n",
    );
}

#[test]
fn finite_float_integer_casts_cover_exact_representable_boundaries() {
    success(
        r#"
fn runtime(float value) float { @print("") return value }
test "finite casts" {
assert @as(int, runtime(-9223372036854775808.0)) == -9223372036854775808
assert @as(int, runtime(-9223372036854774784.0)) == -9223372036854774784
assert @as(int, runtime(9223372036854774784.0)) == 9223372036854774784
assert @as(uint, runtime(18446744073709549568.0)) == 18446744073709549568u
assert @as(uint, runtime(9223372036854775808.0)) == 9223372036854775808u
assert @as(int, runtime(-0.0)) == 0
assert @as(uint, runtime(-0.0)) == 0u
assert @as(int, runtime(-1.75)) == -1
assert @as(int, runtime(1.75)) == 1
assert @as(uint, runtime(1.75)) == 1u
assert @as(int, runtime(-0.75)) == 0
}
@println("finite casts")
"#,
        "finite casts\n",
    );
}

#[test]
fn signed_unsigned_runtime_casts_preserve_the_shared_integer_range() {
    let mut source = String::from(
        "fn runtime_signed(int value) int { @print(\"\") return value }\n\
         fn runtime_unsigned(uint value) uint { @print(\"\") return value }\n\
         test \"exact integer casts\" {\n",
    );
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
        writeln!(
            source,
            "assert @as(uint, runtime_signed({value})) == {value}u\n\
             assert @as(int, runtime_unsigned({value}u)) == {value}\n\
             assert @as(int, @as(uint, runtime_signed({value}))) == {value}",
        )
        .unwrap();
    }
    source.push_str("}\n@println(\"exact integer casts\")\n");
    success(&source, "exact integer casts\n");
}

#[test]
fn negative_fractional_float_to_uint_panics_before_truncation() {
    let tiny = format!("-0.{}5", "0".repeat(323));
    for value in ["-0.75", "-0.5", "-0.0001", tiny.as_str()] {
        let source = format!(
            r#"
fn argument() float {{ _ = @args() @println("argument") return {value} }}
@println(@as(uint, argument()))
@println("after")
"#,
        );
        for release in [false, true] {
            compile_fixture(&source, Path::new("negative-cast.nc"), release).unwrap();
            let output = run_mode(&source, release);
            assert_eq!(output.status.code(), Some(1), "release={release}: {value}");
            assert_eq!(output.stdout, b"argument\n", "release={release}: {value}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("cast out of range"),
                "release={release}: {value}: {stderr}",
            );
        }
    }
}

#[test]
fn finite_out_of_range_numeric_casts_panic_before_c_conversion() {
    for (from, to, value) in [
        ("float", "int", "9223372036854775808.0"),
        ("float", "int", "-9223372036854777856.0"),
        ("float", "uint", "18446744073709551616.0"),
        ("float", "uint", "-1.0"),
        ("int", "uint", "-1"),
        ("uint", "int", "9223372036854775808u"),
        ("uint", "int", "18446744073709551615u"),
    ] {
        runtime_failure(
            &format!(
                "fn runtime({from} value) {from} {{ @print(\"\") return value }} \
                 _ = @as({to}, runtime({value}))",
            ),
            "cast out of range",
        );
    }
}

#[test]
fn inclusion_checks_array_values_and_map_keys() {
    success(
        r#"
struct Entry { int id str name }
test "inclusion" {
    int[] numbers = [2, 4, 2]
    int[] empty = []
    int[3] fixed = [2, 4, 6]
    assert 2 in numbers
    assert 4 in numbers
    assert not (3 in numbers)
    assert not (2 in empty)
    assert 6 in fixed
    assert not (5 in fixed)

    int[][] nested = [[1, 2], [], [3]]
    int[] pair = [1, 2]
    int[] reversed = [2, 1]
    assert pair in nested
    assert empty in nested
    assert not (reversed in nested)

    Entry[] entries = [Entry{.id = 1, .name = "one"}]
    assert Entry{.id = 1, .name = "one"} in entries
    assert not (Entry{.id = 1, .name = "other"} in entries)

    [int]str names = [2: "two", 4: "four"]
    [int]str no_names = []
    assert 2 in names
    assert not (3 in names)
    assert not (2 in no_names)
    [str]int counts = ["key": 42]
    assert "key" in counts
    assert not ("42" in counts)
}
"#,
        "",
    );
}

#[test]
fn string_inclusion_handles_unicode_and_embedded_nuls() {
    success(
        r#"
test "string inclusion" {
    str text = "start 🍪 café\u{0}end"
    assert '🍪' in text
    assert 'é' in text
    assert '\u{0}' in text
    assert "🍪 café" in text
    assert "fé\u{0}en" in text
    assert "end" in text
    assert not ('x' in text)
    assert not ("cafe" in text)
    assert not ("start end" in text)
    assert not ("start 🍪 café\u{0}end!" in text)
    assert not ('a' in "")
    assert not ("a" in "")
}
"#,
        "",
    );
}

#[test]
fn inclusion_evaluates_each_operand_once_in_source_order() {
    success(
        r#"
fn needle() int { @print("needle ") return 2 }
fn numbers() int[] { @print("array ") return [1, 2, 3] }
fn names() [int]str { @print("map ") return [2: "two"] }
fn letter() char { @print("char ") return '🍪' }
fn word() str { @print("word ") return "café" }
fn text() str { @print("text ") return "🍪 café" }
@println(needle() in numbers())
@println(needle() in names())
@println(letter() in text())
@println(word() in text())
"#,
        "needle array true\nneedle map true\nchar text true\nword text true\n",
    );
}

#[test]
fn inclusion_rejects_mismatched_elements_and_noncontainers() {
    for source in [
        "_ = 1 in 1",
        "_ = 1 in (1, 2)",
        "_ = 1 in \"123\"",
        "_ = true in \"true\"",
        "_ = \"a\" in 'a'",
    ] {
        rejects(source, "in requires a compatible container and element");
    }
    for source in [
        "float[] values = [1.0] _ = 1 in values",
        "int[] values = [1] _ = 1.0 in values",
        "int[1] values = [1] _ = true in values",
        "[str]int values = [\"a\": 1] _ = 1 in values",
        "[int]str values = [1: \"a\"] _ = \"a\" in values",
    ] {
        rejects(source, "expected `");
    }
}

#[test]
fn maps_require_bare_if_comparisons() {
    for source in [
        "[str]int values = [] if values { _ -> {} }",
        "[str]int values = [\"a\": 1] if values { values -> {} _ -> {} }",
        "fn values() [str]int { return [] } if values() { _ -> {} }",
        "fn choose<T>(T value) { if value { _ -> {} } } choose<[str]int>([])",
    ] {
        rejects(source, "maps cannot be matched directly");
    }
    success(
        r#"
[str]int values = ["a": 1]
if {
    values == ["a": 2] -> { @println("wrong") }
    values == ["a": 1] -> { @println("equal") }
    _ -> { @println("wrong") }
}
if {
    "a" in values -> { @println("present") }
    _ -> { @println("wrong") }
}
"#,
        "equal\npresent\n",
    );
}

#[test]
fn external_c_composite_signatures_have_stable_aliases() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("native.c"),
        r#"
nc_abi_nc_sum_result nc_sum(nc_abi_nc_sum_arg0 bytes) {
    nc_abi_nc_sum_result result = {0};
    if (!bytes.len) { result.failed = 1; result.error = NC_STRING("empty"); return result; }
    for (uint64_t i = 0; i < bytes.len; ++i) result.value += bytes.vals[i];
    bytes.vals[0] = 99;
    return result;
}
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
extern "native.c" as native { fn sum(byte[] bytes) int! = "nc_sum" }
byte[] bytes = [1, 2, 3]
@println(try native.sum(bytes))
@println(bytes[0])
int fallback = native.sum([]) catch err { 42 }
@println(fallback)
"#,
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"6\n1\n42\n");
    }
    let rejects_external = |source: &str, expected: &str| {
        for release in [false, true] {
            let error = compile_fixture(source, &input, release)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "release={release}: {error}");
        }
    };
    fs::write(directory.path().join("native.etch"), "").unwrap();
    rejects_external(
        "extern \"native.c\" as native { fn sum(Missing value) int = \"sum\" }",
        "unknown type",
    );
    rejects_external(
        "extern \"native.etch\" as native { fn sum() int = \"sum\" }",
        "must be C source",
    );
    rejects_external(
        "extern \"native.c\" as native { fn sum() int = \"not-valid\" }",
        "C identifier",
    );
    rejects(
        "struct S { int n } bool b = S{.n = 1} < S{.n = 2}",
        "ordered comparisons require numeric",
    );
}

#[test]
fn external_c_extensions_are_case_sensitive() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    for (filename, accepted) in [
        ("native.c", true),
        ("native.C.c", true),
        (".native.c", true),
        ("uppercase.C", false),
        ("native.cpp", false),
        ("native.c.bak", false),
        ("nativec", false),
    ] {
        fs::write(directory.path().join(filename), "").unwrap();
        let source = format!("extern \"{filename}\" as native {{ fn value() int = \"value\" }}");
        for release in [false, true] {
            let result = compile_fixture(&source, &input, release);
            if accepted {
                assert!(result.is_ok(), "{filename}, release={release}: {result:?}");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains("must be C source"),
                    "{filename}, release={release}: {error}"
                );
            }
        }
    }
}

#[test]
fn duplicate_generic_and_record_declarations_are_not_silently_overwritten() {
    for source in [
        "fn f<T, T>(T value) T { return value }",
        "struct Box<T, T> { T value }",
        "enum Either<T, T> { Value(T) }",
        "struct Box<T> { T value T value }",
        "struct Box { int value int value }",
        "enum Either<T> { Value(T) Value }",
        "enum Either { Value(int) Value }",
        "fn f<T>(T value) T { return value } fn f<T>(T value) T { return value }",
        "fn f<T>(T value) T { return value } fn f() {}",
        "struct Box<T> { T value } struct Box<T> { T other }",
        "enum Box<T> { Value(T) } struct Box<T> { T value }",
        "type Box = int struct Box<T> { T value }",
        "struct int<T> { T value }",
        "enum bool<T> { Value(T) }",
        "fn str<T>(T value) T { return value }",
    ] {
        rejects(source, "duplicate");
    }
}

#[test]
fn expanding_generic_recursion_reports_a_limit_instead_of_crashing() {
    for source in [
        "fn grow<T>(T value) int { return grow<T[]>([value]) } _ = grow<int>(1)",
        "struct Grow<T> { Grow<T[]>[] next } fn use(Grow<int> value) {}",
        "enum Grow<T> { Next(Grow<T[]>) } fn use(Grow<int> value) {}",
    ] {
        for release in [false, true] {
            let output = run_mode(source, release);
            assert!(!output.status.success());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("specialization limit exceeded"),
                "{source}: {error}"
            );
        }
    }
}

#[test]
fn generic_constructors_receive_nested_type_context() {
    success(
        r#"
enum Choice<type T> { Value(T) Empty }
struct Box<type T> { T value }
struct Holder { Choice<int> choice }
fn read(Choice<int> choice) int {
    if choice { Choice.Value(n) -> { return n } Choice.Empty -> { return 0 } }
}
fn identity<type T>(T value) T { return value }
fn make() Choice<int> { return Choice.Value(7) }
fn wrap<T>(T value) Choice<T> { return Choice.Value(value) }
fn unwrap<T>(Choice<T>? maybe, T fallback) T {
    Choice<T> value = maybe else { Choice.Value(fallback) }
    return if value { Choice.Value(v) -> { v } Choice.Empty -> { fallback } }
}
fn wait<T>(fut T work) T { return await work }
test "nested contexts" {
    Choice<int>? optional = Choice.Value(1)
    Choice<int> choice = optional else { Choice.Empty }
    assert read(choice) == 1
    assert read(Choice.Value(2)) == 2
    assert read(identity<Choice<int>>(Choice.Value(3))) == 3
    [str]Choice<int> map = ["one": Choice.Value(4)]
    assert read(map["one"]) == 4
    Holder holder = Holder{.choice = Choice.Value(5)}
    assert read(holder.choice) == 5
    Box<Choice<int>> box = Box<Choice<int>>{.value = Choice.Value(6)}
    assert read(box.value) == 6
    Choice<int> conditional = if true { true -> { Choice.Value(8) } false -> { Choice.Empty } }
    assert read(conditional) == 8
    if make() { Choice.Value(n) -> { assert n == 7 } Choice.Empty -> { assert false } }
    Choice<Choice<int>> nested = wrap<Choice<int>>(Choice.Value(9))
    assert read(unwrap<Choice<int>>(nested, Choice.Empty)) == 9
    assert read(unwrap<Choice<int>>(none, Choice.Value(10))) == 10
    fut Choice<int> work = async make()
    fut Choice<int> forwarded = async wait<Choice<int>>(work)
    assert read(await forwarded) == 7
}
"#,
        "",
    );
}

#[test]
fn alternative_pattern_bindings_are_consistent() {
    success(
        r#"
enum Either { Left(int) Right(int) }
fn value(Either e) int {
    if e { Either.Left(n), Either.Right(n) -> { return n } }
}
test "alternatives" { assert value(Either.Left(3)) == 3 assert value(Either.Right(4)) == 4 }
"#,
        "",
    );
    rejects(
        "enum E { A(int) B(int) } fn f(E e) int { if e { E.A(a), E.B(b) -> { return a } } }",
        "alternative patterns must bind",
    );
    rejects(
        "enum E { A(int) B(str) } fn f(E e) int { if e { E.A(a), E.B(a) -> { return 0 } } }",
        "alternative patterns must bind",
    );
    rejects(
        "enum E { A(int) B } fn f(E e) int { if e { E.A(a), E.B -> { return a } } }",
        "alternative patterns must bind",
    );
}

#[test]
fn futures_cannot_escape_through_nominal_types_or_captures() {
    rejects(
        "type Hidden = fut int fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "struct Hidden { fut int value } fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "enum Hidden { Value(fut int) } fn escape() Hidden { throw \"no\" }",
        "futures cannot be returned",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() fn later = fn() int { return await value }",
        "cannot capture futures",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() fut int[] values = [value]",
        "future must be initialized",
    );
    rejects(
        "fn one() int { return 1 } fut int value = async one() struct Hidden { fut int value } Hidden hidden = Hidden{.value = value}",
        "not stored in composite",
    );
}

#[test]
fn functions_and_futures_reject_equality_and_string_conversion_recursively() {
    let function = "fn value() int { return 1 }\n";
    for (declaration, expression) in [
        ("", "value"),
        ("(fn() int)[] values = [value]", "values"),
        ("[str](fn() int) values = [\"f\": value]", "values"),
        ("((fn() int), int) values = (value, 1)", "values"),
        (
            "struct Holder { (fn() int) f } Holder holder = Holder{.f = value}",
            "holder",
        ),
        (
            "enum Callback { Value((fn() int)) Empty } Callback c = Callback.Empty",
            "c",
        ),
        (
            "type Callback = (fn() int) Callback c = @as(Callback, value)",
            "c",
        ),
        ("(fn() int)? c = none", "c"),
        ("fut int pending = async value()", "pending"),
        (
            "enum Recursive { Children(Recursive[]) Callback((fn() int)) Empty } Recursive r = Recursive.Empty",
            "r",
        ),
    ] {
        for operation in [
            format!("_ = {expression} == {expression}"),
            format!("_ = {expression} != {expression}"),
            format!("@println({expression})"),
            format!("_ = @as(str, {expression})"),
            format!("_ = \"{{{expression}}}\""),
        ] {
            let source = format!("{function}{declaration}\n{operation}");
            rejects(&source, "not defined for functions or unawaited futures");
            for release in [false, true] {
                let error = compile_fixture(&source, Path::new("test.nc"), release).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("not defined for functions or unawaited futures"),
                    "{source}: {error}"
                );
            }
        }
    }
    for operation in [
        "_ = value in [value]",
        "_ = [value: 1]",
        "[(fn() int)]int table = []",
    ] {
        rejects(&format!("{function}{operation}"), "equality is not defined");
    }
    rejects("fn empty() {} @println(empty())", "not void");
    rejects("fn empty() {} _ = empty() == empty()", "not void");
}

#[test]
fn partial_tuple_destructuring_evaluates_once_and_copies() {
    success(
        r#"
mut int calls = 0
fn values() (int, int[], int) { calls = calls + 1 return (1, [2], 3) }
int a, (int[], int) b = values()
test "partial tuple" {
    assert calls == 1
    assert a == 1
    mut int[] data = [4]
    (int, int[], int) source = (3, data, 5)
    mut int first, (int[], int) rest = source
    data[0] = 9
    assert rest[0][0] == 4
    assert rest[1] == 5
    assert first == 3
    rest[0][0] = 8
    assert source[1][0] == 4
    int plain, (int, int) nested = (1, (2, 3))
    assert nested == (2, 3)
}
"#,
        "",
    );
    rejects(
        "int a, (int, int) b = (1, 2, 3, 4)",
        "expects 2 grouped or 3 flat elements, found 4",
    );
}

#[test]
fn top_level_tuple_bindings_are_visible_to_functions() {
    success(
        r#"
int one, str two = (1, "two")
mut int three, int four = (3, 4)
fn sum() int { return one + three + four }
three = three + 1
test "globals" { assert sum() == 9 assert two == "two" }
"#,
        "",
    );
}

#[test]
fn recursive_struct_layouts_and_value_operations() {
    success(
        r#"
struct Node { int value Node[] children }
struct Parent { Child[] children }
struct Child { Parent parent }
test "recursive values" {
    Node leaf = Node{.value = 2, .children = []}
    Node root = Node{.value = 1, .children = [leaf]}
    mut Node copy = root
    copy.children[0].value = 3
    assert root.children[0].value == 2
    assert copy != root
    assert root == Node{.value = 1, .children = [leaf]}
    str text = @as(str, root)
    assert "children" in text
    Parent empty = Parent{.children = []}
    Parent parent = Parent{.children = [Child{.parent = empty}]}
    assert parent.children[0].parent == empty
}
"#,
        "",
    );
    rejects("struct Loop { Loop value }", "infinite size");
    rejects(
        "struct A { B value } struct B { A? value }",
        "infinite size",
    );
    rejects("type Cycle = Cycle[]", "cyclic nominal");
    rejects("type A = [str]B type B = A?", "cyclic nominal");
}

#[test]
fn for_traversal_retains_original_indices_when_bindings_change_size() {
    success(
        r#"
fn runtime(int value) int { @print("") return value }
test "original indices" {
    mut int[] values = [runtime(1), 2, 3, 4, 5]
    for i in values {
        assert i < 5
        values = values <> [values[i] * 10]
    }
    assert values == [1, 2, 3, 4, 5, 10, 20, 30, 40, 50]
    mut int[] replaced = [10, 20]
    mut uint[] visited = []
    for i in replaced {
        assert i < 2
        visited = visited <> [i]
        if i { 0 -> { replaced = [100, 200, 300] } _ -> {} }
        @println(replaced[i])
    }
    assert visited == [0u, 1u]
    assert replaced == [100, 200, 300]
    mut int[] shortened = [10, 20, 30]
    visited = []
    for i in shortened {
        shortened = []
        visited = visited <> [i]
    }
    assert visited == [0u, 1u, 2u]
    assert shortened.len == 0

    mut str text = "x"
    visited = []
    for i in text {
        assert i == 0
        text = text <> "y"
        visited = visited <> [i]
    }
    assert text == "xy"
    assert visited == [0u]
    text = "a🍪界"
    visited = []
    for i in text {
        text = "x"
        visited = visited <> [i]
    }
    assert visited == [0u, 1u, 2u]
    assert text == "x"
}
"#,
        "100\n200\n",
    );
}

#[test]
fn nested_traversals_keep_independent_snapshots_after_ancestor_replacement() {
    success(
        r#"
fn runtime(int value) int { _ = @args() @print("") return value }
test "independent nested traversal snapshots" {
    mut int[][] rows = [[runtime(1), 2], [3]]
    mut uint[] visited = []
    mut int total = 0
    for i in rows {
        assert i < 2
        for j in rows[i] {
            assert j < 3
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
    assert visited == [0u, 1u, 10u, 11u, 12u]
    assert total == 90
    assert rows.len == 3
    assert rows[0] == [10]
    assert rows[1] == [20, 30, 40]
    assert rows[2] == [50]
    @println(visited, " ", total)
}
"#,
        "[0, 1, 10, 11, 12] 90\n",
    );
}

#[test]
fn for_traversal_retains_original_map_keys_under_insertion_and_replacement() {
    success(
        r#"
fn runtime(int value) int { @print("") return value }
test "original keys" {
    mut [str]int values = ["a": runtime(1), "b": 2]
    mut uint visits = 0
    mut int total = 0
    for key in values {
        assert key == "a" or key == "b"
        values["new"] = 99
        visits = visits + 1
        total = total + values[key]
    }
    assert visits == 2
    assert total == 3
    assert values.len == 3
    mut [str]int replaced = ["a": 1, "b": 2]
    mut [str]bool seen = []
    for key in replaced {
        replaced = ["new": 9]
        seen[key] = true
    }
    assert seen.len == 2
    assert "a" in seen and "b" in seen
    assert replaced == ["new": 9]
}
"#,
        "",
    );
}

#[test]
fn original_traversal_does_not_hide_invalid_current_binding_lookups() {
    runtime_failure(
        r#"
fn runtime() int { @print("") return 1 }
mut int[] values = [runtime(), 2]
for i in values {
    if i { 0 -> { values = [9] } _ -> {} }
    @println(values[i])
}
"#,
        "out of bounds",
    );
    runtime_failure(
        r#"
fn runtime() int { @print("") return 1 }
mut [str]int values = ["original": runtime()]
for key in values {
    values = ["new": 9]
    @println(values[key])
}
"#,
        "map key not found",
    );
    runtime_failure(
        r#"
fn runtime() str { @print("") return "a🍪" }
mut str text = runtime()
for i in text {
    if i { 0 -> { text = "x" } _ -> {} }
    @println(text[i])
}
"#,
        "out of bounds",
    );
}

#[test]
fn for_loops_use_indices_and_map_keys_and_skip_empty_containers() {
    success(
        r#"
fn array_source() int[] { @print("array ") return [10, 20, 30] }
fn string_source() str { @print("string ") return "a🍪界" }
fn map_source() [int]str { @print("map ") return [7: "seven", 11: "eleven"] }
for index in array_source() { @print(index, " ") }
@println("")
for index in string_source() { @print(index, " ") }
@println("")
mut int sum = 0
for key in map_source() { sum = sum + key }
@println(sum)
test "indices and keys" {
    str index = "outer"
    int[3] values = [10, 20, 30]
    mut uint count = 0
    for index in values {
        uint typed = index
        assert typed == count
        assert values[index] == (@as(int, index) + 1) * 10
        count = count + 1
    }
    assert index == "outer"
    assert count == 3
    str text = "a\u{0}👩‍👩‍👧‍👦"
    char[3] expected = ['a', '\u{0}', '👩‍👩‍👧‍👦']
    count = 0
    for index in text {
        uint typed = index
        assert text[typed] == expected[index]
        count = count + 1
    }
    assert count == 3
    [str]int mapping = ["one": 1, "two": 2, "three": 3]
    mut int total = 0
    for key in mapping {
        str typed = key
        assert typed in mapping
        total = total + mapping[key]
    }
    assert total == 6
    int[] empty = []
    int[0] fixed_empty = []
    [str]int empty_map = []
    for index in empty { assert false }
    for index in fixed_empty { assert false }
    for index in "" { assert false }
    for key in empty_map { assert false }
    while false { assert false }
}
"#,
        "array 0 1 2 \nstring 0 1 2 \nmap 18\n",
    );
}

#[test]
fn nested_loop_jumps_and_while_conditions_preserve_effect_order() {
    success(
        r#"
int[][] table = [[1, 2, 4, 3, 99], [5, 99], [6, 7, 10, 99], [100]]
rows: for row in table {
    @print("r", row, " ")
    for col in table[row] {
        int value = table[row][col]
        if value {
            2 -> { continue }
            3 -> { break }
            5 -> { continue :rows }
            10 -> { break :rows }
            _ -> {}
        }
        @print(value, " ")
    }
    @print("done ")
}
@println("end")
mut int checks = 0
mut int bodies = 0
fn condition() bool { checks = checks + 1 @print("c", checks, " ") return checks < 4 }
while condition() {
    bodies = bodies + 1
    if bodies { 2 -> { continue } _ -> {} }
    @print("b", bodies, " ")
}
@println(checks, ":", bodies)
fn find(int[][] values) int {
    for row in values {
        for col in values[row] {
            if values[row][col] > 0 { true -> { return values[row][col] } false -> {} }
        }
    }
    return -1
}
@println(find([[], [-2, 0], [0, 7, 8]]), ":", find([[], [-2, 0]]))
"#,
        "r0 1 4 done r1 r2 6 7 end\nc1 b1 c2 c3 b3 c4 4:3\n7:-1\n",
    );
}

#[test]
fn loop_bindings_types_scopes_and_jump_targets_are_checked() {
    for source in [
        "for index in [1] { index = 1 }",
        "for index in \"a\" { index = 1 }",
        "[str]int values = [\"a\": 1] for key in values { key = \"b\" }",
    ] {
        rejects(source, "cannot mutate immutable");
    }
    for source in [
        "for index in [1] { int signed = index }",
        "for index in \"a\" { int signed = index }",
        "[str]int values = [\"a\": 1] for key in values { uint index = key }",
    ] {
        rejects(source, "expected `");
    }
    for source in [
        "for index in [1] {} _ = index",
        "[str]int values = [] for key in values {} _ = key",
        "while false { int local = 1 } _ = local",
    ] {
        rejects(source, "unknown name");
    }
    for source in [
        "for index in 1 {}",
        "for index in true {}",
        "for index in (1, 2) {}",
    ] {
        rejects(source, "for loop expects an array, map, or str");
    }
    for source in ["while 1 {}", "while \"true\" {}"] {
        rejects(source, "expected `bool`");
    }
    for source in [
        "while false { break :missing }",
        "for index in [1] { continue :missing }",
        "done: while false {} break :done",
        "outer: while false { fn nested() { break :outer } }",
        "outer: for index in [1] { fn nested = fn() { continue :outer } }",
    ] {
        rejects(source, "no valid target");
    }
}

#[test]
fn labelled_conditionals_and_value_breaks() {
    success(
        r#"
test "labels" {
    mut int index = 5
    char letter = if index {
        1 -> { break 'A' }
        _ -> {
            while index < 26 {
                lbl: if index {
                    5 -> { break 'E' }
                    6 -> { break :lbl }
                    7 -> { break }
                    _ -> {}
                }
                index = index + 1
            }
            break 'Z'
        }
    }
    assert letter == 'E'
    index = 6
    while index < 10 {
        lbl: if index { 6 -> { break :lbl } _ -> { break } }
        index = index + 1
    }
    assert index == 7
    mutex int value = 0
    mut int i = 0
    while i < 2 {
        lock value { value = value + 1 i = i + 1 continue }
    }
    lock value { assert value == 2 }
}
"#,
        "",
    );
    rejects(
        "lbl: if true { true -> { continue :lbl } false -> {} }",
        "no valid target",
    );
}

#[test]
fn invalid_labels_value_breaks_and_pattern_comparisons_are_rejected() {
    for statement in [
        "return",
        "break",
        "continue",
        "throw \"bad\"",
        "assert true",
        "int x = 1",
        "@println(1)",
    ] {
        rejects(
            &format!("fn f() ! {{ while true {{ wrong: {statement}\n }} }}"),
            "labels may only be applied",
        );
    }
    for source in [
        "while true { break 1 }",
        "for i in [1] { break missing }",
        "label: if true { true -> { break 1 } false -> {} }",
        "mutex int x = 0 lock x { break 1 }",
        "int x = if true { true -> { fn nested() { break 1 } 1 } false -> { 2 } }",
    ] {
        rejects(source, "break with a value requires");
    }
    for source in [
        "fn f() {} (fn() void) value = f if value { value -> {} _ -> {} }",
        "fn f() {} (fn() void) value = f if [value] { [value] -> {} _ -> {} }",
        "fn f() {} fut void value = async f() if value { value -> {} _ -> {} }",
        "fn f() {} struct Box { (fn() void) value } Box box = Box{.value = f} if box { box -> {} _ -> {} }",
    ] {
        rejects(source, "pattern equality is not defined");
    }
    rejects(
        "struct S { int x } S s = S{.x = 1} if s { S{.x = a, .x = b} -> {} _ -> {} }",
        "duplicate field",
    );
}

// Preserve decomposed literals: NC stores UTF-8 bytes without normalization.
#[test]
fn string_char_elements_remain_separate_after_replacement_and_concatenation() {
    success(
        include_str!(
            "fixtures/unicode/string_char_elements_remain_separate_after_replacement_and_concatenation.nc"
        ),
        "",
    );
}

#[test]
fn string_iteration_preserves_slots_while_neighboring_characters_change() {
    success(
        r#"
fn runtime(str text) str { @print("") return text }
test "iteration elements" {
    mut str text = runtime("\rX")
    for i in text {
        if i { 0 -> { text[1] = '\n' } _ -> {} }
        @print(i, ":", text.len, " ")
        if i { 0 -> { assert text[i] == '\r' } _ -> { assert text[i] == '\n' } }
    }
    @println("")
    mut str accent = runtime("aX")
    for i in accent {
        if i { 0 -> { accent[1] = '\u{301}' } _ -> {} }
        @print(i, ":", accent.len, " ")
    }
    @println("")
    assert accent[$] == '\u{301}'
}
"#,
        "0:2 1:2 \n0:2 1:2 \n",
    );
}

// These raw decomposed graphemes intentionally exercise lexer and string handling.
#[test]
fn unicode_string_length_indexing_and_iteration() {
    success(
        include_str!("fixtures/unicode/unicode_string_length_indexing_and_iteration.nc"),
        "cookie 🍪\n",
    );
    runtime_failure("str empty = \"\" @println(empty[0])", "out of bounds");
}

#[test]
fn mutexes_share_between_tasks_and_unlock_on_exit() {
    success(
        r#"
test "mutexes" {
    mutex int[] numbers = [1,2,3]
    fn add_1() bool {
        lock numbers {
            for i in numbers { numbers[i] = numbers[i] + 1 }
            return true
        }
    }
    fn add_2() bool {
        lock numbers { for i in numbers { numbers[i] = numbers[i] + 2 } }
        return true
    }
    fut bool first = async add_1()
    fut bool second = async add_2()
    assert await first
    assert await second
    lock numbers { assert numbers == [4,5,6] }
    escape: lock numbers {
        for i in numbers { numbers[i] = 10 break :escape }
    }
    lock numbers { assert numbers[0] == 10 }
    fn fail() int! { lock numbers { throw "failed" } }
    int fallback = fail() catch err { 7 }
    assert fallback == 7
    lock numbers { numbers[0] = 11 }
    lock numbers { assert numbers[0] == 11 }
}
"#,
        "",
    );
    rejects("int value = 1 lock value {}", "lock requires a mutex");
    rejects("mutex int value = 1 value = 2", "immutable");
}

#[test]
fn background_futures_and_await() {
    success(
        r#"
fn square(int n) int { return n*n }
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
test "futures" {
    fut int a = async square(7)
    fut int b = async square(8)
    assert (await a) + (await b) == 113
    assert await a == 49
    int captured = 5
    fn closure = fn(int n) int { return captured+n }
    fut int c = async closure(4)
    assert await c == 9
    fut int! error = async checked(0)
    int value = await error catch err { 42 }
    assert value == 42
}
"#,
        "",
    );
    rejects(
        "mut fut int f = async missing()",
        "futures cannot be mutable",
    );
    rejects("fn bad() fut int { }", "futures cannot be returned");
    rejects("fut int f = async 1", "async requires a function call");
}

#[test]
fn return_paths_do_not_count_statements_after_jumps() {
    for source in [
        "fn bad(bool b) int { label: if b { true -> { break :label return 1 } false -> { return 2 } } }",
        "fn bad(bool b) int { label: if b { true -> { { break :label } return 1 } false -> { return 2 } } }",
        "fn bad(bool b) int { outer: if b { true -> { inner: if b { true -> { break :outer return 1 } false -> { return 2 } } return 3 } false -> { return 4 } } }",
        "fn bad(bool b) int { outer: if b { true -> { int n = if b { true -> { break :outer return 1 } false -> { 2 } } return n } false -> { return 3 } } }",
        "fn bad(bool b) int { label: if b { true -> { int? maybe = none int n = maybe else { break :label return 1 } return n } false -> { return 2 } } }",
        "fn bad = fn(bool b) int { label: if b { true -> { break :label return 1 } false -> { return 2 } } }",
    ] {
        rejects(source, "may finish without returning");
    }
    success(
        r#"
fn escaped(bool b) int {
    label: if b { true -> { break :label return 1 } false -> { return 2 } }
    return 3
}
fn expression_returns(bool b) int {
    int unused = if b { true -> { return 4 } false -> { return 5 } }
}
fn loop_exits() int {
    mutex int value = 0
    outer: while true {
        lock value { value = 6 break :outer }
    }
    lock value { return value }
}
test "return paths" {
    assert escaped(true) == 3 and escaped(false) == 2
    assert expression_returns(true) == 4 and expression_returns(false) == 5
    assert loop_exits() == 6
}
"#,
        "",
    );
}

#[test]
fn string_conversion_requires_convertible_constituents_and_explicit_custom_unwrapping() {
    for (declaration, expression, diagnostic) in [
        (
            "type Number = int Number n = 1",
            "n",
            "underlying base types",
        ),
        (
            "type Text = str Text n = \"text\"",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int Number[] n = []",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int fn table() [str]Number { return [] }",
            "table()",
            "underlying base types",
        ),
        (
            "type Text = str fn table() [Text]int { return [] }",
            "table()",
            "underlying base types",
        ),
        (
            "type Number = int Number? n = none",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int (Number, int) n = (1, 2)",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int struct Box { Number value } Box n = Box{.value = 1}",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int enum Box { Empty Value(Number) } Box n = Box.Empty",
            "n",
            "underlying base types",
        ),
        (
            "type Number = int fn result() Number! { throw \"bad\" }",
            "result()",
            "underlying base types",
        ),
        (
            "enum Recursive { Children(Recursive[]) Value(void) Empty } Recursive n = Recursive.Empty",
            "n",
            "not defined for void",
        ),
        ("void[] n = []", "n", "not defined for void"),
        ("void? n = none", "n", "not defined for void"),
        ("fn result() ! {}", "result()", "not defined for void"),
        (
            "fn result() ! { throw \"bad\" }",
            "result()",
            "not defined for void",
        ),
    ] {
        for operation in [
            format!("@println({expression})"),
            format!("@eprintln({expression})"),
            format!("_ = \"{{{expression}}}\""),
            format!("_ = @as(str, {expression})"),
        ] {
            // A custom string can be explicitly unwrapped directly to str.
            if declaration.starts_with("type Text = str Text") && operation.starts_with("_ = @as") {
                continue;
            }
            rejects(&format!("{declaration}\n{operation}"), diagnostic);
        }
    }
    success(
        r#"
type Text = str
type Outer = Text
type Number = int
type Result = int!
fn result() int! { throw "wrong!" }
Outer text = @as(Outer, @as(Text, "hello"))
Number number = 42
Result wrapped = @as(Result, result())
@println(@as(str, @as(Text, text)))
@println(@as(str, @as(int, number)))
@println(@as(str, @as(int!, wrapped)))
"#,
        "hello\n42\nerror: wrong!\n",
    );
}

#[test]
fn error_union_output_supports_both_streams_and_embedded_nuls() {
    let source = r#"
fn result(bool fail) str! {
    if fail { true -> { throw "bad\u{0}🍪" } false -> { return "ok" } }
}
@print(result(false), ":")
@println(result(true))
@eprint(result(false), ":")
@eprintln(result(true))
"#;
    for release in [false, true] {
        let output = run_mode(source, release);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, "ok:error: bad\0🍪\n".as_bytes());
        assert_eq!(output.stderr, output.stdout);
    }
}

#[test]
fn repeated_concurrent_awaits_copy_results_and_discarded_workers_finish() {
    success(
        r#"
fn produce() int[] { return [1, 2] }
fn consume(fut int[] input) int {
    mut int[] copy = await input
    copy[0] = 7
    return copy[0]
}
fn child() { @println("child") }
fn parent() { fut void job = async child() }
test "concurrent await" {
    mut int i = 0
    while i < 16 {
        fut int[] original = async produce()
        fut int a = async consume(original)
        fut int b = async consume(original)
        assert await a == 7 and await b == 7
        assert await original == [1, 2]
        assert await original == [1, 2]
        i = i + 1
    }
    fut void discarded = async parent()
}
"#,
        "child\n",
    );
}

#[test]
fn anonymous_functions_share_mutable_captures() {
    success(
        r#"
fn map<type T, type U>(T[] arr, (fn(T) U) apply) U[] {
    mut U[] result = []
    for i in arr { result = result <> [apply(arr[i])] }
    return result
}
fn make(int n) (fn(int) int) { return fn(int x) int { return n+x } }
test "closures" {
    mut int original = 3
    mut int[] numbers = [2]
    fn add = fn(int x) int { return original + numbers[0] + x }
    original = 30
    numbers[0] = 20
    assert add(1) == 51
    assert make(5)(2) == 7
    str[] strings = map<int,str>([1,2,3], fn(int n) str { return "{n}" })
    assert strings == ["1","2","3"]
    fn outer = fn(int n) (fn(int) int) { return fn(int x) int { return original+n+x } }
    (fn(int) int) inner = outer(4)
    original = 300
    assert inner(5) == 309
}
"#,
        "",
    );
    rejects("int a = 1 fn f = fn() { a = 2 }", "immutable");
}

#[test]
fn indirect_calls_evaluate_callable_then_arguments_once_in_order() {
    success(
        r#"
fn mark(int value) int { @print(value, " ") return value }
fn add(int left, int right) int { @print("call ") return left + right }
fn choose() (fn(int, int) int) { @print("choose ") return add }
fn index() uint { @print("index ") return 0 }
struct Holder { (fn(int, int) int) operation }
fn holder() Holder { @print("holder ") return Holder{.operation = add} }
(fn(int, int) int)[] operations = [add]
@println(choose()(mark(1), mark(2)))
@println(operations[index()](mark(3), mark(4)))
@println(holder().operation(mark(5), mark(6)))
fn apply<T, U>(T left, T right, (fn(T, T) U) operation) U {
    return operation(left, right)
}
@println(apply<int, int>(mark(7), mark(8), choose()))
"#,
        "choose 1 2 call 3\nindex 3 4 call 7\nholder 5 6 call 11\n7 8 choose call 15\n",
    );
}

#[test]
fn mutable_parameter_shadows_keep_caller_composites_independent() {
    success(
        r#"
struct Payload { int[] numbers [str]int[] table str text }
fn changed(Payload input) Payload {
    mut Payload input = input
    input.numbers[0] = 3
    input.table["key"][0] = 4
    input.text[1] = '\u{301}'
    return input
}
fn changed_tuple((int[], str) input) (int[], str) {
    mut (int[], str) input = input
    input[0][0] = 5
    input[1][0] = 'b'
    return input
}
fn runtime(int value) int { @print("") return value }
test "parameter copies" {
    Payload original = Payload{.numbers = [runtime(1)], .table = ["key": [2]], .text = "aX"}
    mut Payload result = changed(original)
    assert original.numbers == [1]
    assert original.table["key"] == [2]
    assert original.text == "aX"
    assert result.numbers == [3]
    assert result.table["key"] == [4]
    assert result.text.len == 2
    assert result.text[0] == 'a'
    assert result.text[1] == '\u{301}'
    result.numbers[0] = 9
    assert changed(original).numbers == [3]
    (int[], str) tuple = ([1], "aX")
    (int[], str) copy = changed_tuple(tuple)
    assert tuple[0] == [1] and tuple[1] == "aX"
    assert copy[0] == [5] and copy[1] == "bX"
}
"#,
        "",
    );
}

#[test]
fn function_parameters_and_callback_signatures_are_strict() {
    for source in [
        "fn bad(int value) { value = 2 }",
        "fn bad(int[] values) { values[0] = 2 }",
        "fn bad([str]int values) { values[\"key\"] = 2 }",
        "fn bad(str value) { value[0] = 'a' }",
        "fn bad((int, int) value) { value[0] = 2 }",
        "struct Item { int value } fn bad(Item item) { item.value = 2 }",
        "fn bad(int[] values) { fn mutate() { values[0] = 2 } }",
    ] {
        rejects(source, "cannot mutate immutable");
    }
    for source in [
        "fn bad(uint value) int { return @as(int, value) } (fn(int) int) callback = bad",
        "fn bad(int value) uint { return @as(uint, value) } (fn(int) int) callback = bad",
        "fn bad(int left, int right) int { return left + right } (fn(int) int) callback = bad",
        "fn bad(int value) {} (fn(int) int) callback = bad",
        "fn apply((fn(int) int) callback) int { return callback(1) } fn bad(str value) int { return 1 } _ = apply(bad)",
        "(fn(int) int) callback = fn(int value) int { return value } _ = callback(true)",
    ] {
        rejects(source, "expected `");
    }
    for source in [
        "fn value() int { return 1 } value()",
        "fn value() int { return 1 } (fn() int) callback = value callback()",
        "fn value() int { return 1 } (fn() int)[] callbacks = [value] callbacks[0]()",
    ] {
        rejects(source, "not used");
    }
    rejects("fn value(int input) {} _ = input", "unknown name");
}

#[test]
fn function_values_and_callbacks() {
    success(
        r#"
fn add(int a, int b) int { return a+b }
fn apply(int a, int b, (fn(int,int) int) op) int { return op(a,b) }
struct Calculator { (fn(int,int) int) operation }
test "callbacks" {
    (fn(int,int) int) operation = add
    assert apply(2,3,operation) == 5
    Calculator calculator = Calculator{.operation = add}
    assert calculator.operation(3,4) == 7
    (fn(int,int) int)[] operations = [add]
    assert operations[0](4,5) == 9
}
"#,
        "",
    );
}

#[test]
fn nested_assignment_places_evaluate_indices_and_rhs_once_in_order() {
    success(
        r#"
struct Bucket { int[] values str text }
fn nested_places() (int, int, str, int, int) {
    mut int trace = 0
    fn index(int marker) int { @print("") trace = trace * 10 + marker return 0 }
    fn key() str { @print("") trace = trace * 10 + 1 return "item" }
    fn replacement() int { @print("") trace = trace * 10 + 3 return 9 }
    mut [str]Bucket buckets = ["item": Bucket{.values = [1], .text = "X"}]
    [str]Bucket original = buckets
    buckets[key()].values[index(2)] = replacement()
    buckets[key()].text[index(4)] = 'Z'
    mut (Bucket, int) pair = (Bucket{.values = [1], .text = "Y"}, 2)
    pair[0].values[index(5)] = 8
    return trace, buckets["item"].values[0], buckets["item"].text, pair[0].values[0], original["item"].values[0]
}
@println(nested_places())
"#,
        "(312145, 9, Z, 8, 1)\n",
    );
}

#[test]
fn composite_assignment_rhs_is_copied_before_index_replaces_tuple_ancestor() {
    success(
        r#"
struct Payload { [str]int[] rows }
fn runtime(int value) int { _ = @args() @print("") return value }
test "RHS copy before tuple ancestor replacement" {
    mut (Payload[], int) state = (
        [
            Payload{.rows = ["k": [runtime(1), 2], "other": [4]]},
            Payload{.rows = ["k": [3]]}
        ],
        10
    )
    mut int trace = 0
    mut int rhs_calls = 0
    mut int index_calls = 0
    mut int[] changed_source = []
    fn source() Payload {
        @print("rhs:")
        rhs_calls = rhs_calls + 1
        trace = trace * 10 + 1
        return state[0][0]
    }
    fn target() int {
        @print("index:")
        index_calls = index_calls + 1
        trace = trace * 10 + 2
        state[0][0].rows["k"][0] = 99
        state[0][0].rows["other"][0] = 98
        changed_source = state[0][0].rows["k"]
        state = (
            [
                Payload{.rows = ["k": [7]]},
                Payload{.rows = ["k": [8]]},
                Payload{.rows = ["k": [9]]}
            ],
            20
        )
        return 1
    }
    state[0][target()] = source()
    assert rhs_calls == 1 and index_calls == 1
    assert trace == 12
    assert changed_source == [99, 2]
    assert state[0].len == 3
    assert state[0][0].rows.len == 1
    assert state[0][0].rows["k"] == [7]
    assert state[0][1].rows.len == 2
    assert state[0][1].rows["k"] == [1, 2]
    assert state[0][1].rows["other"] == [4]
    assert state[0][2].rows.len == 1
    assert state[0][2].rows["k"] == [9]
    assert state[1] == 20
    @println(state[0][1].rows["k"], " ", trace)
}
"#,
        "rhs:index:[1, 2] 12\n",
    );
}

#[test]
fn nested_assignment_failures_are_preserved_in_release() {
    for (source, message) in [
        (
            "fn invalid() int { mut int[][] rows = [[1]] rows[1][0] = 7 return 9 } @println(invalid())",
            "out of bounds",
        ),
        (
            "struct Bucket { int[] values } fn invalid() int { mut [str]Bucket buckets = [\"present\": Bucket{.values = [1]}] buckets[\"missing\"].values[0] = 7 return 9 } @println(invalid())",
            "map key not found",
        ),
        (
            "fn invalid() str { mut str[] texts = [\"x\"] texts[0][1] = 'y' return texts[0] } @println(invalid())",
            "out of bounds",
        ),
    ] {
        runtime_failure(source, message);
    }
}

#[test]
fn assignment_rhs_failures_precede_invalid_targets() {
    for source in [
        "mut int[] values = [1]\nvalues[1] = 1 / 0",
        "mut int[][] rows = [[1]]\nrows[0][1] = 1 / 0",
        "struct Entry { int value } mut [str]Entry entries = [\"present\": Entry{.value = 1}] entries[\"missing\"].value = 1 / 0",
        "fn runtime() int[] { @print(\"\") return [1] } mut int[] values = runtime() values[1] = 1 / 0",
        "mut int[] values = [1] fn shrink() { @print(\"\") values = [] } shrink() values[0] = 1 / 0",
        "fn invalid() int { mut int[] values = [1] values[-1] = 1 / 0 return 9 } @println(invalid())",
        "fn invalid() int { mut int[][] rows = [[1]] rows[1][0] = 1 / 0 return 9 } @println(invalid())",
        "fn invalid() int { mut int[][] rows = [[1]] rows[0][1] = 1 / 0 return 9 } @println(invalid())",
        "struct Entry { int value } fn invalid() int { mut [str]Entry entries = [\"present\": Entry{.value = 1}] entries[\"missing\"].value = 1 / 0 return 9 } @println(invalid())",
        "fn invalid() uint { mut uint[][] rows = [[1]] rows[0][1] = @as(uint, -0.75) return 9 } @println(invalid())",
    ] {
        let source = source
            .replace("1 / 0", "fail()")
            .replace("@as(uint, -0.75)", "@as(uint, fail())");
        let source = format!("fn fail() int {{ @print(\"\") return 1 / 0 }}\n{source}");
        runtime_failure(&source, "division by zero");
    }
}

#[test]
fn assignment_targets_use_bindings_after_rhs_and_index_effects() {
    success(
        r#"
struct Bucket { int[] values str text }
fn assignments() (int[], int[][], str, int, int) {
    mut int[] values = [1]
    fn replace() int { @print("") values = [2, 3] return 7 }
    values[1] = replace()
    mut int[][] rows = [[1]]
    fn resize() int { @print("") rows = [[2, 3]] return 1 }
    rows[0][resize()] = 8
    mut str text = "x"
    fn character() char { @print("") text = "ab" return 'Z' }
    text[$] = character()
    mut [str]Bucket buckets = ["item": Bucket{.values = [1], .text = "x"}]
    fn insert() int { @print("") buckets = ["item": Bucket{.values = [2], .text = "y"}, "new": Bucket{.values = [3], .text = "z"}] return 0 }
    buckets["item"].values[insert()] = 9
    mut int trace = 0
    fn index() int { @print("") trace = trace * 10 + 1 return 0 }
    fn replacement() int { @print("") trace = trace * 10 + 2 return 4 }
    values[index()] = replacement()
    return values, rows, text, buckets["item"].values[0], trace
}
@println(assignments())
"#,
        "([4, 7], [[2, 8]], aZ, 9, 21)\n",
    );
    success(
        r#"
struct Bucket { int[] values }
mut int[][] rows = []
fn repair() int { @print("repair:") rows = [[1]] return 0 }
rows[0][repair()] = 7
@println(rows)
rows = []
rows[0][[repair()][$]] = 8
@println(rows)
mut [str]Bucket buckets = []
fn restore() int { @print("restore:") buckets = ["item": Bucket{.values = [1]}] return 0 }
buckets["item"].values[restore()] = 9
@println(buckets["item"].values)
"#,
        "repair:[[7]]\nrepair:[[8]]\nrestore:[9]\n",
    );
    runtime_failure(
        "mut int[][] rows = [] fn fail() int { @print(\"\") return 1 / 0 } rows[0][fail()] = 7",
        "division by zero",
    );
    for source in [
        "mut int[] values = [1, 2] fn replace() int { @print(\"\") values = [] return 7 } values[1] = replace()",
        "mut int[][] rows = [[1]] fn index() int { @print(\"\") rows = [] return 0 } rows[0][index()] = 7",
        "mut int[][] rows = [[1]] fn index() int { @print(\"\") rows = [[]] return 0 } rows[0][index()] = 7",
        "struct Bucket { int[] values } mut [str]Bucket buckets = [\"item\": Bucket{.values = [1]}] fn index() int { @print(\"\") buckets = [] return 0 } buckets[\"item\"].values[index()] = 7",
        "mut str text = \"ab\" fn replace() char { @print(\"\") text = \"\" return 'Z' } text[1] = replace()",
    ] {
        runtime_failure(
            source,
            if source.contains("buckets") {
                "map key not found"
            } else {
                "out of bounds"
            },
        );
    }
}

#[test]
fn writable_places_and_evaluation_order() {
    success(
        r#"
struct Inner { int x }
struct Outer { Inner inner [str]int counts }
mut int counter = 1
fn update() int { counter = 9 return 2 }
fn pair(int a, int b) int { return a * 10 + b }
int shadow = 2
int shadow = shadow + 3
test "places" {
    assert shadow == 5
    assert pair(counter, update()) == 12
    counter = 1
    assert counter + update() == 3
    mut Outer item = Outer{.inner = Inner{.x = 1}, .counts = ["one":1]}
    item.inner.x = 4
    item.counts["two"] = 2
    assert item.inner.x == 4
    assert item.counts["two"] == 2
    mut Inner[] items = [Inner{.x = 1}]
    items[0].x = 5
    assert items[0].x == 5
    mut (int,int) tup = (1,2)
    tup[0] = 3
    assert tup[0] == 3
    mut int? optional = none
    optional = 7
    assert (optional else 0) == 7
}
"#,
        "",
    );
}

#[test]
fn generic_structs_and_enums() {
    success(
        r#"
struct Data<type T> { T data }
enum Result<type T, type E> { Ok(T) Err(E) }
fn wrap<type T>(T value) Data<T> { return Data<T>{.data = value} }
fn result() Result<int,str> { return Result.Ok(42) }
test "generic data" {
    Data<str> text = Data<str>{.data = "hello"}
    assert text.data == "hello"
    Data<Data<int>> nested = Data<Data<int>>{.data = wrap<int>(3)}
    assert nested.data.data == 3
    Result<Data<str>,str> val = Result.Ok(text)
    if val {
        Result.Ok(v) -> { assert v.data == "hello" }
        Result.Err(e) -> { assert false }
    }
    Result<int,str> number = result()
    if number {
        Result.Ok(n) -> { assert n == 42 }
        Result.Err(e) -> { assert false }
    }
    assert (16 >> 2) == 4
}
"#,
        "",
    );
    rejects(
        "struct Data<type T> { T data } Data<int,str> bad = Data<int>{.data = 1}",
        "incorrect number",
    );
    rejects(
        "struct Data<type T> { T data } Data<int> bad = Data<int>{.data = \"bad\"}",
        "expected",
    );
}

#[test]
fn nominal_types_and_checked_casts() {
    success(
        r#"type Name = str
test "nominal" {
    Name name = "hello"
    str plain = @as(str,name)
    assert plain == "hello"
    Name again = @as(Name,plain)
    int[2] fixed = [1,2]
    int[] dynamic = @as(int[],fixed)
    assert dynamic == fixed
    assert @as(int,3.5) == 3
    byte[] bytes = @as(byte[],258)
    assert @as(int,bytes[0]) == 2
    assert @as(int,bytes[1]) == 1
    assert @as(byte[],-1) == [@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255),@as(byte,255)]
    byte[] one = @as(byte[],1.0)
    assert @as(int,one[6]) == 240
    assert @as(int,one[7]) == 63
}"#,
        "",
    );
    rejects(
        "type Name = str fn plain(str s) {} Name n = \"hello\" plain(n)",
        "expected",
    );
    runtime_failure("@println(@as(uint, -@as(int, @args().len)))", "panic:");
}

#[test]
fn recursive_enum_representation() {
    success(
        r#"
enum Tree { Leaf(int) Branch(Tree[]) }
test "recursive" {
    Tree a = Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])
    Tree b = Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])
    assert a == b
    @println(a)
}
"#,
        "Tree.Branch([Tree.Leaf(1), Tree.Leaf(2)])\n",
    );
}

#[test]
fn release_evaluates_pure_functions_and_preserves_effects() {
    let scoped =
        "fn scoped() int { mut int x = 1 { x = 2 int x = 3 } return x } @println(scoped())";
    let c = compile_fixture(scoped, Path::new("scope.nc"), true).unwrap();
    assert!(c.contains("2LL"));
    assert!(!c.contains("nc_fn_scoped"));
    let source = r"fn fib(int n) int { if n { 0,1 -> { return n } _ -> { return fib(n-1)+fib(n-2) } } } @println(fib(10))";
    let c = compile_fixture(source, Path::new("fib.nc"), true).unwrap();
    let main = c.split("int main(void)").last().unwrap();
    assert!(main.contains("55LL"));
    assert!(!main.contains("nc_fn_fib"));
    let effect = "fn effect() int { @println(\"keep\") return 2 } @println(effect())";
    let c = compile_fixture(effect, Path::new("effect.nc"), true).unwrap();
    assert!(
        c.split("int main(void)")
            .last()
            .unwrap()
            .contains("nc_fn_effect")
    );
}

#[test]
fn byte_patterns_cover_the_entire_domain_without_a_wildcard() {
    let arms = (0..=255)
        .map(|value| format!("0x{value:02x} -> {{ {value} }}"))
        .collect::<Vec<_>>()
        .join("\n");
    success(
        &format!(
            r#"
fn identify(byte value) int {{ return if value {{ {arms} }} }}
test "all bytes" {{
    mut uint value = @args().len - 1
    mut int total = 0
    while value < 256 {{
        int result = identify(@as(byte, value))
        assert result == @as(int, value)
        total = total + result
        value = value + 1
    }}
    assert total == 32640
}}
"#
        ),
        "",
    );
    let alternatives = (0..=255)
        .map(|value| format!("0b{value:b}"))
        .collect::<Vec<_>>()
        .join(", ");
    success(
        &format!(
            "fn covered(byte value) bool {{ return if value {{ {alternatives} -> {{ true }} }} }} @println(covered(0), covered(255))"
        ),
        "truetrue\n",
    );
    let incomplete = (0..255)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for patterns in [
        incomplete.clone(),
        format!("{incomplete}, 0"),
        format!("{incomplete}, 0x00, 0b0, 0o0"),
    ] {
        rejects(
            &format!("fn missing(byte value) {{ if value {{ {patterns} -> {{}} }} }}"),
            "not exhaustive",
        );
    }
}

#[test]
fn array_and_string_patterns_require_exact_values_and_lengths() {
    success(
        r#"
fn array(int[] value) int {
    return if value {
        [] -> { 0 }
        [1] -> { 1 }
        [1, n] -> { n }
        _ -> { -1 }
    }
}
fn string(str value) int {
    return if value {
        "" -> { 0 }
        "café" -> { 1 }
        "a\u{0}🍪" -> { 2 }
        _ -> { -1 }
    }
}
fn fixed(int[2] value) int { return if value { [a, b] -> { a + b } } }
fn empty(int[0] value) bool { return if value { [] -> { true } } }
test "exact patterns" {
    assert array([]) == 0
    assert array([1]) == 1
    assert array([1, 7]) == 7
    assert array([1, 7, 8]) == -1
    assert array([2, 7]) == -1
    assert string("") == 0
    assert string("café") == 1
    assert string("café!") == -1
    assert string("cafe") == -1
    assert string("a\u{0}🍪") == 2
    assert string("a\u{0}other") == -1
    assert fixed([2, 3]) == 5
    assert empty([])
}
"#,
        "",
    );
    rejects(
        "fn missing(int[] value) { if value { [] -> {} [x] -> {} } }",
        "not exhaustive",
    );
    rejects(
        "fn missing(str value) { if value { \"\" -> {} \"hello\" -> {} } }",
        "not exhaustive",
    );
}

#[test]
fn float_patterns_use_ieee_equality_for_nonfinite_values_and_signed_zero() {
    success(
        r#"
fn runtime(float value) float { @print("") return value }
fn classify(float value) str {
    return if value {
        NaN -> { "unreachable" }
        inf -> { "positive" }
        -inf -> { "negative" }
        0.0 -> { "zero" }
        _ -> { "other" }
    }
}
@println(classify(runtime(NaN)))
@println(classify(runtime(inf)))
@println(classify(runtime(-inf)))
@println(classify(runtime(0.0)))
@println(classify(runtime(-0.0)))
@println(classify(runtime(1.0)))
"#,
        "other\npositive\nnegative\nzero\nzero\nother\n",
    );
}

#[test]
fn enum_patterns_with_partial_payload_coverage_are_not_exhaustive() {
    for source in [
        "enum Choice { Empty Number(int) } fn missing(Choice value) { if value { Choice.Empty -> {} Choice.Number(0) -> {} } }",
        "enum Choice { Empty Number(int) } fn missing(Choice value, int expected) { if value { Choice.Empty -> {} Choice.Number(expected) -> {} } }",
        "enum Choice { Empty Numbers(int[]) } fn missing(Choice value) { if value { Choice.Empty -> {} Choice.Numbers([]) -> {} Choice.Numbers([x]) -> {} } }",
    ] {
        rejects(source, "not exhaustive");
    }
    success(
        r#"
enum Choice { Empty Number(int) }
fn number(Choice value) int {
    return if value {
        Choice.Number(0) -> { 10 }
        Choice.Number(n) -> { n }
        Choice.Empty -> { -1 }
    }
}
@println(number(Choice.Number(0)), ":", number(Choice.Number(7)), ":", number(Choice.Empty))
"#,
        "10:7:-1\n",
    );
}

#[test]
fn enum_payloads_and_binding_patterns() {
    rejects(
        "enum E { A B } E value = E.A E bad = value.B",
        "through the enum type",
    );
    rejects(
        "enum E { A B } fn value() E { @println(\"effect\") return E.A } E bad = value().B",
        "through the enum type",
    );
    success(
        r#"
enum Node { Empty Text(str) Number(int) }
fn render(Node node) str {
    return if node {
        Node.Empty -> { "empty" }
        Node.Text(text) -> { text }
        Node.Number(n) -> { "number {n}" }
    }
}
test "patterns" {
    Node node = Node.Text("hello")
    assert render(node) == "hello"
    assert render(Node.Number(2)) == "number 2"
    assert node == Node.Text("hello")
    (str,int) p = ("hello",3)
    if p { ("hello", n) -> { assert n == 3 } (_,_) -> {} }
    int[] a = [1,2,3]
    if a { [1,b,c] -> { assert b + c == 5 } _ -> {} }
    @println(Node.Text("quoted"))
}
"#,
        "Node.Text(\"quoted\")\n",
    );
    rejects(
        "enum E { A B } E v = E.A if v { E.A -> {} }",
        "not exhaustive",
    );
}

#[test]
fn maps_mutation_iteration_and_equality() {
    success(
        r#"
test "maps" {
    mut [str]int counts = ["a": 1, "b": 2,]
    counts["c"] = 3
    counts["a"] = 4
    assert counts.len == 3
    assert "a" in counts
    assert not ("z" in counts)
    assert counts["a"] == 4
    mut int total = 0
    for key in counts { total = total + counts[key] }
    assert total == 9
    [str]int reordered = ["c": 3, "a": 4, "b": 2]
    assert counts == reordered
    [str]int combined = counts <> ["a": 5]
    assert combined["a"] == 5
    assert counts["a"] == 4
    [str]int empty = []
    assert empty.len == 0
}
"#,
        "",
    );
}

#[test]
fn strings_interpolation_and_conversion() {
    success(
        r#"
fn describe(int n) str { return "value: {n}" }
@println(describe(7))
@println("nested: {describe(2)}")
@println("escaped: \{2 + 3}")
test "strings" {
    str x = "hello" <> " " <> "world"
    assert "hello" in x
    assert 'w' in x
    assert x == "hello world"
    assert @as(str, 5.0) == "5.0"
}
"#,
        "value: 7\nnested: value: 2\nescaped: {2 + 3}\n",
    );
}

#[test]
fn generic_declarations_require_explicit_correct_type_arguments() {
    let function = "fn identity<T>(T value) T { return value } ";
    rejects(
        &format!("{function}_ = identity(1)"),
        "requires explicit type arguments",
    );
    for use_site in [
        "_ = identity<int, str>(1)",
        "_ = identity<int, int, int>(1)",
    ] {
        rejects(
            &format!("{function}{use_site}"),
            "incorrect number of type arguments",
        );
    }
    for use_site in [
        "Box value = Box{.value = 1}",
        "Box<int, str> value = Box<int, str>{.value = 1}",
        "fn accept(Box value) {}",
        "fn result() Box<int, str> {}",
    ] {
        rejects(
            &format!("struct Box<T> {{ T value }} {use_site}"),
            "incorrect number of type arguments",
        );
    }
    for use_site in [
        "Choice value = Choice.Value(1)",
        "Choice<int, str> value = Choice.Value(1)",
        "fn accept(Choice value) {}",
        "fn result() Choice<int, str> {}",
    ] {
        rejects(
            &format!("enum Choice<T> {{ Value(T) Empty }} {use_site}"),
            "incorrect number of type arguments",
        );
    }
    for source in [
        format!("{function}_ = identity<int>(\"wrong\")"),
        "struct Box<T> { T value } Box<int> value = Box<int>{.value = \"wrong\"}".into(),
        "enum Choice<T> { Value(T) Empty } Choice<int> value = Choice.Value(\"wrong\")".into(),
    ] {
        rejects(&source, "expected `int`, found `str`");
    }
}

#[test]
fn generic_function_specialization() {
    success(
        r#"
struct Vec2 { int x int y }
fn get_x<type T>(T value) int { return value.x }
fn identity<type T>(T value) T { return value }
fn first<type T>(T[] values) T { return values[0] }
test "generic" {
    Vec2 v = Vec2{.x = 3, .y = 4}
    assert get_x<Vec2>(v) == 3
    assert identity<int>(42) == 42
    assert identity<str>("yes") == "yes"
    assert first<int>([1,2,3]) == 1
}
"#,
        "",
    );
    rejects(
        "fn bad<type T>(T x) int { return x.missing } int x = bad<int>(1)",
        "member",
    );
}

#[test]
fn module_namespaces_keep_identical_type_and_generic_names_distinct() {
    let directory = ncc::temp::Directory::new().unwrap();
    let main = directory.path().join("main.nc");
    for (module, bias) in [("left", 10), ("right", 20)] {
        fs::write(
            directory.path().join(format!("{module}.nc")),
            format!(
                r"
pub type Count = int
pub struct Point {{ int x }}
pub struct Box<T> {{ T value }}
pub enum Choice<T> {{ Empty Value(T) }}
int bias = {bias}
fn private_add(int value) int {{ return value + bias }}
pub fn calculate(int value) int {{ return private_add(value) }}
pub fn wrap<T>(T value) Box<T> {{ return Box<T>{{.value = value}} }}
pub fn choose<T>(T value) Choice<T> {{ return Choice.Value(value) }}
"
            ),
        )
        .unwrap();
    }
    let source = r#"
import { "left" as left "right" as right }
fn calculate(int value) int { return value + 100 }
struct Point { int x }
test "namespaces" {
    left.Point a = left.Point{.x = 1}
    right.Point b = right.Point{.x = 2}
    Point local = Point{.x = 3}
    left.Box<left.Point> first = left.wrap<left.Point>(a)
    right.Box<right.Point> second = right.wrap<right.Point>(b)
    assert first.value.x == 1
    assert second.value.x == 2
    assert local.x == 3
    left.Choice<left.Point> variant = left.choose<left.Point>(a)
    if variant {
        left.Choice.Value(p) -> { assert p.x == 1 }
        left.Choice.Empty -> { assert false }
    }
    (fn(int) int) callback = right.calculate
    assert callback(1) == 21
    assert left.calculate(1) == 11
    assert calculate(1) == 101
}
"#;
    fs::write(&main, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&main).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        for invalid in [
            "right.Point value = left.Point{.x = 1}",
            "right.Box<int> value = left.wrap<int>(1)",
            "right.Choice<int> value = left.choose<int>(1)",
            "fn accept(right.Point value) {} accept(left.Point{.x = 1})",
        ] {
            let error = compile_fixture(
                &format!("import {{ \"left\" as left \"right\" as right }} {invalid}"),
                &main,
                release,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("expected `"),
                "{invalid}: {error}"
            );
        }
        for private in [
            "left.bias",
            "left.private_add(1)",
            "right.bias",
            "right.private_add(1)",
        ] {
            let error = compile_fixture(
                &format!("import {{ \"left\" as left \"right\" as right }} _ = {private}"),
                &main,
                release,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("does not export"),
                "{private}: {error}"
            );
        }
    }
}

#[test]
fn nested_imports_resolve_relative_helpers_and_preserve_exported_generic_types() {
    let directory = ncc::temp::Directory::new().unwrap();
    let library = directory.path().join("library");
    fs::create_dir(&library).unwrap();
    fs::write(
        library.join("helper.nc"),
        r"
pub struct Box<T> { T value }
int bias = 2
fn adjust(int value) int { return value + bias }
pub fn answer(int value) int { return adjust(value) }
pub fn wrap<T>(T value) Box<T> { return Box<T>{.value = value} }
",
    )
    .unwrap();
    fs::write(
        library.join("facade.nc"),
        r#"
import { "helper" as helper }
pub fn number() int { return helper.answer(40) }
pub fn boxed() helper.Box<str> { return helper.wrap<str>("a" <> "\u{301}") }
"#,
    )
    .unwrap();
    let main = directory.path().join("main.nc");
    fs::write(
        &main,
        r#"
import { "library/facade" as facade "library/helper" as helper }
test "nested imports" {
    assert facade.number() == 42
    helper.Box<str> result = facade.boxed()
    assert result.value.len == 2
    assert result.value[0] == 'a'
    assert result.value[1] == '\u{301}'
    assert result.value == "a" <> "\u{301}"
}
"#,
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&main).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        let error = compile_fixture(
            "import { \"library/facade\" as facade } _ = facade.helper.answer(1)",
            &main,
            release,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not export"), "{error}");
    }
}

#[test]
fn modules_exports_and_external_functions() {
    let dir = ncc::temp::Directory::new().unwrap();
    fs::write(
        dir.path().join("one.nc"),
        "pub int value = 7 pub fn square(int n) int { return n * n } int hidden = 9\n\
         pub int first, str second = (3, \"four\")\n\
         pub int head, (int, int) tail = (5, 6, 7)\n\
         int private_first, int private_second = (5, 6)",
    )
    .unwrap();
    fs::write(dir.path().join("two.nc"), "pub int value = 2").unwrap();
    fs::write(
        dir.path().join("native.c"),
        "int64_t native_add(int64_t a, int64_t b) { return a + b; }",
    )
    .unwrap();
    let main = dir.path().join("main.nc");
    fs::write(
        &main,
        r#"
import { "one" as one "two" as two }
extern "native.c" as native { fn add(int a, int b) int = "native_add" }
@println(native.add(one.square(one.value), two.value))
@println(one.first)
@println(one.second)
@println(one.tail)
"#,
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&main).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"51\n3\nfour\n(6, 7)\n");
        for name in ["private_first", "private_second"] {
            assert!(
                compile_fixture(
                    &format!("import {{ \"one\" as one }} @println(one.{name})"),
                    &main,
                    release
                )
                .unwrap_err()
                .to_string()
                .contains("does not export")
            );
        }
        assert!(
            compile_fixture(
                "import { \"one\" as one } @println(one.hidden)",
                &main,
                release
            )
            .unwrap_err()
            .to_string()
            .contains("does not export")
        );
        fs::write(dir.path().join("cycle.nc"), "import { \"cycle\" as again }").unwrap();
        assert!(
            compile_fixture("import { \"cycle\" as cycle }", &main, release)
                .unwrap_err()
                .to_string()
                .contains("cyclic")
        );
    }
}

#[test]
fn imported_types_and_patterns() {
    let dir = ncc::temp::Directory::new().unwrap();
    fs::write(
        dir.path().join("data.nc"),
        r"
pub struct Point { int x }
pub struct Box<type T> { T value }
pub enum Choice { Point(Point) Empty }
pub fn number(Choice choice) int {
    return if choice { Choice.Point(p) -> { p.x } Choice.Empty -> { 0 } }
}
",
    )
    .unwrap();
    let main = dir.path().join("main.nc");
    fs::write(
        &main,
        r#"
import { "data" as data }
test "imported types" {
    data.Point point = data.Point{.x = 7}
    data.Box<data.Point> boxed = data.Box<data.Point>{.value = point}
    data.Choice choice = data.Choice.Point(boxed.value)
    assert data.number(choice) == 7
    if choice { data.Choice.Point(p) -> { assert p.x == 7 } data.Choice.Empty -> { assert false } }
    data.Point data = point
    assert data.x == 7
}
"#,
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&main).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn optional_and_error_fallbacks_evaluate_only_the_active_branch_once() {
    success(
        r#"
fn optional(bool present) int? {
    @print("optional ")
    if present { true -> { return 7 } false -> { return none } }
}
fn checked(bool succeeds) int! {
    @print("checked ")
    if succeeds { true -> { return 8 } false -> { throw "failure" } }
}
fn fallback() int { @print("fallback ") return 9 }
@println(optional(true) else fallback())
@println(optional(false) else fallback())
@println(optional(true) else { @print("wrong ") break fallback() })
@println(optional(false) else { @print("block ") break fallback() })
@println(checked(true) catch message { @print(message, " ") break fallback() })
@println(checked(false) catch message { @print(message, " ") break fallback() })
fn forward(bool succeeds) int! {
    int value = try checked(succeeds)
    @print("after ")
    return value + 1
}
@println(forward(true) catch message { @print("wrong ") break 0 })
@println(forward(false) catch message { @print(message, " ") break 0 })
"#,
        "optional 7\noptional fallback 9\noptional 7\noptional block fallback 9\nchecked 8\nchecked failure fallback 9\nchecked after 9\nchecked failure 0\n",
    );
}

#[test]
fn optional_and_error_payload_extraction_preserves_value_copies() {
    success(
        r#"
fn failed() int[]! { throw "failure" }
test "payload copies" {
    mut int[] original = [1, 2]
    int[]? present = original
    int[]? absent = none
    int[]! successful = original
    int[]! failure = failed()
    mut int[] from_present = present else [0]
    mut int[] from_success = successful catch message { break [0] }
    mut int[] from_none = absent else original
    mut int[] from_error = failure catch message { break original }
    from_present[0] = 3
    from_success[0] = 4
    from_none[0] = 5
    from_error[0] = 6
    assert original == [1, 2]
    assert (present else [0]) == [1, 2]
    assert (successful catch message { break [0] }) == [1, 2]
    original[1] = 9
    assert from_present == [3, 2]
    assert from_success == [4, 2]
    assert from_none == [5, 2]
    assert from_error == [6, 2]
    assert (present else [0]) == [1, 2]
    assert (successful catch message { break [0] }) == [1, 2]
}
"#,
        "",
    );
}

#[test]
fn top_level_error_propagation_preserves_messages_and_skips_later_effects() {
    for handler in ["try fail()", "fail() catch message { throw message }"] {
        let source = format!(
            r#"
fn fail() int! {{ @println("before") throw "bad\u{{0}}🍪" }}
int value = {handler}
@println("unreachable", value)
"#
        );
        for release in [false, true] {
            compile_fixture(&source, Path::new("propagation.nc"), release).unwrap();
            let output = run_mode(&source, release);
            assert_eq!(output.status.code(), Some(1), "release={release}");
            assert_eq!(output.stdout, b"before\n", "release={release}");
            assert_eq!(output.stderr, "bad\0🍪\n".as_bytes(), "release={release}");
        }
    }
}

#[test]
fn optional_and_error_operations_require_explicit_unwrapping() {
    for source in [
        "int? value = 1 int result = value",
        "fn take(int value) {} int? value = 1 take(value)",
        "fn result() int { int? value = 1 return value }",
        "int? value = 1 _ = value + 1",
        "int? value = 1 _ = value else false",
        "int? value = 1 _ = value else { break false }",
        "int! value = 1 int result = value",
        "fn take(int value) {} int! value = 1 take(value)",
        "fn result() int { int! value = 1 return value }",
        "int! value = 1 _ = value + 1",
        "int! value = 1 _ = value catch message { break false }",
    ] {
        rejects(
            source,
            if source.starts_with("int? ") && source.contains(" + ") {
                "arithmetic requires numeric operands"
            } else {
                "expected `"
            },
        );
    }
    rejects("_ = 1 else 2", "else requires an optional value");
    rejects(
        "_ = 1 catch message { break 2 }",
        "catch requires an error union",
    );
    rejects("_ = try 1", "try requires an error union");
    rejects(
        "fn failure() int! { throw \"failure\" } fn invalid() int { return try failure() }",
        "try requires a throwing function",
    );
    rejects("int! value = 1 _ = message", "unknown name");
    rejects(
        "int! value = 1 _ = value catch message { break 0 } _ = message",
        "unknown name",
    );
}

#[test]
fn error_unions_catch_and_propagation() {
    success(
        r#"
fn checked(int n) int! { if n { 0 -> { throw "zero" } _ -> { return n } } }
fn forwarded(int n) int! { return try checked(n) }
fn notify() ! { throw "notification" }
test "errors" {
    int good = try checked(3)
    assert good == 3
    int bad = forwarded(0) catch error { break 99 }
    assert bad == 99
    void! pending = notify()
    pending catch error { @println(error) }
}
"#,
        "notification\n",
    );
    rejects("fn bad() int { throw \"bad\" }", "throwing function");
    runtime_failure(
        "fn bad() int! { throw \"failure\" } int n = try bad()",
        "failure\n",
    );
}

#[test]
fn optional_values_and_conditional_expressions() {
    success(
        r#"
fn defaulted(int? value, int fallback) int { return value else fallback }
test "values" {
    int n = 2
    char letter = if n {
        1 -> { break 'A' }
        2 -> { break 'B' }
        _ -> { break 'Z' }
    }
    assert letter == 'B'
    int value = if true { true -> { 42 } false -> { 0 } }
    assert value == 42
    int? empty = none
    int? full = 5
    assert defaulted(empty, 7) == 7
    assert defaulted(full, 7) == 5
    assert defaulted(3, 7) == 3
    int answer = empty else { break 99 }
    assert answer == 99
}
"#,
        "",
    );
    rejects("int value = none", "cannot infer");
    rejects("int? value = 1 int result = value", "expected");
}

#[test]
fn checked_integer_arithmetic() {
    success(
        r#"test "numbers" {
        assert 2 ** 6 == 64
        assert 2 ** 3 ** 2 == 512
        assert 0b1011 == 11
        assert 0o777 == 511
        assert 5 / 2 == 2
        int low = -9223372036854775808
        uint high = 18446744073709551615u
        assert low < 0
        assert high > 0
        byte b = 255
        assert b == 255
        assert (1 << 4) == 16
    }"#,
        "",
    );
    for source in [
        "int n = 9223372036854775807 @println(n + @as(int, @args().len))",
        "byte b = 255 @println(b + @as(byte, @args().len))",
        "int n = @as(int, @args().len) - 1 @println(1 / n)",
        "@println(@as(int, @args().len) << 64)",
        "@println((@as(int, @args().len) + 1) ** 63)",
    ] {
        runtime_failure(source, "panic:");
    }
}

#[test]
fn tuples_and_structs() {
    success(
        r#"
struct Fraction { int numerator int denominator }
fn pair(int a, int b) (int, int) { return a + b, a - b }
test "records" {
    mut Fraction f = Fraction{.numerator = 1, .denominator = 10}
    f.numerator = f.numerator * 2
    assert f == Fraction{.numerator = 2, .denominator = 10}
    (int, int) vals = pair(2, 4)
    int a, int b = vals
    assert vals == (6, -2)
    assert a == 6 and b == -2
    assert vals[0] == 6
}
"#,
        "",
    );
    rejects("struct A { int x } A a = A{.x = true}", "expected");
    rejects("struct A { int x } A a = A{.y = 1}", "unknown field");
}

#[test]
fn fixture_modes_preserve_execution_and_module_declaration_scopes() {
    let ordinary = "str text = \"test body\" // test \"not a root\"\n@println(text)";
    assert!(!fixture_has_tests(ordinary));
    assert_eq!(executable_fixture(ordinary), ordinary);
    success(ordinary, "test body\n");
    rejects("str text = \"test\" _ = missing", "unknown name");
    rejects("test \"negative root\" { _ = missing }", "unknown name");

    let mixed = r#"
mut int counter = 1
fn read() int { return counter }
@println(read())
counter = counter + 1
int value = counter
int value = value + 10
@println(counter, ":", value)
test "module bindings" { assert read() == 2 assert value == 12 }
"#;
    let rooted = executable_fixture(mixed);
    let module = fixture_module(&rooted).unwrap();
    assert_eq!(
        module
            .items
            .iter()
            .filter(|item| matches!(item, ncc::ast::Item::Global(_)))
            .count(),
        3
    );
    assert!(
        !module
            .items
            .iter()
            .any(|item| matches!(item, ncc::ast::Item::Statement(_)))
    );
    assert_eq!(executable_fixture(&rooted), rooted);
    success(mixed, "1\n2:12\n");
}

#[test]
fn test_body_runtime_failures_keep_the_runtime_barrier() {
    runtime_failure(
        "test \"runtime panic\" { @println(1 / 0) }",
        "division by zero",
    );
}

#[test]
fn imported_only_test_roots_use_explicit_test_mode() {
    let dir = ncc::temp::Directory::new().unwrap();
    fs::write(
        dir.path().join("dependency.nc"),
        "mut int value = 1 value = value + 1 pub fn read() int { return value }",
    )
    .unwrap();
    fs::write(
        dir.path().join("suite.nc"),
        r#"
import { "dependency" as dependency }
@println("unrelated imported output")
test "imported root" { @println("imported test") assert dependency.read() == 2 }
"#,
    )
    .unwrap();
    let source = "import { \"suite\" as suite }";
    assert!(!fixture_has_tests(source));
    let main = dir.path().join("main.nc");
    fs::write(&main, source).unwrap();
    for release in [false, true] {
        // Local AST detection cannot discover imported roots: these fixtures
        // deliberately choose the test API and CLI rather than the adaptive helper.
        ncc::compile_test_source_with_options(source, &main, release).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&main).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"imported test\n", "release={release}");
    }
}

fn fixture_module(source: &str) -> Result<ncc::ast::Module, ncc::diagnostic::Diagnostics> {
    ncc::parser::parse(ncc::lexer::lex(source)?)
}

fn fixture_has_tests(source: &str) -> bool {
    fixture_module(source).is_ok_and(|module| {
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
    let module = fixture_module(source).unwrap();
    if !module
        .items
        .iter()
        .any(|item| matches!(item, ncc::ast::Item::Test { .. }))
    {
        return source.to_owned();
    }
    // These mixed fixtures predate test slicing and intentionally execute every
    // top-level statement. Root each statement separately, keeping declarations
    // in module scope so closures, shadowing and cross-test bindings stay intact.
    let mut source = source.to_owned();
    for (index, item) in module.items.iter().enumerate().rev() {
        if let ncc::ast::Item::Statement(statement) = item {
            let (_, span) = statement.source().unwrap();
            source.insert_str(span.end, "\n}\n");
            source.insert_str(
                span.start,
                &format!("test \"fixture statement {index}\" {{\n"),
            );
        }
    }
    source
}

fn runtime_failure(source: &str, message: &str) {
    let mut source = executable_fixture(source);
    // Printing is analysable. Put the runtime barrier inside each test root,
    // rather than in an unrelated top-level statement that slicing would drop.
    if fixture_has_tests(&source) {
        let module = fixture_module(&source).unwrap();
        for item in module.items.iter().rev() {
            if let ncc::ast::Item::Test { body, .. } = item
                && let Some(statement) = body.statements.first()
            {
                let (_, span) = statement.source().unwrap();
                source.insert_str(span.start, "_ = @args()\n");
            }
        }
    } else {
        source.insert_str(0, "_ = @args()\n");
    }
    for release in [false, true] {
        // Require valid NC first: a front-end error must not masquerade as a panic.
        compile_fixture(&source, Path::new("test.nc"), release).unwrap();
        let output = run_mode(&source, release);
        assert_eq!(output.status.code(), Some(1), "release={release}: {source}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(message),
            "release={release}: {source}\n{stderr}"
        );
    }
}
fn run_mode(source: &str, release: bool) -> std::process::Output {
    let source = executable_fixture(source);
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("test.nc");
    fs::write(&file, &source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command.arg(if fixture_has_tests(&source) {
        "test"
    } else {
        "run"
    });
    if release {
        command.arg("-r");
    }
    command.arg(&file).output().unwrap()
}
fn success(source: &str, stdout: &str) {
    for release in [false, true] {
        let output = run_mode(source, release);
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            stdout,
            "release={release}"
        );
    }
}
fn rejects(source: &str, message: &str) {
    for release in [false, true] {
        let error = compile_fixture(source, Path::new("test.nc"), release).unwrap_err();
        assert!(
            error.to_string().contains(message),
            "release={release}: {error}"
        );
    }
}

#[test]
fn shadowing_initialization_and_typed_output() {
    success(
        r#"
fn greeting() str { return "hello" }
str value = greeting()
@println(value)
test "shadow" {
    int x = 1
    mut int x = x + 1
    x = x + 1
    { str x = "inner" @println(x) }
    @println(x)
    bool b = true
    float f = 2.5
    @println(b, " ", f)
}
"#,
        "hello\ninner\n3\ntrue 2.5\n",
    );
}

#[test]
fn functions_cannot_be_used_as_type_names() {
    for declaration in [
        "type Bad = function",
        "struct Bad { function value }",
        "enum Bad { Value(function) }",
        "fn bad(function value) {}",
        "fn bad() function { return function }",
        "function value = function",
        "function[] values = []",
    ] {
        rejects(
            &format!("fn function() int {{ return 1 }} {declaration}"),
            "not a type",
        );
    }
}

#[test]
fn invalid_operator_type_matrix_is_rejected_in_both_modes() {
    for (ty, value, operators) in [
        ("bool", "true", "+ - * / % ** < <= > >= & | ^ << >> <>"),
        ("str", "\"x\"", "+ - * / % ** < <= > >= & | ^ << >> and or"),
        (
            "char",
            "'x'",
            "+ - * / % ** < <= > >= & | ^ << >> and or <>",
        ),
        ("float", "1.0", "& | ^ << >> and or <>"),
        ("int", "1", "and or <>"),
        ("uint", "1", "and or <>"),
        ("byte", "1", "and or <>"),
        ("int[]", "[1]", "+ - * / % ** < <= > >= & | ^ << >> and or"),
        (
            "[str]int",
            "[\"x\": 1]",
            "+ - * / % ** < <= > >= & | ^ << >> and or",
        ),
        (
            "(int, str)",
            "(1, \"x\")",
            "+ - * / % ** < <= > >= & | ^ << >> and or <>",
        ),
    ] {
        for op in operators.split_whitespace() {
            let source = format!("{ty} a = {value}\n{ty} b = {value}\n_ = a {op} b");
            for release in [false, true] {
                assert!(
                    compile_fixture(&source, Path::new("operators.nc"), release).is_err(),
                    "release={release}: {source}"
                );
            }
        }
    }
    for source in [
        "fn identity<T>(T value) T { return value } fn f() int { return 1 } fut int work = async f() _ = identity<fut int>(work)",
        "enum Hidden<T> { Value(T) } fn bad() Hidden<fut int> {}",
        "struct Hidden<T> { T value } fn bad() Hidden<fut int>[] {}",
    ] {
        rejects(source, "futures cannot be returned");
    }
}

#[test]
fn scalar_semantic_errors() {
    rejects("fn bad() int { return missing }", "unknown name");
    rejects("fn bad() int {}", "without returning");
    rejects("bool x = 1 and 2", "expected");
    rejects("float x = 1.0 & 2.0", "integers");
    rejects("fn value() int { return 1 } value()", "not used");
    rejects("if 1 { 1 -> {} }", "not exhaustive");
    rejects("assert true", "only available");
    rejects("byte b = 256", "does not fit");
}

#[test]
fn short_circuit_and_labels() {
    success(
        r#"
fn noisy() bool { @println("wrong") return true }
test "control" {
    bool a = false and noisy()
    bool b = true or noisy()
    mut int i = 0
    outer: while i < 4 {
        i = i + 1
        while true { break :outer }
    }
    assert i == 1
}
"#,
        "",
    );
}

#[test]
fn void_values_can_be_stored_and_passed_without_losing_effects() {
    success(
        r#"
fn unit() { @print("u") }
fn take(void value) void { @print("t") return value }
fn identity<T>(T value) T { return value }
struct Holder { void value }
enum Choice { Value(void) Empty }
void global = unit()
test "void storage" {
    _ = global
    mut void local = unit()
    local = take(local)
    void[2] array = [local, unit()]
    (void, int) tuple = (local, 2)
    [int]void map = [1:local]
    Holder holder = Holder{.value=unit()}
    Choice choice = Choice.Value(local)
    void? optional = local
    void value = optional else { unit() }
    void copied = identity<void>(local)
    take(array[0])
    take(tuple[0])
    take(map[1])
    take(holder.value)
    take(value)
    take(copied)
    if choice { Choice.Value(v) -> { take(v) } Choice.Empty -> {} }
    fn callback = fn(void value) { take(value) }
    callback(local)
    fut void task = async take(local)
    void awaited = await task
    take(awaited)
    assert array.len == 2
    assert map.len == 1
}
"#,
        "uutuutttttttttt",
    );
}

#[test]
fn nominal_void_and_optional_void_preserve_type_context() {
    success(
        r#"
type Unit = void
fn unit() {}
fn wrapped() Unit { return @as(Unit, unit()) }
Unit value = wrapped()
void plain = @as(void, value)
void? present = plain
void? absent = none
void unwrapped = present else { @println("wrong") }
void fallback = absent else { @println("fallback") }
Unit[2] values = [value, wrapped()]
@println(values.len)
"#,
        "fallback\n2\n",
    );
}

#[test]
fn fixed_and_dynamic_array_concatenation_preserves_sizes_and_value_copies() {
    success(
        r#"
fn fixed() int[2] { @print("fixed ") return [1, 2] }
fn dynamic() int[] { @print("dynamic ") return [3, 4] }
fn nothing() int[0] { @print("empty ") return [] }
@println(fixed() <> dynamic())
@println(dynamic() <> fixed())
@println(nothing() <> fixed() <> nothing())
test "array concatenation" {
    int[0] zero = []
    int[1] one = [1]
    int[2] two = [2, 3]
    int[3] three = one <> two
    int[3] with_empty = zero <> three <> zero
    int[0] still_empty = zero <> zero
    int[] empty = []
    int[] dynamic_left = empty <> three
    int[] dynamic_right = three <> empty
    int[] both_dynamic = dynamic_left <> dynamic_right
    assert with_empty == [1, 2, 3]
    assert still_empty.len == 0
    assert dynamic_left == three
    assert dynamic_right == three
    assert both_dynamic == [1, 2, 3, 1, 2, 3]

    mut int[][1] left = [[1, 2]]
    mut int[][1] right = [[3, 4]]
    mut int[][2] joined = left <> right
    joined[0][0] = 9
    assert left[0] == [1, 2]
    right[0][0] = 8
    assert joined[1] == [3, 4]
    mut int[][] converted = @as(int[][], joined)
    converted[1][0] = 7
    assert joined[1] == [3, 4]
    assert converted == [[9, 2], [7, 4]]
}
"#,
        "fixed dynamic [1, 2, 3, 4]\ndynamic fixed [3, 4, 1, 2]\nempty fixed empty [1, 2]\n",
    );
}

#[test]
fn fixed_array_conversions_and_concatenations_reject_incompatible_sizes() {
    for source in [
        "int[] values = [1, 2] int[2] fixed = values",
        "int[] values = [1, 2] int[2] fixed = @as(int[2], values)",
        "int[] values = [] int[0] fixed = values",
        "int[] values = [] int[0] fixed = @as(int[0], values)",
        "int[1] a = [1] int[] b = [2] int[2] joined = a <> b",
        "int[] a = [1] int[1] b = [2] int[2] joined = a <> b",
        "int[] a = [1] int[] b = [2] int[2] joined = a <> b",
        "int[0] a = [] int[] b = [] int[0] joined = a <> b",
        "int[1] a = [1] int[2] b = [2, 3] int[2] joined = a <> b",
        "fn take(int[2] values) {} int[] values = [1, 2] take(values)",
        "fn wrong() int[2] { int[] values = [1, 2] return values }",
        "int[]? values = [1, 2] int[2] fixed = values else [0, 0]",
    ] {
        rejects(
            source,
            if source.contains("@as") {
                "this cast is not implemented"
            } else {
                "expected `int["
            },
        );
    }
}

#[test]
fn fixed_array_lengths_use_integer_literal_syntax() {
    success(
        r#"
fn identity(int[0x2] values) int[0b10u] { return values }
test "array lengths" {
    int[0o2] values = identity([3, 4])
    int[2u] decimal = values
    int[0x0] empty = []
    int[0b10][0o1] nested = [[3, 4]]
    assert decimal == [3, 4]
    assert empty.len == 0
    assert nested[0] == values
}
"#,
        "",
    );
    for literal in ["0x", "0b2", "0o8", "18446744073709551616", "1.5"] {
        for source in [
            format!("fn f(int[{literal}] values) {{}}"),
            format!("int[{literal}] values = []"),
            format!("fn f() {{ int[{literal}] values = [] }}"),
            format!("int[{literal}]? values = none"),
            format!("int[{literal}][2] values = []"),
        ] {
            let start = source.find(literal).unwrap();
            for release in [false, true] {
                let errors = compile_fixture(&source, Path::new("size.nc"), release).unwrap_err();
                assert!(errors.to_string().contains("array size"), "{errors}");
                assert_eq!(errors.0[0].span, start..start + literal.len());
            }
        }
    }
    rejects("int[-1] values = []", "expected integer array size");
    rejects("int[size] values = []", "expected integer array size");
    rejects("int[0x2] values = [1]", "array");
}

#[test]
fn composite_construction_preserves_effect_order_and_deep_copies() {
    success(
        r#"
fn mark(int n) int { @print(n) return n }
struct Pair { int first int second }
enum Choice { Pair(int, int) }
int[] array = [mark(1), mark(2)]
(int, int) tuple = (mark(3), mark(4))
[int]int map = [mark(5): mark(6), mark(7): mark(8)]
Pair pair = Pair{.second = mark(9), .first = mark(10)}
Choice choice = Choice.Pair(mark(11), mark(12))
@println("")
struct Bundle { int[][] rows [str]int[] table int[]? maybe }
fn returned(Bundle bundle) Bundle { return bundle }
test "deep copies" {
    mut int[] row = [1, 2]
    Bundle original = Bundle{.rows = [row], .table = ["row": row], .maybe = row}
    mut Bundle copy = returned(original)
    row[0] = 99
    copy.rows[0][0] = 3
    copy.table["row"][0] = 4
    mut int[] optional = copy.maybe else []
    optional[0] = 5
    assert original.rows == [[1, 2]]
    assert original.table["row"] == [1, 2]
    assert (original.maybe else []) == [1, 2]
    assert copy.rows == [[3, 2]]
    assert copy.table["row"] == [4, 2]
    assert (copy.maybe else []) == [1, 2]
    fn captured = fn() int[][] { return copy.rows }
    copy.rows[0][0] = 6
    mut int[][] captured_rows = captured()
    captured_rows[0][0] = 7
    assert captured() == [[6, 2]]
    assert array == [1, 2] and tuple == (3, 4)
    assert map == [5: 6, 7: 8]
    assert pair == Pair{.first = 10, .second = 9}
    assert choice == Choice.Pair(11, 12)
    int[2][2] fixed = [[1, 2], [3, 4]]
    int[][] dynamic = fixed
    assert dynamic == fixed
    int[][][] deep = [fixed]
    assert deep == [[[1, 2], [3, 4]]]
    assert fixed in [dynamic]
}
"#,
        "123456789101112\n",
    );
}

#[test]
fn arrays_indexing_iteration_and_value_copies() {
    success(
        r#"
fn first(int[] values) int { return values[0] }
test "arrays" {
    int[2] a = [1, 2]
    int[3] b = [3, 4, 5]
    int[5] all = a <> b
    assert all == [1, 2, 3, 4, 5]
    assert all[$] == 5
    assert all[$-1] == 4
    assert all.len == 5
    assert 3 in all
    mut int[] copy = all
    copy[0] = 99
    assert all[0] == 1
    mut int sum = 0
    for i in all { sum = sum + all[i] }
    assert sum == 15
    assert first(all) == 1
    int[] empty = []
    assert empty.len == 0
    @println(copy)
}
"#,
        "[99, 2, 3, 4, 5]\n",
    );
    rejects("test \"bad\" { int[] a = [1] a[0] = 2 }", "immutable");
    rejects("int[2] a = [1]", "length");
    runtime_failure("int[] a = [1] @println(a[2])", "out of bounds");
}

#[test]
fn integer_arithmetic_boundaries_and_overflow_in_both_modes() {
    // The CLI passes only the executable name. Reading its argument count keeps
    // failure operands runtime-dependent even with release constant evaluation.
    for (ty, max, high_power) in [
        ("byte", "255", "7"),
        ("int", "9223372036854775807", "62"),
        ("uint", "18446744073709551615u", "63"),
    ] {
        success(
            &format!(
                r#"test "boundaries" {{
                    {ty} max = {max}
                    {ty} zero = 0
                    {ty} one = 1
                    {ty} two = 2
                    {ty} power = {high_power}
                    assert max + zero == max
                    assert max - zero == max
                    assert max * one == max
                    assert max / one == max
                    assert max % one == zero
                    assert max ** one == max
                    assert max << zero == max
                    assert max >> zero == max
                    assert two ** power == one << power
                    assert (one << power) >> power == one
                }}"#
            ),
            "",
        );
        for expression in ["max + one", "max * two", "max ** two", "max << one"] {
            runtime_failure(
                &format!(
                    "{ty} max = {max} {ty} one = @as({ty}, @args().len) {ty} two = one + one @println({expression})"
                ),
                "panic: integer overflow",
            );
        }
    }
    success(
        r#"test "signed lower boundary" {
            int min = -9223372036854775808
            assert min + 0 == min
            assert min - 0 == min
            assert min * 1 == min
            assert min / 1 == min
            assert min ** 1 == min
            assert min << 0 == min
        }"#,
        "",
    );
    for source in [
        "int min = -9223372036854775808 @println(min - @as(int, @args().len))",
        "int min = -9223372036854775808 @println(min * -@as(int, @args().len))",
        "int min = -9223372036854775808 @println(min / -@as(int, @args().len))",
        "int min = -9223372036854775808 @println(min << @as(int, @args().len))",
        "byte zero = 0 @println(zero - @as(byte, @args().len))",
        "uint zero = 0 @println(zero - @args().len)",
    ] {
        runtime_failure(source, "panic: integer overflow");
    }
}

#[test]
fn arithmetic_shifts_round_negative_values_down_at_runtime() {
    success(
        r#"
fn right(int value, int count) int { return value >> count }
fn left(int value, int count) int { return value << count }
type Signed = int
fn nominal(Signed value, Signed count) Signed { return @as(Signed, @as(int, value) >> @as(int, count)) }
test "arithmetic shifts" {
    int zero = @as(int, @args().len) - 1
    int[] values = [-9223372036854775808, -9223372036854775807, -5, -4, -3, -2, -1, 0, 1, 2, 3, 9223372036854775807]
    int[] halves = [-4611686018427387904, -4611686018427387904, -3, -2, -2, -1, -1, 0, 0, 1, 1, 4611686018427387903]
    for i in values {
        assert right(values[i], zero) == values[i]
        assert right(values[i], zero + 1) == halves[i]
        int sign = if values[i] < 0 { true -> { -1 } false -> { 0 } }
        assert right(values[i], zero + 63) == sign
    }
    assert left(-3, zero + 1) == -6
    assert left(-1, zero + 63) == -9223372036854775808
    assert @as(int, nominal(@as(Signed, -3), @as(Signed, zero + 1))) == -2
    uint high = 18446744073709551615u
    assert high >> @as(uint, zero + 63) == 1u
    byte high_byte = 255
    assert high_byte >> @as(byte, zero + 7) == 1
}
"#,
        "",
    );
}

#[test]
fn arithmetic_shifts_match_floor_division_for_every_valid_count() {
    let values = [i64::MIN, i64::MIN + 1, -65, -3, -1, 0, 1, 3, 65, i64::MAX];
    let mut source = String::from(
        "fn shift(int value, int count) int { return value >> count }\n\
         test \"floor division oracle\" {\n\
         int zero = @as(int, @args().len) - 1\n",
    );
    for value in values {
        // Use wider floor division as an oracle independent of bit shifting.
        let expected = (0..64)
            .map(|count| (i128::from(value).div_euclid(2_i128.pow(count))).to_string())
            .collect::<Vec<_>>()
            .join(",");
        write!(
            source,
            "int[] expected = [{expected}]\n\
             for count in expected {{\n\
             assert shift({value}, @as(int, count) + zero) == expected[count]\n\
             }}\n"
        )
        .unwrap();
    }
    source.push_str("}\n");
    success(&source, "");
}

#[test]
fn named_function_side_effects_share_outer_mutable_bindings() {
    success(
        r#"
mut int val = 0
fn increment() { val = val + 1 }
@println(val)
increment()
@println(val)
val = val + 5
@println(val)
increment()
@println(val)

struct State { int[] values [str]int counts }
mut State state = State{.values = [1], .counts = ["calls": 0]}
fn update() int {
    state.values[0] = state.values[0] + val
    state.counts["calls"] = state.counts["calls"] + 1
    return state.values[0]
}
fn invoke((fn() int) callback) int { return callback() }
fn forward() int { return update() }
test "outer effects" {
    val = 10
    assert invoke(forward) == 11
    assert state.counts["calls"] == 1
    state = State{.values = [20], .counts = ["calls": 4]}
    (fn() int) alias = update
    assert alias() == 30
    assert state.values == [30]
    assert state.counts["calls"] == 5
    State copy = state
    _ = alias()
    assert state.values == [40]
    assert copy.values == [30]
    assert copy.counts["calls"] == 5
}
"#,
        "0\n1\n6\n7\n",
    );
}

#[test]
fn escaped_closures_share_bindings_and_keep_independent_invocations() {
    success(
        r#"
struct Counter { (fn() int) read (fn(int) void) write (fn() void) increment }
fn counter(int initial) Counter {
    mut int value = initial
    fn read() int { return value }
    fn write(int next) { value = next }
    fn increment() { value = value + 1 }
    return Counter{.read = read, .write = write, .increment = increment}
}
test "shared cells" {
    Counter first = counter(2)
    Counter copy = first
    Counter second = counter(10)
    first.write(5)
    copy.increment()
    assert first.read() == 6
    assert copy.read() == 6
    assert second.read() == 10
    mut int outer = 1
    fn write_only = fn() { outer = 8 }
    write_only()
    assert outer == 8
    fn nested = fn() (fn() void) { return fn() { outer = outer + 1 } }
    (fn() void) increment = nested()
    increment()
    assert outer == 9
    mut int outer = 100
    increment()
    assert outer == 100
    assert first.read() == 6
    mut int a, int b = (1, 2)
    fn change = fn() { a = 3 b = 4 }
    change()
    assert a == 3 and b == 4
}
"#,
        "",
    );
}

#[test]
fn loop_closures_retain_distinct_mutable_cells_and_immutable_values() {
    success(
        r#"
fn make() (fn() int)[] {
    mut (fn() int)[] callbacks = []
    for i in [10, 20, 30] {
        mut int count = @as(int, i)
        fn next = fn() int { count = count + 1 return count }
        callbacks = callbacks <> [next]
    }
    return callbacks
}
test "iteration cells" {
    (fn() int)[] callbacks = make()
    assert callbacks[0]() == 1
    assert callbacks[1]() == 2
    assert callbacks[0]() == 2
    assert callbacks[2]() == 3
    assert callbacks[1]() == 3
    int fixed = 7
    fn read = fn() int { return fixed }
    int fixed = 99
    assert read() == 7
}
"#,
        "",
    );
}

#[test]
fn closures_capture_mutexes_without_inheriting_lock_permissions() {
    for body in [
        "fn later = fn() { value = 2 }",
        "fn later() { value = 2 }",
        "fn outer = fn() { fn inner = fn() { value = 2 } }",
    ] {
        rejects(
            &format!("mutex int value = 1 lock value {{ {body} }}"),
            "immutable",
        );
    }
    success(
        r#"
fn make() (fn() int) {
    mutex int value = 1
    lock value {
        value = 2
        return fn() int {
            lock value { value = value + 1 return value }
        }
    }
}
test "lock permissions" {
    (fn() int) next = make()
    assert next() == 3
    assert next() == 4
    mutex int value = 5
    mut (fn() int) read = fn() int { return 0 }
    lock value { read = fn() int { lock value { return value } } }
    lock value { value = 6 }
    assert read() == 6
    fut int first = async next()
    fut int second = async next()
    int a = await first
    int b = await second
    assert a + b == 11
    assert next() == 7
}
"#,
        "",
    );
}

#[test]
fn mutex_reads_require_an_explicit_lock_in_each_function() {
    for expression in [
        "_ = value",
        "int copy = value",
        "@println(value)",
        "str text = \"{value}\"",
        "str text = @as(str, value)",
        "bool same = value == 1",
        "if value { 1 -> {} _ -> {} }",
        "if 1 { value -> {} _ -> {} }",
        "if (1, 2) { (value, _) -> {} _ -> {} }",
        "fn read() int { return value }",
        "fn read = fn() int { return value }",
        "lock value {} int copy = value",
        "lock value { fn read() int { return value } }",
        "lock value { fn read = fn() int { return value } }",
    ] {
        rejects(
            &format!("mutex int value = 1 {expression}"),
            "cannot read mutex `value` outside a lock scope",
        );
    }
    for expression in [
        "uint length = values.len",
        "int first = values[0]",
        "int[] copy = values",
        "for i in values {}",
        "bool present = 1 in values",
    ] {
        rejects(
            &format!("mutex int[] values = [1] {expression}"),
            "cannot read mutex `values` outside a lock scope",
        );
    }
    rejects(
        "struct State { int count } mutex State state = State{.count = 1} int n = state.count",
        "cannot read mutex `state` outside a lock scope",
    );
    success(
        r#"
test "explicit read scopes" {
    mutex int[] values = [1, 2]
    mut int[] snapshot = []
    lock values {
        snapshot = values
        assert values.len == 2
        assert values[0] == 1
        assert 2 in values
        for i in values { assert values[i] > 0 }
        fut void printed = async @println(values)
        await printed
    }
    snapshot[0] = 99
    lock values { assert values == [1, 2] }
    mutex int value = 1
    lock value {
        if 1 { value -> {} _ -> { assert false } }
    }
}
"#,
        "[1, 2]\n",
    );
}

#[test]
fn pattern_comparisons_capture_existing_outer_bindings() {
    success(
        r#"
fn make(int initial) (fn(int) bool) {
    mut int expected = initial
    fn matches(int value) bool { return if value { expected -> { true } _ -> { false } } }
    expected = expected + 1
    return matches
}
test "pattern captures" {
    (fn(int) bool) matches = make(2)
    assert matches(3)
    assert not matches(2)
    int expected = 4
    fn tuple((int, int) value) bool {
        return if value { (expected, _) -> { true } _ -> { false } }
    }
    assert tuple((4, 9))
    assert not tuple((5, 9))
}
"#,
        "",
    );
}

#[test]
fn composite_patterns_preserve_captures_and_comparison_order() {
    success(
        r#"
struct Item { int key str label }
enum Choice { Item(Item) Empty }
fn mark(int n) int { @print(n) return n }
test "composite captures" {
    mut int expected = 1
    str label = "item"
    fn array(int[] value) bool {
        return if value { [expected, _] -> { true } _ -> { false } }
    }
    fn record(Item value) bool {
        return if value { Item{.key = expected, .label = label} -> { true } _ -> { false } }
    }
    fn variant(Choice value) bool {
        return if value { Choice.Item(Item{.key = expected, .label = label}) -> { true } _ -> { false } }
    }
    expected = 2
    assert array([2, 9])
    assert not array([1, 9])
    assert not array([2])
    assert record(Item{.key = 2, .label = "item"})
    assert not record(Item{.key = 2, .label = "other"})
    assert variant(Choice.Item(Item{.key = 2, .label = "item"}))
    assert not variant(Choice.Empty)
    assert not variant(Choice.Item(Item{.key = 1, .label = "item"}))
    if mark(2) {
        mark(1) -> { assert false }
        mark(2) -> {}
        mark(3) -> { assert false }
        _ -> { assert false }
    }
    @println("")
}
"#,
        "212\n",
    );
}
