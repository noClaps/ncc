use std::fmt::Write as _;
use std::{fs, process::Command};

#[test]
fn c_extern_strings_preserve_explicit_character_boundaries() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r#"
nc_abi_roundtrip_result roundtrip(nc_abi_roundtrip_arg0 value) { return value; }
nc_abi_raw_result raw(void) { return NC_STRING("a\xcc\x81"); }
nc_abi_separate_result separate(void) {
    static const size_t ends[] = {1, 3};
    return (nc_string){3, "a\xcc\x81", 2, ends};
}
"#,
    )
    .unwrap();
    fs::write(
        &input,
        r#"
extern "native.c" as native {
    fn roundtrip(str value) str = "roundtrip"
    fn raw() str = "raw"
    fn separate() str = "separate"
}
test "string ABI" {
    str separate = "a" <> "\u{301}"
    str result = native.roundtrip(separate)
    assert result.len == 2
    assert result == separate
    assert result != "a\u{301}"
    assert native.raw().len == 1
    assert native.raw() == "a\u{301}"
    assert native.separate().len == 2
    assert native.separate() == separate
    assert @as(char[], result) == ['a', '\u{301}']
    mut str copy = result
    copy[0] = 'b'
    assert separate[0] == 'a'
    assert result[0] == 'a'
}
"#,
    )
    .unwrap();
    run_both(&input, b"");
}

#[test]
fn c_extern_nonfinite_floats_keep_ieee_values_and_canonical_formatting() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r"
#include <math.h>
nc_abi_invalid_result invalid(void) { return -NAN; }
nc_abi_positive_result positive(void) { return INFINITY; }
nc_abi_negative_result negative(void) { return -INFINITY; }
nc_abi_roundtrip_result roundtrip(nc_abi_roundtrip_arg0 value) { return value; }
",
    )
    .unwrap();
    let declarations = r#"
extern "native.c" as native {
    fn invalid() float = "invalid"
    fn positive() float = "positive"
    fn negative() float = "negative"
    fn roundtrip(float value) float = "roundtrip"
}
"#;
    fs::write(
        &input,
        declarations.to_owned()
            + r#"
float invalid = native.invalid()
float positive = native.positive()
float negative = native.negative()
test "nonfinite ABI" {
    assert invalid != invalid
    assert positive == inf
    assert negative == -inf
    assert native.roundtrip(inf) == inf
    assert native.roundtrip(-inf) == -inf
@println(invalid, ":", positive, ":", negative)
@println("{invalid}:{positive}:{negative}")
@println(@as(str, invalid), ":", @as(str, positive), ":", @as(str, negative))
@eprintln(invalid, ":", positive, ":", negative)
@println(native.roundtrip(NaN))
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
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            b"NaN:inf:-inf\nNaN:inf:-inf\nNaN:inf:-inf\nNaN\n"
        );
        assert_eq!(output.stderr, b"NaN:inf:-inf\n");
    }
    for function in ["invalid", "positive", "negative"] {
        for ty in ["int", "uint"] {
            fs::write(
                &input,
                format!("{declarations}\n_ = @as({ty}, native.{function}())"),
            )
            .unwrap();
            for release in [false, true] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
                command.arg("run");
                if release {
                    command.arg("-r");
                }
                let output = command.arg(&input).output().unwrap();
                assert_eq!(output.status.code(), Some(1));
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("cast out of range"),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[test]
fn c_extern_nominal_float_error_chains_preserve_active_payloads_and_cast_checks() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        "nc_abi_roundtrip_result roundtrip(nc_abi_roundtrip_arg0 value) { return value; }\n",
    )
    .unwrap();
    let declarations = r#"
type Result = float!
type Outer = Result
extern "native.c" as native {
    fn roundtrip(Outer value) Outer = "roundtrip"
}
fn roundtrip(float! value) float! {
    Outer wrapped = @as(Outer, @as(Result, value))
    return try @as(float!, @as(Result, native.roundtrip(wrapped)))
}
fn failed() float! { throw "bad" }
"#;
    fs::write(
        &input,
        declarations.to_owned()
            + r#"
test "nominal nonfinite error ABI" {
    float! nan = NaN
    float! positive = inf
    float! negative = -inf
    float![] values = [
        roundtrip(nan), roundtrip(positive), roundtrip(negative), roundtrip(failed())
    ]
    @println(values)
    @println("{values}")
    @println(@as(str, values))
}
"#,
    )
    .unwrap();
    run_both(
        &input,
        b"[NaN, inf, -inf, error: bad]\n[NaN, inf, -inf, error: bad]\n[NaN, inf, -inf, error: bad]\n",
    );
    for value in ["NaN", "inf", "-inf"] {
        for ty in ["int", "uint"] {
            fs::write(
                &input,
                format!(
                    r"{declarations}
float! successful = {value}
float extracted = roundtrip(successful) catch message {{ break 0.0 }}
@println(@as({ty}, extracted))
",
                ),
            )
            .unwrap();
            for release in [false, true] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
                command.arg("run");
                if release {
                    command.arg("-r");
                }
                let output = command.arg(&input).output().unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(1),
                    "{value} -> {ty}, release={release}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    output.stdout.is_empty(),
                    "{value} -> {ty}, release={release}: {:?}",
                    output.stdout
                );
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("cast out of range"),
                    "{value} -> {ty}, release={release}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[test]
fn c_abi_roundtrips_scalar_nominal_and_composite_values() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    let cases = [
        ("boolean", "bool", "true"),
        ("octet", "byte", "255"),
        ("signed_value", "int", "-9223372036854775808"),
        ("unsigned_value", "uint", "18446744073709551615u"),
        ("real", "float", "1.25"),
        ("character", "char", "'🍪'"),
        ("text", "str", r#""a\u{0}🍪""#),
        ("nominal", "Count", "@as(Count, 42u)"),
        ("array", "int[]", "[1, 2]"),
        ("fixed", "int[2]", "[1, 2]"),
        ("nested", "int[][]", "[[1], [2, 3]]"),
        ("tuple", "(int, str)", "(1, \"two\")"),
        ("mapping", "[str]int[]", "[\"one\":[1]]"),
        ("record", "Record", "Record{.values=[1, 2]}"),
        ("variant", "Choice", "Choice.Values([1, 2])"),
        ("empty_variant", "Choice", "Choice.Empty"),
        ("present", "int[]?", "[1, 2]"),
        ("absent", "int[]?", "none"),
        ("success", "int[]!", "[1, 2]"),
        ("failure", "int[]!", "failed()"),
    ];
    let mut source = String::from(
        "type Count = uint\nstruct Record { int[] values }\nenum Choice { Empty Values(int[]) }\nfn failed() int[]! { throw \"failure\" }\nextern \"native.c\" as native {\n",
    );
    let mut c = String::new();
    for (name, ty, _) in cases {
        writeln!(source, "fn {name}({ty} value) {ty} = \"{name}\"").unwrap();
        writeln!(
            c,
            "nc_abi_{name}_result {name}(nc_abi_{name}_arg0 value) {{ return value; }}"
        )
        .unwrap();
    }
    source.push_str("fn callback((fn(int) int) value) (fn(int) int) = \"callback\"\nfn invoke((fn(int) int) callback, int n) int = \"invoke\"\nfn observe(fut int value) bool = \"observe\"\n}\nfn increment(int n) int { return n + 1 }\ntest \"ABI\" {\n");
    c.push_str("nc_abi_callback_result callback(nc_abi_callback_arg0 value) { return value; }\n");
    c.push_str("nc_abi_invoke_result invoke(nc_abi_invoke_arg0 callback, nc_abi_invoke_arg1 n) { return callback.call(callback.env, n); }\nnc_abi_observe_result observe(nc_abi_observe_arg0 value) { return value != 0; }\n");
    for (name, ty, value) in cases {
        write!(
            source,
            "{ty} {name} = {value}\nassert native.{name}({name}) == {name}\n"
        )
        .unwrap();
    }
    source.push_str(
        "(fn(int) int) callback = native.callback(increment)\nassert callback(41) == 42\nint extra = 2\nassert native.invoke(fn(int n) int { return n + extra }, 40) == 42\nfut int future = async increment(41)\nassert native.observe(future)\nassert await future == 42\n}\n",
    );
    fs::write(directory.path().join("native.c"), c).unwrap();
    fs::write(&input, source).unwrap();
    run_both(&input, b"");
}

#[test]
fn void_storage_and_native_void_returns_have_distinct_abi_types() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r"
static int calls;
nc_abi_touch_result touch(nc_abi_touch_arg0 value) { calls += value == 0; }
nc_abi_count_result count(void) { return calls; }
nc_abi_roundtrip_result roundtrip(nc_abi_roundtrip_arg0 values) { return values; }
",
    )
    .unwrap();
    fs::write(
        &input,
        r#"
extern "native.c" as native {
    fn touch(void value) = "touch"
    fn count() int = "count"
    fn roundtrip(void[] values) void[] = "roundtrip"
}
fn unit() {}
void value = native.touch(unit())
void[] values = native.roundtrip([value, native.touch(value)])
test "void ABI output" { @println(native.count(), ":", values.len) }
"#,
    )
    .unwrap();
    run_both(&input, b"2:2\n");
}

#[test]
fn inactive_external_payloads_are_not_read_or_copied() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r#"
nc_abi_absent_result absent(void) {
    nc_abi_absent_result result = {0};
    result.value.len = 1; result.value.vals = 0;
    return result;
}
nc_abi_failed_result failed(void) {
    nc_abi_failed_result result = {0};
    result.failed = 1; result.error = NC_STRING("failure");
    result.value.len = 1; result.value.vals = 0;
    return result;
}
nc_abi_succeeded_result succeeded(void) {
    static int64_t values[] = {7};
    nc_abi_succeeded_result result = {0};
    result.error.bytes = 1; result.error.data = 0;
    result.value.len = 1; result.value.cap = 1; result.value.vals = values;
    return result;
}
"#,
    )
    .unwrap();
    fs::write(
        &input,
        r#"
extern "native.c" as native {
    fn absent() int[]? = "absent"
    fn failed() int[]! = "failed"
    fn succeeded() int[]! = "succeeded"
}
int[]? optional = native.absent()
int[]! failure = native.failed()
test "inactive ABI payloads" {
@println(optional == native.absent())
@println(failure == native.failed())
}
int[] fallback = failure catch message { @println(message);break [9] }
test "inactive ABI formatting" {
@println(fallback)
@println(failure, ":", native.succeeded())
@println("{failure}:{native.succeeded()}")
@println(@as(str, failure), ":", @as(str, native.succeeded()))
}
int[]![] results = [failure, native.succeeded()]
test "inactive ABI containers" {
@println(results)
}
"#,
    )
    .unwrap();
    run_both(
        &input,
        b"true\ntrue\nfailure\n[9]\nerror: failure:[7]\nerror: failure:[7]\nerror: failure:[7]\n[error: failure, [7]]\n",
    );
}

#[test]
fn shared_c_sources_see_all_abi_declarations_and_are_included_once() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r"
nc_abi_first_result first(nc_abi_first_arg0 value) { return value + second(2); }
nc_abi_second_result second(nc_abi_second_arg0 value) { return value * 2; }
",
    )
    .unwrap();
    fs::write(
        directory.path().join("other.nc"),
        r#"
extern "native.c" as native { fn second(int n) int = "second" }
pub fn value() int { return native.second(3) }
"#,
    )
    .unwrap();
    let source = r#"
extern "native.c" as native { fn first(int n) int = "first" }
import { "other" as other }
test "shared native implementation" { @println(native.first(1), other.value()) }
"#;
    fs::write(&input, source).unwrap();
    let c = ncc::compile_test_source(source, &input).unwrap();
    assert_eq!(c.matches("native.c\"").count(), 1);
    run_both(&input, b"56\n");
    let conflict = r#"
extern "native.c" as one { fn first(int n) int = "first" }
extern "native.c" as two { fn first(str n) int = "first" }
"#;
    for release in [false, true] {
        let error = ncc::compile_source_with_options(conflict, &input, release).unwrap_err();
        assert!(
            error.to_string().contains("conflicting declarations"),
            "release={release}: {error}"
        );
        assert_eq!(error.0[0].path.as_ref(), Some(&input));
    }
    for symbol in [
        "int",
        "signed",
        "unsigned",
        "return",
        "_Atomic",
        "_Thread_local",
    ] {
        let source = format!("extern \"native.c\" as native {{ fn value() int = \"{symbol}\" }}");
        for release in [false, true] {
            let error = ncc::compile_source_with_options(&source, &input, release).unwrap_err();
            assert!(
                error.to_string().contains("C keyword"),
                "release={release}, symbol={symbol}: {error}"
            );
            assert_eq!(error.0[0].path.as_ref(), Some(&input));
        }
    }
}

fn run_both(input: &std::path::Path, expected: &[u8]) {
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected);
    }
}
