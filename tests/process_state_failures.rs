use std::{fs, path::Path, process::Command, process::Output};

#[test]
fn args_negative_and_length_indices_panic_at_runtime() {
    for index in ["-@as(int, arguments.len)", "arguments.len"] {
        let source = format!(
            "str[] arguments = @args()\n\
             @eprintln(\"before\")\n\
             _ = arguments[{index}]\n\
             @eprintln(\"after\")\n"
        );
        with_program(&source, |binary, release| {
            for arguments in [&[][..], &["first", "second"][..]] {
                let output = runtime(binary).args(arguments).output().unwrap();
                assert_runtime_panic(&output, "array index out of bounds", release);
            }
        });
    }
}

#[test]
fn env_absent_reads_panic_but_present_empty_reads_succeed() {
    for lookup in ["environment[\"NC_PROCESS_LOOKUP\"]", "@env()[@args()[1]]"] {
        let source = format!(
            "[str]str environment = @env()\n\
             @eprintln(\"before\")\n\
             str value = {lookup}\n\
             @eprintln(value == \"\")\n\
             @eprintln(\"after\")\n"
        );
        with_program(&source, |binary, release| {
            let absent = runtime(binary).arg("NC_PROCESS_LOOKUP").output().unwrap();
            assert_runtime_panic(&absent, "map key not found", release);

            let empty = runtime(binary)
                .arg("NC_PROCESS_LOOKUP")
                .env("NC_PROCESS_LOOKUP", "")
                .output()
                .unwrap();
            assert!(empty.status.success(), "release={release}: {empty:?}");
            assert!(empty.stdout.is_empty(), "{empty:?}");
            assert_eq!(empty.stderr, b"before\ntrue\nafter\n");

            let present = runtime(binary)
                .arg("NC_PROCESS_LOOKUP")
                .env("NC_PROCESS_LOOKUP", "runtime value")
                .output()
                .unwrap();
            assert!(present.status.success(), "release={release}: {present:?}");
            assert!(present.stdout.is_empty(), "{present:?}");
            assert_eq!(present.stderr, b"before\nfalse\nafter\n");
        });
    }
}

#[test]
fn process_builtin_index_and_element_type_errors_keep_original_diagnostics() {
    for (source, failing, message) in [
        (
            "// invalid argument index\n_ = @args()[true]",
            "@args()[true]",
            "array index must be int or uint",
        ),
        (
            "// invalid environment key\n_ = @env()[1]",
            "1",
            "expected `str`, found `int`",
        ),
        (
            "// target has only OS and architecture\n_ = @target()[2]",
            "@target()[2]",
            "tuple index out of bounds",
        ),
        (
            "// OS is a string\nint os = @target()[0]",
            "@target()[0]",
            "expected `int`, found `str`",
        ),
        (
            "// architecture is a string\nint arch = @target()[1]",
            "@target()[1]",
            "expected `int`, found `str`",
        ),
    ] {
        let path = Path::new("process_state_failures.nc");
        for release in [false, true] {
            let error = ncc::compile_source_with_options(source, path, release)
                .expect_err(&format!("release={release}: {source}"));
            assert_eq!(error.0.len(), 1, "release={release}: {error}");
            let diagnostic = &error.0[0];
            assert_eq!(diagnostic.message, message, "release={release}: {error}");
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
            assert_eq!(&source[diagnostic.span.clone()], failing, "{error}");
            let column = diagnostic.span.start - source.find('\n').unwrap();
            assert_eq!(
                error.render(source, path),
                format!(
                    "{}:2:{column}: error: {message}\n  |\n 2 | {}\n  | {}^\n",
                    path.display(),
                    source.lines().nth(1).unwrap(),
                    " ".repeat(column - 1),
                ),
                "release={release}",
            );
        }
    }
}

fn with_program(source: &str, action: impl Fn(&Path, bool)) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("process_state_failures.nc");
    let binary = directory.path().join("process_state_failures");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("build")
            .arg(&input)
            .arg("-o")
            .arg(&binary)
            .arg(if release { "-r" } else { "-d" })
            .args(["--target", "macos-arm64"])
            // Runtime absence must not be replaced by the compiler's environment.
            .env("NC_PROCESS_LOOKUP", "compile-time value")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        action(&binary, release);
    }
}

fn runtime(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command.env_clear();
    command
}

fn assert_runtime_panic(output: &Output, detail: &str, release: bool) {
    assert!(!output.status.success(), "release={release}: {output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("before\npanic: {detail}\n"),
        "release={release}",
    );
}
