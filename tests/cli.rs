use std::{fmt::Write as _, fs, process::Command};
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

fn build_process_fixture(file: &std::path::Path, binary: &std::path::Path, release: bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
        .args([
            "build",
            file.to_str().unwrap(),
            "-o",
            binary.to_str().unwrap(),
            if release { "-r" } else { "-d" },
        ])
        .env("NC_TARGET", "macos-arm64")
        .env("NC_TEST_PRESENT", "compile-time only")
        .env("NC_TEST_ABSENT", "compile-time only")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn process_args_preserve_executable_empty_unicode_and_independent_copies() {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("arguments.nc");
    let binary = dir.path().join("program with spaces");
    fs::write(
        &file,
        r#"
mut str[] arguments = @args()
str[] snapshot = arguments
@println(arguments[0])
@println(arguments.len)
for index in arguments {
    if { index > 0 -> { @print("<", arguments[index], ">") } _ -> {} }
}
@println()
arguments[0] = "changed executable"
arguments = arguments <> ["extra"]
str[] fresh = @args()
@println(snapshot == fresh)
@println(arguments.len == fresh.len + 1)
@println(arguments[0] == "changed executable")
"#,
    )
    .unwrap();
    for release in [false, true] {
        build_process_fixture(&file, &binary, release);
        for arguments in [vec![], vec!["", "one two", "--help", "a=b", "e\u{301}🙂"]] {
            let output = Command::new(&binary).args(&arguments).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stderr, b"");
            let mut values = String::new();
            for value in &arguments {
                write!(values, "<{value}>").unwrap();
            }
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!(
                    "{}\n{}\n{values}\ntrue\ntrue\ntrue\n",
                    binary.display(),
                    arguments.len() + 1
                )
            );
        }
    }
}

#[test]
fn process_env_preserves_empty_absent_unicode_values_and_independent_copies() {
    let dir = ncc::temp::Directory::new().unwrap();
    let file = dir.path().join("environment.nc");
    let binary = dir.path().join("environment");
    fs::write(
        &file,
        r#"
mut [str]str environment = @env()
[str]str snapshot = environment
@println(environment.len == 3)
@println("NC_TEST_PRESENT" in environment)
@println("NC_TEST_EMPTY" in environment)
@println(not ("NC_TEST_ABSENT" in environment))
@println(environment["NC_TEST_EMPTY"] == "")
@println(environment["NC_TEST_PRESENT"] == "runtime=✓=e\u{301}🙂")
@println(environment["NC_TEST_MULTILINE"] == "first\nsecond\tend")
environment["NC_TEST_PRESENT"] = "changed"
environment["NC_TEST_EMPTY"] = "no longer empty"
environment["NC_TEST_INSERTED"] = "local only"
[str]str fresh = @env()
@println(snapshot == fresh)
@println(not ("NC_TEST_INSERTED" in fresh))
@println(environment.len == fresh.len + 1)
@println(environment["NC_TEST_PRESENT"] == "changed")
"#,
    )
    .unwrap();
    for release in [false, true] {
        build_process_fixture(&file, &binary, release);
        let output = Command::new(&binary)
            .env_clear()
            .env("NC_TEST_PRESENT", "runtime=✓=e\u{301}🙂")
            .env("NC_TEST_EMPTY", "")
            .env("NC_TEST_MULTILINE", "first\nsecond\tend")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stderr, b"");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "true\n".repeat(11)
        );
    }
}

fn assert_directory_entries(directory: &std::path::Path, expected: &[&str]) {
    let mut actual: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    actual.sort();
    let mut expected: Vec<_> = expected.iter().map(std::ffi::OsString::from).collect();
    expected.sort();
    assert_eq!(actual, expected);
}

fn assert_successful_output(output: &std::process::Output, stdout: &[u8]) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, stdout);
    assert_eq!(output.stderr, b"");
}

#[derive(Clone, Copy, Debug)]
enum BuildArtifact {
    C,
    Object,
    Executable,
}

fn assert_build_artifact(path: &std::path::Path, artifact: BuildArtifact) {
    match artifact {
        BuildArtifact::C => {
            let source = fs::read_to_string(path).unwrap();
            assert!(source.contains("Generated by ncc"));
            assert!(source.contains("int main("));
            let directory = ncc::temp::Directory::new().unwrap();
            let binary = directory.path().join("from-c");
            let output = Command::new("cc")
                .args(["-x", "c", "-std=c11"])
                .arg(path)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert_successful_output(&output, b"");
            assert_successful_output(&Command::new(binary).output().unwrap(), b"artifact:42\n");
        }
        BuildArtifact::Object => {
            let bytes = fs::read(path).unwrap();
            // A 64-bit little-endian Mach-O header with MH_OBJECT file type.
            assert!(bytes.len() >= 32);
            assert_eq!(&bytes[..4], &[0xcf, 0xfa, 0xed, 0xfe]);
            assert_eq!(&bytes[12..16], &[1, 0, 0, 0]);
            let directory = ncc::temp::Directory::new().unwrap();
            let binary = directory.path().join("from-object");
            // Explicit obj output may have a .c or arbitrary extension; link as an object.
            let object = directory.path().join("program.o");
            fs::copy(path, &object).unwrap();
            let output = Command::new("cc")
                .arg(object)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert_successful_output(&output, b"");
            assert_successful_output(&Command::new(binary).output().unwrap(), b"artifact:42\n");
        }
        BuildArtifact::Executable => {
            assert_successful_output(&Command::new(path).output().unwrap(), b"artifact:42\n");
        }
    }
}

fn assert_build_case(mode: &str, format: Option<&str>, output: Option<&str>, kind: BuildArtifact) {
    let directory = ncc::temp::Directory::new().unwrap();
    let source = "@println(\"artifact:\", 6 * 7)\n";
    fs::write(directory.path().join("input.nc"), source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command
        .current_dir(directory.path())
        .env("NC_TARGET", "macos-arm64")
        .args(["build", "input.nc", mode]);
    if let Some(format) = format {
        command.args(["--format", format]);
    }
    if let Some(output) = output {
        command.args(["--output", output]);
    }
    let result = command.output().unwrap();
    assert_successful_output(&result, b"");
    let artifact = output.unwrap_or(match kind {
        BuildArtifact::C => "input.c",
        BuildArtifact::Object => "input.o",
        BuildArtifact::Executable => "input",
    });
    assert_directory_entries(directory.path(), &["input.nc", artifact]);
    assert_eq!(
        fs::read_to_string(directory.path().join("input.nc")).unwrap(),
        source
    );
    assert_build_artifact(&directory.path().join(artifact), kind);
    assert_directory_entries(directory.path(), &["input.nc", artifact]);
}

#[test]
fn build_inferred_explicit_and_default_output_artifact_matrix_in_both_modes() {
    let outputs = ["requested.c", "requested.o", "requested", "requested.other"];
    for mode in ["-d", "-r"] {
        for (output, artifact) in outputs.into_iter().zip([
            BuildArtifact::C,
            BuildArtifact::Object,
            BuildArtifact::Executable,
            BuildArtifact::Executable,
        ]) {
            assert_build_case(mode, None, Some(output), artifact);
        }
        assert_build_case(mode, None, None, BuildArtifact::Executable);
        for (format, artifact) in [
            ("C", BuildArtifact::C),
            ("obj", BuildArtifact::Object),
            ("exe", BuildArtifact::Executable),
        ] {
            for output in outputs {
                assert_build_case(mode, Some(format), Some(output), artifact);
            }
            assert_build_case(mode, Some(format), None, artifact);
        }
    }
}

#[test]
fn run_and_test_forward_arguments_without_artifacts_in_both_modes() {
    let directory = ncc::temp::Directory::new().unwrap();
    let file = directory.path().join("arguments with spaces.nc");
    let source = r#"
fn emit(str prefix) {
    str[] arguments = @args()
    @println(prefix, ":", arguments.len)
    for index in arguments {
        if { index > 0 -> { @print("<", arguments[index], ">") } _ -> {} }
    }
    @println()
}
emit("run")
test "forwarded arguments" { emit("test") }
"#;
    fs::write(&file, source).unwrap();
    for mode in ["-d", "-r"] {
        for command in ["run", "test"] {
            for arguments in [
                None,
                Some(vec![]),
                Some(vec![
                    "",
                    "one two",
                    "--help",
                    "--release",
                    "-o",
                    "a=b",
                    "--",
                    "e\u{301}🙂",
                ]),
            ] {
                let mut invocation = Command::new(env!("CARGO_BIN_EXE_ncc"));
                invocation
                    .current_dir(directory.path())
                    .arg(command)
                    .arg(&file)
                    .arg(mode);
                let mut values = String::new();
                let count = arguments.as_ref().map_or(0, Vec::len);
                if let Some(arguments) = arguments {
                    invocation.arg("--").args(&arguments);
                    for value in arguments {
                        write!(values, "<{value}>").unwrap();
                    }
                }
                let expected = format!("{command}:{}\n{values}\n", count + 1);
                assert_successful_output(&invocation.output().unwrap(), expected.as_bytes());
                assert_directory_entries(directory.path(), &["arguments with spaces.nc"]);
                assert_eq!(fs::read_to_string(&file).unwrap(), source);
            }
        }
    }
}

#[test]
fn test_without_test_blocks_succeeds_silently_in_both_modes() {
    let directory = ncc::temp::Directory::new().unwrap();
    let file = directory.path().join("no-tests.nc");
    for source in [
        "",
        "@println(\"discarded stdout\");@eprintln(\"discarded stderr\")\n",
    ] {
        fs::write(&file, source).unwrap();
        for mode in ["-d", "-r"] {
            for arguments in [vec![], vec!["--"], vec!["--", "", "--help", "one two", "✓"]] {
                let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
                    .current_dir(directory.path())
                    .arg("test")
                    .arg(&file)
                    .arg(mode)
                    .args(arguments)
                    .output()
                    .unwrap();
                assert_successful_output(&output, b"");
                assert_directory_entries(directory.path(), &["no-tests.nc"]);
                assert_eq!(fs::read_to_string(&file).unwrap(), source);
            }
        }
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
