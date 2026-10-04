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
        let error = ncc::compile_source(&format!("_ = @{name}(1)"), &file).unwrap_err();
        assert!(error.to_string().contains("expects no arguments"));
    }
}

#[test]
fn help_and_invalid_options() {
    for command in ["build", "run", "test"] {
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
        vec!["test"],
        vec!["test", "file.nc", "-f", "C"],
        vec!["test", "file.nc", "-o", "output"],
        vec!["test", "file.nc", "--target", "macos-arm64"],
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
fn removed_commands_are_rejected_without_touching_input() {
    let dir = ncc::temp::Directory::new().unwrap();
    let path = dir.path().join("input.nc");
    let source = "  @println(1)\n";
    fs::write(&path, source).unwrap();
    let help = cli(&["--help"]);
    let help = String::from_utf8(help.stdout).unwrap();
    for command in ["check", "fmt", "lsp"] {
        assert!(
            !help
                .lines()
                .any(|line| line.trim_start().starts_with(command))
        );
        for args in [
            vec![command],
            vec![command, "--help"],
            vec![command, path.to_str().unwrap()],
        ] {
            let out = cli(&args);
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
        }
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn release_changes_generated_code_and_detects_overflow() {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("fib.nc");
    let output = dir.path().join("output.c");
    let source = "fn fib(int n) int { if n { 0,1 -> { return n } _ -> { return fib(n-1)+fib(n-2) } } };@println(fib(45))";
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
    assert!(
        release.contains("1134903170"),
        "final output must be precomputed"
    );
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
fn tests_execute_in_source_order_only_in_test_mode() {
    let dir = ncc::temp::Directory::new().unwrap();
    let input = dir.path().join("main.nc");
    let binary = dir.path().join("built");
    fs::write(
        dir.path().join("library.nc"),
        "pub fn value() int { return 7 };test \"imported\" { @println(\"imported\") }",
    )
    .unwrap();
    fs::write(
        &input,
        r#"
import { "library" as lib }
fn initialize() int { @println("initialize");return lib.value() }
mut int value = initialize()
@println("before")
test "first" { assert value == 7;value = value + 1;@println(value) }
@println(value)
test "second" { assert value == 8;@println("second") }
@println("after")
"#,
    )
    .unwrap();
    let input = input.to_str().unwrap();
    let binary = binary.to_str().unwrap();
    for mode in ["-d", "-r"] {
        let output = cli(&["test", input, mode]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"imported\ninitialize\n8\nsecond\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        let output = cli(&["run", input, mode]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"initialize\nbefore\n7\nafter\n");
        assert!(cli(&["build", input, mode, "-o", binary]).status.success());
        let output = Command::new(binary).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"initialize\nbefore\n7\nafter\n");
        fs::remove_file(binary).unwrap();
    }
}

#[test]
fn test_failures_are_nonzero_and_leave_no_generated_files() {
    let dir = ncc::temp::Directory::new().unwrap();
    let input = dir.path().join("main.nc");
    let path = input.to_str().unwrap();
    for body in ["assert false", "throw \"test failure\"", "int value = true"] {
        fs::write(
            &input,
            format!("test \"failure\" {{ {body} }};@println(\"after\")"),
        )
        .unwrap();
        for mode in ["-d", "-r"] {
            let output = cli(&["test", path, mode]);
            assert!(!output.status.success(), "{body}, {mode}");
            assert_ne!(output.stderr.as_slice(), b"");
            assert_eq!(output.stdout.as_slice(), b"");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            let output = cli(&["run", path, mode]);
            assert!(output.status.success());
            assert_eq!(output.stdout, b"after\n");
        }
    }
    fs::write(&input, "test \"args\" { assert @args()[1] == \"--help\" }").unwrap();
    assert!(cli(&["test", path, "--", "--help"]).status.success());
    fs::write(&input, "@println(\"no tests\")").unwrap();
    let output = cli(&["test", path]);
    assert!(output.status.success());
    assert_eq!(output.stdout.as_slice(), b"");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
