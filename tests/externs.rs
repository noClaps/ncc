use std::{fs, process::Command};

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
        source.push_str(&format!("fn {name}({ty} value) {ty} = \"{name}\"\n"));
        c.push_str(&format!(
            "nc_abi_{name}_result {name}(nc_abi_{name}_arg0 value) {{ return value; }}\n"
        ));
    }
    source.push_str("fn callback((fn(int) int) value) (fn(int) int) = \"callback\"\nfn invoke((fn(int) int) callback, int n) int = \"invoke\"\nfn observe(fut int value) bool = \"observe\"\n}\nfn increment(int n) int { return n + 1 }\ntest \"ABI\" {\n");
    c.push_str("nc_abi_callback_result callback(nc_abi_callback_arg0 value) { return value; }\n");
    c.push_str("nc_abi_invoke_result invoke(nc_abi_invoke_arg0 callback, nc_abi_invoke_arg1 n) { return callback.call(callback.env, n); }\nnc_abi_observe_result observe(nc_abi_observe_arg0 value) { return value != 0; }\n");
    for (name, ty, value) in cases {
        source.push_str(&format!(
            "{ty} {name} = {value}\nassert native.{name}({name}) == {name}\n"
        ));
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
        r#"
static int calls;
nc_abi_touch_result touch(nc_abi_touch_arg0 value) { calls += value == 0; }
nc_abi_count_result count(void) { return calls; }
nc_abi_roundtrip_result roundtrip(nc_abi_roundtrip_arg0 values) { return values; }
"#,
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
@println(native.count(), ":", values.len)
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
@println(optional == native.absent())
@println(failure == native.failed())
int[] fallback = failure catch message { @println(message) break [9] }
@println(fallback)
@println(failure, ":", native.succeeded())
@println("{failure}:{native.succeeded()}")
@println(@as(str, failure), ":", @as(str, native.succeeded()))
int[]![] results = [failure, native.succeeded()]
@println(results)
"#,
    )
    .unwrap();
    run_both(
        &input,
        b"true\ntrue\nfailure\n[9]\nfailure:[7]\nfailure:[7]\nfailure:[7]\n[failure, [7]]\n",
    );
}

#[test]
fn shared_c_sources_see_all_abi_declarations_and_are_included_once() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r#"
nc_abi_first_result first(nc_abi_first_arg0 value) { return value + second(2); }
nc_abi_second_result second(nc_abi_second_arg0 value) { return value * 2; }
"#,
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
@println(native.first(1), other.value())
"#;
    fs::write(&input, source).unwrap();
    let c = ncc::compile_source(source, &input).unwrap();
    assert_eq!(c.matches("native.c\"").count(), 1);
    run_both(&input, b"56\n");
    let conflict = r#"
extern "native.c" as one { fn first(int n) int = "first" }
extern "native.c" as two { fn first(str n) int = "first" }
"#;
    assert!(
        ncc::compile_source(conflict, &input)
            .unwrap_err()
            .to_string()
            .contains("conflicting declarations")
    );
    for symbol in [
        "int",
        "signed",
        "unsigned",
        "return",
        "_Atomic",
        "_Thread_local",
    ] {
        let source = format!("extern \"native.c\" as native {{ fn value() int = \"{symbol}\" }}");
        let error = ncc::compile_source(&source, &input).unwrap_err();
        assert!(error.to_string().contains("C keyword"), "{error}");
    }
}

fn run_both(input: &std::path::Path, expected: &[u8]) {
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
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
