use std::{fs, process::Command};

#[test]
fn embeds_binary_empty_and_module_relative_files_without_runtime_dependencies() {
    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("lib")).unwrap();
    let bytes: Vec<u8> = (0..=255).collect();
    fs::write(root.join("data.bin"), &bytes).unwrap();
    fs::write(root.join("empty"), []).unwrap();
    fs::write(
        root.join("lib/data.nc"),
        "pub fn bytes() byte[] { return @embed(\"../data.bin\") }",
    )
    .unwrap();
    let input = root.join("main.nc");
    let source = format!(
        r#"
import {{ "lib/data" as data }}
byte[] bytes = data.bytes()
byte[] absolute = @embed("{}")
byte[] empty = @embed("empty")
test "embedded" {{
    assert bytes == absolute
    assert bytes.len == 256
    assert bytes[0] == 0
    assert bytes[$] == 255
    assert empty.len == 0
}}
@println(bytes.len)
"#,
        root.join("data.bin").display()
    );
    fs::write(&input, source).unwrap();
    let binaries = [root.join("debug"), root.join("release")];
    for (mode, binary) in ["-d", "-r"].into_iter().zip(&binaries) {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("build")
            .arg(&input)
            .arg(mode)
            .arg("-o")
            .arg(binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_file(root.join("data.bin")).unwrap();
    fs::remove_file(root.join("empty")).unwrap();
    for binary in binaries {
        let output = Command::new(binary).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"256\n");
    }
    let error = ncc::check_source("byte[] bytes = @embed(\"missing\")", &input).unwrap_err();
    assert!(error.to_string().contains("cannot embed"));
    assert_eq!(error.0[0].path.as_ref(), Some(&input));
    assert_eq!(error.0[0].span.start, 15);
    for invalid in [
        "@embed()",
        "@embed(1)",
        "@embed(\"file\", 1)",
        "@embed(variable)",
        "@embed(\"{variable}\")",
    ] {
        assert!(
            ncc::check_source(&format!("_ = {invalid}"), &input).is_err(),
            "{invalid}"
        );
    }
}

#[test]
#[cfg(unix)]
fn rejects_symlink_files_and_parent_directories() {
    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("actual")).unwrap();
    fs::write(root.join("actual/data"), [42]).unwrap();
    std::os::unix::fs::symlink("actual/data", root.join("link")).unwrap();
    std::os::unix::fs::symlink("actual", root.join("directory-link")).unwrap();
    for name in ["link", "directory-link/data"] {
        let error = ncc::check_source(&format!("_ = @embed(\"{name}\")"), &root.join("main.nc"))
            .unwrap_err();
        assert!(
            error.to_string().contains("does not follow symlinks"),
            "{error}"
        );
    }
}
