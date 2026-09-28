use std::{fs, process::Command};

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
"#,
    )
    .unwrap();
    fs::write(
        &input,
        r#"
extern "native.c" as native {
    fn absent() int[]? = "absent"
    fn failed() int[]! = "failed"
}
int[]? optional = native.absent()
int[]! failure = native.failed()
@println(optional == native.absent())
@println(failure == native.failed())
int[] fallback = failure catch message { @println(message) break [9] }
@println(fallback)
"#,
    )
    .unwrap();
    run_both(&input, b"true\ntrue\nfailure\n[9]\n");
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
