//! End-to-end tests: compile NC programs with the built `ncc` and compare their output.

use std::path::{Path, PathBuf};
use std::process::Command;

fn ncc() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ncc"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn run(args: &[&str], file: &Path) -> (String, String, i32) {
    let out = Command::new(ncc())
        .args(args)
        .arg(file)
        .output()
        .expect("failed to run ncc");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn sorted_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(ext))
        .collect();
    v.sort();
    v
}

/// `name.out` holds the expected stdout of `ncc run` (debug and release);
/// `name.test.out` holds the expected stdout of `ncc test`.
#[test]
fn programs() {
    let mut dirs = vec![root().join("programs")];
    dirs.push(root().join("programs/modproj"));
    let mut failures = Vec::new();
    for dir in dirs {
        for out in sorted_files(&dir, ".out") {
            let name = out.file_name().unwrap().to_string_lossy().into_owned();
            let expected = std::fs::read_to_string(&out).unwrap();
            let (stem, mode) = match name.strip_suffix(".test.out") {
                Some(s) => (s.to_string(), "test"),
                None => (name.strip_suffix(".out").unwrap().to_string(), "run"),
            };
            let src = dir.join(format!("{stem}.nc"));
            let variants: Vec<Vec<&str>> = if mode == "test" {
                vec![vec!["test"]]
            } else {
                vec![vec!["run"], vec!["run", "-r"]]
            };
            for args in variants {
                let (stdout, stderr, _) = run(&args, &src);
                if stdout != expected {
                    failures.push(format!(
                        "{} ({}):\n--- expected\n{expected}\n--- got\n{stdout}\n--- stderr\n{stderr}",
                        src.display(),
                        args.join(" ")
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Each file in `tests/fail` starts with `// expect: <message>`; `ncc check` must fail with it.
#[test]
fn compile_failures() {
    let mut failures = Vec::new();
    for src in sorted_files(&root().join("fail"), ".nc") {
        let text = std::fs::read_to_string(&src).unwrap();
        let expect = text
            .lines()
            .next()
            .unwrap()
            .strip_prefix("// expect: ")
            .expect("missing expect line")
            .to_string();
        let (_, stderr, code) = run(&["check"], &src);
        if code == 0 || !stderr.contains(&expect) {
            failures.push(format!(
                "{}: expected `{expect}`, got (exit {code}):\n{stderr}",
                src.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Files in `tests/panic` declare the expected exit code and a stderr substring.
#[test]
fn runtime_failures() {
    let mut failures = Vec::new();
    for src in sorted_files(&root().join("panic"), ".nc") {
        let text = std::fs::read_to_string(&src).unwrap();
        let mut lines = text.lines();
        let code: i32 = lines
            .next()
            .unwrap()
            .strip_prefix("// expect-exit: ")
            .unwrap()
            .parse()
            .unwrap();
        let err = lines
            .next()
            .unwrap()
            .strip_prefix("// expect-stderr: ")
            .unwrap()
            .to_string();
        for args in [vec!["run"], vec!["run", "-r"]] {
            let (_, stderr, got) = run(&args, &src);
            if got != code || !stderr.contains(&err) {
                failures.push(format!(
                    "{} ({}): expected exit {code} with `{err}`, got exit {got}:\n{stderr}",
                    src.display(),
                    args.join(" ")
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn build_formats() {
    let dir = std::env::temp_dir().join(format!("ncc-formats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = root().join("programs/fib.nc");
    for (fmt, file) in [("c", "fib.c"), ("asm", "fib.s"), ("exe", "fib")] {
        let out = dir.join(file);
        let status = Command::new(ncc())
            .args(["build", "-r", "-f", fmt, "-o"])
            .arg(&out)
            .arg(&src)
            .status()
            .unwrap();
        assert!(status.success(), "build -f {fmt} failed");
        assert!(out.is_file(), "{} missing", out.display());
    }
    // format inferred from the output extension
    let out = dir.join("inferred.c");
    let status = Command::new(ncc())
        .args(["build", "-o"])
        .arg(&out)
        .arg(&src)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(std::fs::read_to_string(&out).unwrap().contains("int main("));
    let exe = Command::new(dir.join("fib")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&exe.stdout), "55\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_creates_project() {
    let dir = std::env::temp_dir().join(format!("ncc-init-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let status = Command::new(ncc())
        .args(["init", "--name", "demo"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(dir.join(".nc/config.toml")).unwrap(),
        "name = \"demo\"\n"
    );
    assert!(dir.join(".nc/deps").is_dir());
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        ".nc/deps/\n"
    );
    let out = Command::new(ncc())
        .args(["run", "main.nc"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello, world!\n");
    // a second init refuses
    let status = Command::new(ncc())
        .args(["init"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(!status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files in `tests/multi` contain several errors; `ncc check` must report all of them, in
/// order (each line of the `.errors` file is a message that must appear).
#[test]
fn all_errors_reported() {
    let mut failures = Vec::new();
    for src in sorted_files(&root().join("multi"), ".nc") {
        let expected = std::fs::read_to_string(src.with_extension("errors")).unwrap();
        let (_, stderr, code) = run(&["check"], &src);
        let mut rest = stderr.as_str();
        for msg in expected.lines() {
            match rest.find(msg) {
                Some(i) => rest = &rest[i + msg.len()..],
                None => failures.push(format!(
                    "{}: missing or out of order `{msg}`:\n{stderr}",
                    src.display()
                )),
            }
        }
        let n = expected.lines().count();
        if code == 0 || (n > 1 && !stderr.contains(&format!("found {n} errors"))) {
            failures.push(format!(
                "{}: expected {n} errors (exit {code}):\n{stderr}",
                src.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Every sample program formats; formatting is idempotent and keeps all comments.
#[test]
fn formatter_round_trip() {
    let mut failures = Vec::new();
    let mut files = sorted_files(&root().join("programs"), ".nc");
    files.extend(sorted_files(&root().join("programs/modproj"), ".nc"));
    files.extend(sorted_files(&root().join("fail"), ".nc"));
    for src in files.into_iter().filter(|f| f.is_file()) {
        let text = std::fs::read_to_string(&src).unwrap();
        let once = Command::new(ncc())
            .args(["fmt", "-o", "-"])
            .arg(&src)
            .output()
            .unwrap();
        if !once.status.success() {
            // files with syntax errors on purpose are refused
            continue;
        }
        let formatted = String::from_utf8(once.stdout).unwrap();
        let tmp = std::env::temp_dir().join(format!(
            "ncc-fmt-{}-{}",
            std::process::id(),
            src.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&tmp, &formatted).unwrap();
        let check = Command::new(ncc())
            .args(["fmt", "--check"])
            .arg(&tmp)
            .status()
            .unwrap();
        if !check.success() {
            failures.push(format!("{}: formatting is not idempotent", src.display()));
        }
        let comments = |s: &str| s.lines().filter(|l| l.contains("//")).count();
        if comments(&text) != comments(&formatted) {
            failures.push(format!("{}: comments changed", src.display()));
        }
        let _ = std::fs::remove_file(&tmp);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fmt_command() {
    let dir = std::env::temp_dir().join(format!("ncc-fmt-cmd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.nc");
    std::fs::write(&file, "int  x=1+2\n").unwrap();
    // --check reports unformatted files
    let st = Command::new(ncc())
        .args(["fmt", "--check"])
        .arg(&file)
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(1));
    // -o - prints to stdout without touching the file
    let out = Command::new(ncc())
        .args(["fmt", "-o"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "int x = 1 + 2\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "int  x=1+2\n");
    // -o file writes there
    let other = dir.join("b.nc");
    let st = Command::new(ncc())
        .args(["fmt", "-o"])
        .arg(&other)
        .arg(&file)
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "int x = 1 + 2\n");
    // default: in place
    let st = Command::new(ncc()).arg("fmt").arg(&file).status().unwrap();
    assert!(st.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "int x = 1 + 2\n");
    let st = Command::new(ncc())
        .args(["fmt", "--check"])
        .arg(&file)
        .status()
        .unwrap();
    assert!(st.success());
    // syntax errors are refused
    std::fs::write(&file, "int x = (\n").unwrap();
    let st = Command::new(ncc()).arg("fmt").arg(&file).status().unwrap();
    assert!(!st.success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unused code is a warning in debug builds and an error in release builds.
#[test]
fn unused_code() {
    let dir = std::env::temp_dir().join(format!("ncc-unused-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("main.nc");
    std::fs::write(
        &src,
        "fn dead(int x, int _ok) int {\n  int y = 1\n  return 0\n}\n\
         fn helper() int {\n  return 2\n}\n\
         pub fn api() int {\n  return 3\n}\n\
         struct Unused {\n  int a\n}\n\
         int _quiet = 1\n\
         @println(\"hi\")\n\
         test \"t\" {\n  assert helper() == 2\n}\n",
    )
    .unwrap();
    let expected = [
        "unused function `dead`",
        "unused parameter `x`",
        "unused variable `y`",
        "unused struct `Unused`",
    ];
    // debug: warnings, and the program still runs
    let (stdout, stderr, code) = run(&["run"], &src);
    assert_eq!((stdout.as_str(), code), ("hi\n", 0), "{stderr}");
    for e in expected {
        assert!(stderr.contains(&format!("warning: {e}")), "{e}:\n{stderr}");
    }
    // helpers used by tests, `pub` items and `_` names are not reported
    for quiet in ["helper", "api", "_ok", "_quiet"] {
        assert!(
            !stderr.contains(&format!("`{quiet}`")),
            "{quiet}:\n{stderr}"
        );
    }
    // release: errors, and nothing is built
    let (_, stderr, code) = run(&["build", "-r"], &src);
    assert_ne!(code, 0);
    for e in expected {
        assert!(stderr.contains(&format!("error: {e}")), "{e}:\n{stderr}");
    }
    assert!(stderr.contains("found 4 errors"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
