use std::{fs, process::Command};

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
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"56\n");
    }
    let conflict = r#"
extern "native.c" as one { fn first(int n) int = "first" }
extern "native.c" as two { fn first(str n) int = "first" }
"#;
    assert!(
        ncc::check_source(conflict, &input)
            .unwrap_err()
            .to_string()
            .contains("conflicting declarations")
    );
}
