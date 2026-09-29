use std::{fs, path::Path, process::Command};

fn warnings(source: &str) -> Vec<String> {
    let mut modes = Vec::new();
    for release in [false, true] {
        let output =
            ncc::compile_source_with_diagnostics(source, Path::new("race.nc"), release).unwrap();
        modes.push(
            output
                .warnings
                .0
                .into_iter()
                .map(|d| d.message)
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(modes[0], modes[1]);
    modes.remove(0)
}

#[test]
fn shared_state_warns_through_named_functions_aliases_and_closures() {
    for source in [
        "mut int count = 0 fn increment() { count = count + 1 } fut void a = async increment() fut void b = async increment() await a await b",
        "mut int count = 0 fn increment() { count = count + 1 } fn forward() { increment() } fut void a = async forward() await a",
        "mut int count = 0 fn increment() { count = count + 1 } (fn() void) alias = increment fut void a = async alias() await a",
        "test \"local\" { mut int count = 0 fn increment = fn() { count = count + 1 } fut void a = async increment() await a }",
        "fn make() (fn() void) { mut int count = 0 return fn() { count = count + 1 } } (fn() void) f = make() fut void a = async f() await a",
        "mut int count = 0 fn write() { count = 2 } fut void a = async write() await a",
        "mut int count = 0 fn read() int { return count } fut int a = async read() _ = await a",
    ] {
        let messages = warnings(source);
        assert!(!messages.is_empty(), "{source}");
        assert!(messages.iter().all(|m| m.contains("potential data race")));
    }
}

#[test]
fn mutexes_immutable_captures_and_private_locals_do_not_warn() {
    for source in [
        "mutex int count = 0 fn increment() { lock count { count = count + 1 } } fut void a = async increment() await a",
        "test \"mutex\" { mutex int count = 0 fn increment() { lock count { count = count + 1 } } fut void a = async increment() await a }",
        "fn private() int { mut int count = 0 count = count + 1 return count } fut int a = async private() _ = await a",
        "test \"copy\" { int count = 1 fn read() int { return count } fut int a = async read() _ = await a }",
        "mut int count = 0 fn private() int { mut int count = 1 count = count + 1 return count } fut int a = async private() _ = await a",
        "mut int count = 0 fn increment() { count = count + 1 } increment()",
        "mut int count = 1 fut void a = async @println(count) await a",
    ] {
        assert!(warnings(source).is_empty(), "{source}");
    }
}

#[test]
fn cli_emits_located_warnings_and_still_builds_and_runs() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("race.nc");
    // A single worker and an await make the result deterministic. The warning
    // concerns potential sharing, not a claim that overlap occurs on this run.
    fs::write(&path, "mut int count = 0\nfn increment() { count = count + 1 }\nfut void a = async increment()\nawait a\n@println(count)\n").unwrap();
    for release in [false, true] {
        for command in ["build", "run"] {
            let mut cli = Command::new(env!("CARGO_BIN_EXE_ncc"));
            cli.arg(command).arg(&path);
            if release {
                cli.arg("--release");
            }
            if command == "build" {
                cli.args(["--format", "C", "-o"])
                    .arg(directory.path().join("race.c"));
            }
            let output = cli.output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
            assert!(stderr.contains("race.nc:3:"), "{stderr}");
            assert!(stderr.contains("potential data race"), "{stderr}");
            if command == "run" {
                assert_eq!(output.stdout, b"1\n");
            }
        }
    }
}

#[test]
fn imported_specializations_keep_original_warning_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let library = directory.path().join("worker.nc");
    let library_source = "mut int count = 0\nfn increment() { count = count + 1 }\npub fn launch<T>(T value) {\n    fut void job = async increment()\n    await job\n}\n";
    fs::write(&library, library_source).unwrap();
    let main = directory.path().join("main.nc");
    let source = "import { \"worker\" as worker } worker.launch<int>(1) worker.launch<str>(\"x\")";
    for release in [false, true] {
        let output = ncc::compile_source_with_diagnostics(source, &main, release).unwrap();
        assert_eq!(output.warnings.0.len(), 1);
        let diagnostic = &output.warnings.0[0];
        assert_eq!(diagnostic.path.as_deref(), Some(library.as_path()));
        assert!(library_source[diagnostic.span.clone()].starts_with("async"));
        let rendered = output.warnings.render_warnings(source, &main);
        assert!(rendered.contains("worker.nc:4:"), "{rendered}");
        assert!(
            rendered.contains("warning: potential data race"),
            "{rendered}"
        );
    }
}

#[test]
fn recursive_and_indirect_async_calls_are_conservative_and_terminate() {
    assert!(!warnings("mut int count = 0 fn a(int n) { if n > 0 { true -> { b(n - 1) } false -> { count = count + 1 } } } fn b(int n) { a(n) } fut void job = async b(2) await job").is_empty());
    assert!(warnings("fn a(int n) { if n > 0 { true -> { b(n - 1) } false -> {} } } fn b(int n) { a(n) } fut void job = async b(2) await job").is_empty());
    assert!(!warnings("fn apply((fn() void) f) { f() } mut int count = 0 fn increment() { count = count + 1 } fut void job = async apply(increment) await job").is_empty());
}
