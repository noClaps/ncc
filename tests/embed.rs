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
    @println(bytes.len)
}}
"#,
        root.join("data.bin").display()
    );
    fs::write(&input, &source).unwrap();
    let binaries = [root.join("debug"), root.join("release")];
    for (mode, binary) in ["-d", "-r"].into_iter().zip(&binaries) {
        let c = ncc::compile_test_source_with_options(&source, &input, mode == "-r").unwrap();
        let c_path = root.join("embedded.c");
        fs::write(&c_path, c).unwrap();
        let output = Command::new("cc")
            .arg(&c_path)
            .arg("-std=c11")
            .arg(if mode == "-r" { "-O3" } else { "-O0" })
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
    for release in [false, true] {
        let error =
            ncc::compile_source_with_options("byte[] bytes = @embed(\"missing\")", &input, release)
                .unwrap_err();
        assert!(error.to_string().contains("cannot embed"), "{error}");
        assert_eq!(error.0[0].path.as_ref(), Some(&input));
        assert_eq!(error.0[0].span.start, 15);
    }
    for invalid in [
        "@embed()",
        "@embed(1)",
        "@embed(\"file\", 1)",
        "@embed(variable)",
        "@embed(\"{variable}\")",
    ] {
        for release in [false, true] {
            assert!(
                ncc::compile_source_with_options(&format!("_ = {invalid}"), &input, release)
                    .is_err(),
                "release={release}: {invalid}"
            );
        }
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
    std::os::unix::fs::symlink("missing", root.join("dangling")).unwrap();
    std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
    for name in [
        "link",
        "directory-link/data",
        "directory-link/../actual/data",
        "dangling",
        "loop",
    ] {
        for file in [std::path::PathBuf::from(name), root.join(name)] {
            rejects_embed(
                &format!("_ = @embed(\"{}\")", file.display()),
                &root.join("main.nc"),
                "does not follow symlinks",
            );
        }
    }
}

fn rejects_embed(source: &str, input: &std::path::Path, expected: &str) {
    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, input, release).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\n{error}"
        );
        assert_eq!(error.0[0].path.as_deref(), Some(input));
        assert_eq!(error.0[0].span.start, 4);
    }
}

#[test]
fn rejects_missing_files_directories_and_non_directory_parents() {
    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("directory")).unwrap();
    fs::write(root.join("file"), [42]).unwrap();
    let input = root.join("main.nc");
    for name in ["missing", "directory", "file/child"] {
        rejects_embed(&format!("_ = @embed(\"{name}\")"), &input, "cannot embed");
    }
    rejects_embed("_ = @embed(\"file\u{0}child\")", &input, "cannot embed");
    ncc::compile_source_with_options("_ = @embed(\"file\")", &input, false).unwrap();
    ncc::compile_source_with_options("_ = @embed(\"file\")", &input, true).unwrap();
}

#[test]
#[cfg(unix)]
fn rejects_controlled_unreadable_files() {
    use std::os::unix::fs::PermissionsExt;

    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("unreadable");
    fs::write(&file, [42]).unwrap();
    let permissions = fs::metadata(&file).unwrap().permissions();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let read = fs::read(&file);
    let input = root.join("main.nc");
    let source = "_ = @embed(\"unreadable\")";
    let results =
        [false, true].map(|release| ncc::compile_source_with_options(source, &input, release));
    // Restore before assertions so a failure cannot leave an unreadable fixture behind.
    fs::set_permissions(&file, permissions).unwrap();
    let Err(read_error) = read else {
        eprintln!("skipping permission-denied assertions: this user can read mode-000 files");
        return;
    };
    assert_eq!(read_error.kind(), std::io::ErrorKind::PermissionDenied);
    for (release, result) in [false, true].into_iter().zip(results) {
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("cannot embed"),
            "release={release}: {error}"
        );
        assert!(
            error.to_string().contains(&read_error.to_string()),
            "{error}"
        );
        assert_eq!(error.0[0].path.as_ref(), Some(&input));
        assert_eq!(error.0[0].span.start, 4);
    }
    for release in [false, true] {
        ncc::compile_source_with_options(source, &input, release).unwrap();
    }
}

#[test]
fn computed_paths_follow_lexical_constants_in_debug_and_release() {
    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("data1.bin"), [0, 42, 255]).unwrap();
    fs::write(root.join("name"), b"data1.bin").unwrap();
    fs::write(root.join("paths.nc"), "pub str filename = \"data1.bin\"").unwrap();
    let input = root.join("main.nc");
    let source = r#"
import { "paths" as paths }
str stem = "data"
fn filename(uint n) str { return "{stem}{n}.bin" }
fn bytes() byte[] {
    str local = filename(1)
    return @embed(local)
}
byte[] a = @embed(stem <> "1.bin")
byte[] b = @embed("{stem}1.bin")
byte[] c = @embed(filename(1))
byte[] d = @embed(paths.filename)
fn choose = fn() str { return "data1.bin" }
byte[] e = @embed(choose())
byte[] nested = @embed(if @embed("name").len {
    9 -> { "data1.bin" }
    _ -> { "missing" }
})
str first, str _ = ("data1.bin", "ignored")
str _, (str, int) tail = ("ignored", "data1.bin", 5)
str grouped, (int, str) rest = ("data1.bin", (1, "ignored"))
byte[] tuple_path = @embed(first)
byte[] partial_path = @embed(tail[0])
byte[] grouped_path = @embed(grouped)
fn tuple_local() byte[] {
    str path, int n = ("data1.bin", 1)
    str path, int n = (path, n + 1)
    return @embed(path)
}
fn captured_path() byte[] {
    str name, str suffix = ("data1", ".bin")
    fn choose = fn() str { return name <> suffix }
    return @embed(choose())
}
fn nested_capture() byte[] {
    str name = "data1.bin"
    fn outer = fn() str {
        fn inner = fn() str { return name }
        return inner()
    }
    return @embed(outer())
}
fn shared_path() str {
    mut str name = "missing"
    fn choose() { name = "data1.bin" }
    choose()
    return name
}
byte[] shared = @embed(shared_path())
test "computed paths" {
    assert shared == a
    assert a == b and b == c and c == d and d == bytes()
    assert e == a and nested == a
    assert tuple_path == a and partial_path == a and grouped_path == a
    assert tuple_local() == a
    assert captured_path() == a and nested_capture() == a
    byte[] expected = [0, 42, 255]
    assert a == expected
    @println(a)
}
"#;
    fs::write(&input, source).unwrap();
    ncc::compile_test_source(source, &input).unwrap();
    for mode in ["-d", "-r"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args(["test", input.to_str().unwrap(), mode])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"[0, 42, 255]\n");
    }
}

#[test]
fn runtime_dependent_paths_fail_without_executing_effects() {
    let temp = ncc::temp::Directory::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("data"), [42]).unwrap();
    let input = root.join("main.nc");
    for source in [
        "fn path() str { mut str name = \"data\";fn choose() str { @println(\"effect\");return name };return choose() };_ = @embed(path())",
        "mut str path = \"data\"\nbyte[] b = @embed(path)",
        "str path = \"data\"\nfn f(str path) byte[] { return @embed(path) }",
        "fn path() str { @println(\"effect\")\n return \"data\" }\nbyte[] b = @embed(path())",
        "fn f(str path) byte[] { str copy = path\n return @embed(copy) }",
        "fn loop() str { return loop() }\nbyte[] b = @embed(loop())",
        "fn path() str { return \"data\" }\nfn f((fn() str) path) byte[] { return @embed(path()) }",
        "mut str path, int n = (\"data\", 1)\n_ = @embed(path)",
        "fn f(str runtime) byte[] { str path, int n = (runtime, 1)\n return @embed(path) }",
        "str path, int n = (\"data\", 1)\nfn f(str path) byte[] { return @embed(path) }",
        "fn f(str path) byte[] { fn choose = fn() str { return path };return @embed(choose()) }",
        "mut str path = \"data\"\nfn choose = fn() str { return path }\n_ = @embed(choose())",
    ] {
        for release in [false, true] {
            let error = ncc::compile_source_with_options(source, &input, release).unwrap_err();
            assert!(
                error.to_string().contains("compile-time string"),
                "release={release}: {source}: {error}"
            );
            assert_eq!(error.0[0].path.as_ref(), Some(&input));
            assert!(source[error.0[0].span.clone()].starts_with("@embed("));
        }
    }
}
