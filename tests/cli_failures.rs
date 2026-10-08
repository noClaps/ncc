use std::{fs, path::Path, process::Command, process::Output};

struct Fixture {
    directory: ncc::temp::Directory,
    source: String,
}

impl Fixture {
    fn new(source: &str) -> Self {
        let directory = ncc::temp::Directory::new().unwrap();
        fs::create_dir(directory.path().join("temporary")).unwrap();
        fs::write(directory.path().join("input.nc"), source).unwrap();
        fs::write(directory.path().join("keep"), b"unrelated file\0").unwrap();
        Self {
            directory,
            source: source.into(),
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn command(&self, command: &str, mode: &str) -> Command {
        let mut process = Command::new(env!("CARGO_BIN_EXE_ncc"));
        process
            .current_dir(self.path())
            .env("TMPDIR", self.path().join("temporary"))
            .env("NC_TARGET", "macos-arm64")
            .env_remove("NC_CLEANUP_MISSING_KEY")
            .args([command, "input.nc", mode]);
        process
    }

    fn assert_clean(&self, additional: &[&str]) {
        let mut expected = vec!["input.nc", "keep", "temporary"];
        expected.extend_from_slice(additional);
        assert_entries(self.path(), &expected);
        assert_entries(&self.path().join("temporary"), &[]);
        assert_eq!(
            fs::read(self.path().join("keep")).unwrap(),
            b"unrelated file\0"
        );
        assert_eq!(
            fs::read_to_string(self.path().join("input.nc")).unwrap(),
            self.source
        );
    }
}

fn assert_entries(path: &Path, expected: &[&str]) {
    let mut entries: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(entries, expected, "{}", path.display());
}

fn assert_failure(output: &Output, expected: &str) {
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "expected {expected:?}: {output:?}"
    );
}

fn assert_backend_failure(fixture: &Fixture, mode: &str, expected: &str, files: &[&str]) {
    for format in ["obj", "exe"] {
        // Object emission does not resolve external symbols; link failures use exe only.
        if expected == "nc_cleanup_missing_symbol" && format == "obj" {
            continue;
        }
        for existing in [false, true] {
            let artifact = fixture.path().join("requested.output");
            if existing {
                fs::write(&artifact, b"previous output\0").unwrap();
            }
            let output = fixture
                .command("build", mode)
                .args(["-f", format, "-o", "requested.output"])
                .output()
                .unwrap();
            assert_failure(&output, "ncc: C compiler failed");
            assert_failure(&output, expected);
            let mut retained = files.to_vec();
            if existing {
                retained.push("requested.output");
                assert_eq!(fs::read(&artifact).unwrap(), b"previous output\0");
            }
            fixture.assert_clean(&retained);
            if existing {
                fs::remove_file(artifact).unwrap();
            }
        }
    }
    for command in ["run", "test"] {
        let output = fixture.command(command, mode).output().unwrap();
        assert_failure(&output, "ncc: C compiler failed");
        assert_failure(&output, expected);
        fixture.assert_clean(files);
    }
}

#[test]
fn backend_compile_and_link_failures_preserve_outputs_and_remove_intermediates() {
    let source = r#"
extern "native.c" as native { fn invoke() = "nc_cleanup_missing_symbol" }
native.invoke()
test "backend" { native.invoke() }
"#;
    for (native, diagnostic) in [
        (
            "#error NC_CLEANUP_COMPILE_FAILURE\n",
            "NC_CLEANUP_COMPILE_FAILURE",
        ),
        (
            "/* Deliberately missing the external implementation. */\n",
            "nc_cleanup_missing_symbol",
        ),
    ] {
        let fixture = Fixture::new(source);
        fs::write(fixture.path().join("native.c"), native).unwrap();
        for mode in ["-d", "-r"] {
            assert_backend_failure(&fixture, mode, diagnostic, &["native.c"]);
            assert_eq!(
                fs::read_to_string(fixture.path().join("native.c")).unwrap(),
                native
            );
        }
    }
}

#[test]
fn missing_backend_compiler_cleans_generated_source_without_replacing_output() {
    let fixture = Fixture::new("@println(42)\ntest \"backend\" { assert true }\n");
    fs::create_dir(fixture.path().join("no-tools")).unwrap();
    for mode in ["-d", "-r"] {
        for command in ["build", "run", "test"] {
            let artifact = fixture.path().join("requested.output");
            fs::write(&artifact, b"previous output\0").unwrap();
            let mut process = fixture.command(command, mode);
            process.env("PATH", fixture.path().join("no-tools"));
            if command == "build" {
                process.args(["-o", "requested.output"]);
            }
            assert_failure(&process.output().unwrap(), "ncc: cannot run C compiler:");
            fixture.assert_clean(&["no-tools", "requested.output"]);
            assert_eq!(fs::read(&artifact).unwrap(), b"previous output\0");
            fs::remove_file(artifact).unwrap();
        }
    }
}

#[test]
fn failed_artifact_publication_cleans_backend_and_staging_directories() {
    let fixture = Fixture::new("@println(42)\n");
    fs::create_dir(fixture.path().join("requested.output")).unwrap();
    fs::write(
        fixture.path().join("requested.output/keep"),
        b"directory contents",
    )
    .unwrap();
    for mode in ["-d", "-r"] {
        for format in ["obj", "exe"] {
            let output = fixture
                .command("build", mode)
                .args(["-f", format, "-o", "requested.output"])
                .output()
                .unwrap();
            assert_failure(&output, "requested.output:");
            fixture.assert_clean(&["requested.output"]);
            assert_entries(&fixture.path().join("requested.output"), &["keep"]);
            assert_eq!(
                fs::read(fixture.path().join("requested.output/keep")).unwrap(),
                b"directory contents"
            );
        }
    }
}

#[test]
fn failing_assertions_and_uncaught_test_errors_remove_temporary_files() {
    for (body, diagnostic) in [
        ("assert @args().len == 0u", "assertion failed"),
        ("throw \"NC_CLEANUP_TEST_ERROR\"", "NC_CLEANUP_TEST_ERROR"),
    ] {
        let source = format!(
            "@eprintln(\"discarded\")\n\
             test \"failure\" {{ @eprintln(\"before\");{body};@eprintln(\"after\") }}\n\
             test \"unreached\" {{ @eprintln(\"unreached\") }}\n"
        );
        let fixture = Fixture::new(&source);
        for mode in ["-d", "-r"] {
            let output = fixture.command("test", mode).output().unwrap();
            assert_failure(&output, diagnostic);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = if diagnostic == "assertion failed" {
                format!("panic: {diagnostic}")
            } else {
                diagnostic.into()
            };
            // Located unreachable-code warnings may quote the skipped output literals.
            assert!(
                stderr.ends_with(&format!("before\n{message}\n")),
                "{output:?}"
            );
            for skipped in ["discarded", "after", "unreached"] {
                assert!(!stderr.lines().any(|line| line == skipped), "{output:?}");
            }
            fixture.assert_clean(&[]);
        }
    }
}

#[test]
fn runtime_failures_leave_only_requested_build_output_and_no_run_test_artifacts() {
    for (body, diagnostic) in [
        ("_ = @args()[@args().len]", "array index out of bounds"),
        (
            "_ = @env()[\"NC_CLEANUP_MISSING_KEY\"]",
            "map key not found",
        ),
        (
            "uint zero = @args().len - @args().len;_ = 1u / zero",
            "division by zero",
        ),
    ] {
        let source = format!(
            "fn fail() {{ @eprintln(\"before\");{body};@eprintln(\"after\") }}\n\
             fail()\ntest \"runtime failure\" {{ fail() }}\n"
        );
        let fixture = Fixture::new(&source);
        for mode in ["-d", "-r"] {
            for command in ["run", "test"] {
                let output = fixture.command(command, mode).output().unwrap();
                assert_failure(&output, diagnostic);
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr),
                    format!("before\npanic: {diagnostic}\n")
                );
                fixture.assert_clean(&[]);
            }
            let build = fixture
                .command("build", mode)
                .args(["-o", "requested.output"])
                .output()
                .unwrap();
            assert!(build.status.success(), "{build:?}");
            assert!(build.stdout.is_empty(), "{build:?}");
            fixture.assert_clean(&["requested.output"]);
            let output = Command::new(fixture.path().join("requested.output"))
                .current_dir(fixture.path())
                .env("TMPDIR", fixture.path().join("temporary"))
                .env_remove("NC_CLEANUP_MISSING_KEY")
                .output()
                .unwrap();
            assert_failure(&output, diagnostic);
            assert_eq!(
                String::from_utf8_lossy(&output.stderr),
                format!("before\npanic: {diagnostic}\n")
            );
            fixture.assert_clean(&["requested.output"]);
            fs::remove_file(fixture.path().join("requested.output")).unwrap();
        }
    }
}
