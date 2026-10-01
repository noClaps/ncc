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
        "fn local() { mut int count = 0 fn increment = fn() { count = count + 1 } fut void a = async increment() await a }",
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
        "fn local() { mutex int count = 0 fn increment() { lock count { count = count + 1 } } fut void a = async increment() await a }",
        "fn private() int { mut int count = 0 count = count + 1 return count } fut int a = async private() _ = await a",
        "fn local() { int count = 1 fn read() int { return count } fut int a = async read() _ = await a }",
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
    assert_ne!(warnings("mut int count = 0 fn a(int n) { if n > 0 { true -> { b(n - 1) } false -> { count = count + 1 } } } fn b(int n) { a(n) } fut void job = async b(2) await job").as_slice(), &[] as &[String]);
    assert_eq!(warnings("fn a(int n) { if n > 0 { true -> { b(n - 1) } false -> {} } } fn b(int n) { a(n) } fut void job = async b(2) await job").as_slice(), &[] as &[String]);
    assert_ne!(warnings("fn apply((fn() void) f) { f() } mut int count = 0 fn increment() { count = count + 1 } fut void job = async apply(increment) await job").as_slice(), &[] as &[String]);
}

#[test]
fn pattern_comparisons_count_as_shared_reads() {
    assert_ne!(warnings("mut int expected = 1 fn matches(int value) bool { return if value { expected -> { true } _ -> { false } } } fut bool job = async matches(1) _ = await job").as_slice(), &[] as &[String]);
    assert_ne!(warnings("fn local() { mut int expected = 1 fn matches(int value) bool { return if value { expected -> { true } _ -> { false } } } fut bool job = async matches(1) _ = await job }").as_slice(), &[] as &[String]);
    assert_eq!(warnings("mutex int expected = 1 fn matches(int value) bool { lock expected { return if value { expected -> { true } _ -> { false } } } } fut bool job = async matches(1) _ = await job").as_slice(), &[] as &[String]);
}

#[test]
fn infinite_loops_and_following_code_warn_without_execution() {
    for source in [
        "while true {} @println(1)",
        "fn spin() { while true {} @println(1) }",
        "fn spin = fn() { while not false { continue } @println(1) }",
        "fn spin() { outer: while true { while true { continue :outer } } @println(1) }",
        "fn spin() { while true { while true { break } } @println(1) }",
    ] {
        let messages = warnings(source);
        assert!(
            messages.iter().any(|m| m.contains("infinite loop")),
            "{source}: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m == "unreachable code"),
            "{source}: {messages:?}"
        );
    }
}

#[test]
fn reachable_exits_and_unknown_conditions_do_not_claim_infinite_loops() {
    for source in [
        "fn f() { while true { break } @println(1) }",
        "fn f(bool stop) { while true { if stop { true -> { break } false -> {} } } @println(1) }",
        "fn f(bool stop) { while stop {} @println(1) }",
        "fn f() { outer: while true { while true { break :outer } } @println(1) }",
        "fn f() { while true { return } }",
        "fn f() int ! { while true { throw \"done\" } }",
        "fn f() { while true { int unused = if true { true -> { return } false -> { 1 } } } }",
    ] {
        let messages = warnings(source);
        assert!(
            !messages.iter().any(|m| m.contains("infinite loop")),
            "{source}: {messages:?}"
        );
    }
}

#[test]
fn unreachable_exits_do_not_hide_infinite_loops() {
    for source in [
        "fn f() { while true { continue break } @println(1) }",
        "fn f() { while true { if false { true -> { break } false -> {} } } @println(1) }",
        "fn f() { while true { while false { break } } @println(1) }",
        "fn f() { while true or false { continue } @println(1) }",
        "fn f() { while true { fn local = fn() { return } } @println(1) }",
        "fn f() { while true { int value = if true { true -> { break 1 } false -> { break 2 } } } @println(1) }",
    ] {
        let messages = warnings(source);
        assert!(
            messages.iter().any(|m| m.contains("infinite loop")),
            "{source}: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m == "unreachable code"),
            "{source}: {messages:?}"
        );
    }
}

#[test]
fn abrupt_expression_exits_make_following_statements_unreachable() {
    for source in [
        "fn f() { int unused = if true { true -> { return } false -> { 1 } } @println(2) }",
        "fn f() { while true { if true { true -> { break } false -> {} } @println(1) } @println(2) }",
        "fn f() { while true { continue @println(1) } }",
    ] {
        assert!(
            warnings(source).iter().any(|m| m == "unreachable code"),
            "{source}"
        );
    }
}

#[test]
fn imported_flow_warnings_keep_original_statement_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let library = directory.path().join("spin.nc");
    let library_source = "pub fn spin<T>(T value) {\n    fn local = fn() {\n        while true {}\n        @println(value)\n    }\n}\n";
    fs::write(&library, library_source).unwrap();
    let main = directory.path().join("main.nc");
    let source = "import { \"spin\" as worker } worker.spin<int>(1) worker.spin<str>(\"x\")";
    for release in [false, true] {
        let output = ncc::compile_source_with_diagnostics(source, &main, release).unwrap();
        assert_eq!(output.warnings.0.len(), 2);
        for diagnostic in &output.warnings.0 {
            assert_eq!(diagnostic.path.as_deref(), Some(library.as_path()));
            let text = &library_source[diagnostic.span.clone()];
            if diagnostic.message.contains("infinite loop") {
                assert!(text.starts_with("while true"), "{text}");
            } else {
                assert_eq!(diagnostic.message, "unreachable code");
                assert!(text.starts_with("@println"), "{text}");
            }
        }
        let rendered = output.warnings.render_warnings(source, &main);
        assert!(rendered.contains("spin.nc:3:"), "{rendered}");
        assert!(rendered.contains("spin.nc:4:"), "{rendered}");
    }
}
