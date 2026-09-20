use std::{fs, process::Command};
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args(args)
        .output()
        .unwrap()
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
