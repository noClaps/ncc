use std::{fs, process::Command};
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn runtime_process_builtins_and_target_selection() {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("process.nc");
    let binary = dir.path().join("process");
    fs::write(
        &file,
        r#"
str[] args = @args()
[str]str environment = @env()
str os, str arch = @target()
@println(args.len)
@println(args[1])
@println(args[2])
@println(environment["NC_TEST_PROCESS_VALUE"])
@println(os)
@println(arch)
"#,
    )
    .unwrap();
    assert_eq!(cli(&["--targets"]).stdout, b"macos-arm64\n");
    for release in [false, true] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args([
                "build",
                file.to_str().unwrap(),
                "-o",
                binary.to_str().unwrap(),
                if release { "-r" } else { "-d" },
            ])
            .env("NC_TARGET", "invalid-default")
            .args(["--target", "macos-arm64"])
            .env("NC_TEST_PROCESS_VALUE", "compile-time")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&binary)
            .args(["one two", "--help"])
            .env("NC_TEST_PROCESS_VALUE", "runtime=✓")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "3\none two\n--help\nruntime=✓\nmacos\narm64\n"
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args(["run", file.to_str().unwrap(), "--", "first", "second"])
        .env("NC_TEST_PROCESS_VALUE", "forwarded")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"3\nfirst\nsecond\nforwarded\nmacos\narm64\n"
    );
    for target in ["macos-arm64", "invalid"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args([
                "build",
                file.to_str().unwrap(),
                "-f",
                "C",
                "-o",
                dir.path().join("process.c").to_str().unwrap(),
            ])
            .env("NC_TARGET", target)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), target == "macos-arm64");
    }
    for name in ["args", "env", "target"] {
        let error = ncc::check_source(&format!("_ = @{name}(1)"), &file).unwrap_err();
        assert!(error.to_string().contains("expects no arguments"));
    }
}

#[test]
fn help_and_invalid_options() {
    for command in ["build", "run", "check", "fmt", "lsp"] {
        let out = cli(&[command, "--help"]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("Usage: ncc {command}")));
    }
    for args in [
        vec!["build", "-o"],
        vec!["build", "-f"],
        vec!["build", "--target"],
        vec!["build", "file.nc", "--target=invalid"],
        vec!["--targets", "unexpected"],
        vec!["run", "file.nc", "-f", "C"],
        vec!["check", "file.nc", "--release"],
        vec!["fmt", "one", "two"],
        vec!["lsp", "unexpected"],
        vec!["unknown"],
    ] {
        let out = cli(&args);
        assert!(!out.status.success(), "{args:?}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains("No such file"));
    }
}
#[test]
fn release_changes_generated_code_and_detects_overflow() {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("fib.nc");
    let output = dir.path().join("output.c");
    let source = "fn fib(int n) int { if n { 0,1 -> { return n } _ -> { return fib(n-1)+fib(n-2) } } } @println(fib(45))";
    fs::write(&file, source).unwrap();
    let file = file.to_str().unwrap();
    let output = output.to_str().unwrap();
    assert!(
        cli(&["build", "--debug", file, "--output", output])
            .status
            .success()
    );
    let debug = fs::read_to_string(output).unwrap();
    assert!(debug.contains("nc_fn_fib"));
    assert!(
        cli(&["build", "--release", file, "--format=C", "--output", output])
            .status
            .success()
    );
    let release = fs::read_to_string(output).unwrap();
    assert!(release.contains("1134903170LL"));
    assert!(!release.contains("nc_fn_fib"));
    assert!(release.len() < debug.len());
    fs::write(file, source.replace("fib(45)", "fib(100)")).unwrap();
    let out = cli(&["build", "-r", file, "-o", output]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("integer overflow"));
    assert_eq!(fs::read_to_string(output).unwrap(), release);
    assert!(!cli(&["build", file, "-o", file]).status.success());
}
#[test]
fn capture_warnings_imports_and_suppression() {
    let dir = ncc::temp::Directory::new().unwrap();
    let root = dir.path().join("main.nc");
    let imported = dir.path().join("lib.nc");
    let source = "pub fn make(int n) (fn() int) { return fn() int { return n } }";
    fs::write(&imported, source).unwrap();
    fs::write(&root, "import { \"lib\" as lib } _ = lib.make(1)").unwrap();
    let out = cli(&["check", root.to_str().unwrap()]);
    assert!(out.status.success());
    let warning = String::from_utf8_lossy(&out.stderr);
    assert!(warning.contains("lib.nc:1:"));
    assert!(warning.contains("warning: [capture]"));
    assert!(warning.contains("function parameters"));
    fs::write(&imported, format!("// @ncc lint disable capture\n{source}")).unwrap();
    let out = cli(&["check", root.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let clean = "fn make(int n) (fn(int) int) { return fn(int x) int { return x } }";
    assert!(ncc::lint::check(clean, &root).unwrap().is_empty());
}
