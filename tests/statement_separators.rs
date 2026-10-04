use std::{fs, path::Path, process::Command};

fn parse(source: &str) -> Result<ncc::ast::Module, ncc::diagnostic::Diagnostics> {
    ncc::parser::parse(ncc::lexer::lex(source).unwrap())
}

#[test]
fn accepts_newlines_semicolons_and_mixed_statement_lists() {
    for source in [
        "mut int a = 1\na = 2\nmut int b = 1; b = 2",
        "int a = 1\r\nint b = 2",
        "int a = 1 // comment with ;\nint b = 2",
        ";; int a = 1;;;\n; int b = 2;;",
        "fn f() {; return;; }; f();",
        "while true { break; @println(1) }; @println(2)",
        "while true { continue; @println(1) }",
        "fn f() int { return 1; }; _ = f()",
        "int a = if { true -> { break 1; } };",
        "import { \"a\" as a }; import { \"b\" as b }; int x = 1",
        "type Number = int; struct S { int value }; enum E { A B };",
        "extern \"a.c\" as c { fn a() = \"a\" }; c.a()",
        "test \"first\" { assert true; assert true }; test \"second\" {}",
        "fn f() { fn inner() {}; inner(); { @println(1); }; }",
        "fn f() { outer: while true { break :outer; }; }",
    ] {
        parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    }
}

#[test]
fn rejects_missing_statement_separators_in_both_modes() {
    for (source, next) in [
        ("mut int a = 1 a = 2", "a = 2"),
        ("@println(1) @println(2)", "@println(2)"),
        ("fn f() {} f()", "f()"),
        ("fn f() { int a = 1 return }", "return"),
        ("fn f() { {} {} }", "{} }"),
        ("fn f() { while true { break } return }", "return"),
        ("type Number = int int a = 1", "int a"),
        ("struct S { int value } int a = 1", "int a"),
        ("enum E { A B } int a = 1", "int a"),
        ("import { \"a\" as a } int x = 1", "int x"),
        ("extern \"a.c\" as c { fn a() = \"a\" } int x = 1", "int x"),
        ("test \"ignored\" {} @println(1)", "@println(1)"),
        (
            "test \"ignored\" { assert true assert true }",
            "assert true }",
        ),
    ] {
        for release in [false, true] {
            for test_mode in [false, true] {
                let path = Path::new("separators.nc");
                let error = if test_mode {
                    ncc::compile_test_source_with_options(source, path, release)
                } else {
                    ncc::compile_source_with_options(source, path, release)
                }
                .unwrap_err();
                assert!(
                    error.to_string().contains("expected newline or `;`"),
                    "{source}: {error}"
                );
                assert_eq!(
                    error.0[0].span.start,
                    source.rfind(next).unwrap(),
                    "{source}"
                );
                assert_eq!(error.0[0].path.as_deref(), Some(path));
            }
        }
    }
}

#[test]
fn separators_do_not_change_multiline_expressions_or_member_lists() {
    for source in [
        "int a = 1 +\n2; int b = 3\n+ 4",
        "int a\n=\n1; int[] b = [\n1,\n2\n]; _ = b[\n0\n]",
        "fn f(int a, int b) int { return a + b }; _ = f(\n1,\n2\n)",
        "struct S { int a int b }; S s = S{.a = 1, .b = 2}; _ = s\n.a",
        "int? a = none; int b = a\nelse { break 2; }",
        "fn f() int! { return 1 }; int a = f()\ncatch message { break 2; }",
        "_ = \"literal ; text\"; _ = ';'; _ = \"{if { true -> { break 1; } }}\"",
        "_ = \"\"\"line one\nline two;\n\"\"\"; int a = 1",
        "import { \"a\" as a \"b\" as b }; enum E { A B }",
        "extern \"a.c\" as c { fn a() = \"a\" fn b() = \"b\" }",
    ] {
        parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    }
    for source in ["_ = [1; 2]", "_ = (1; 2)", "@println(1; 2)"] {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn imported_separator_errors_retain_original_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let library = directory.path().join("library.nc");
    let source = "// library\npub fn f() { int a = 1 return }\n";
    fs::write(&library, source).unwrap();
    for release in [false, true] {
        let error = ncc::compile_source_with_options(
            "import { \"library\" as lib }; lib.f()",
            &root,
            release,
        )
        .unwrap_err();
        assert_eq!(error.0[0].path.as_ref(), Some(&library));
        assert_eq!(&source[error.0[0].span.clone()], "return");
    }
}

#[test]
fn semicolon_programs_preserve_execution_order_in_debug_and_release() {
    let source = r#"
        mut int value = 1; value = 2
        fn advance() { value = value + 1; return; value = 99 }
        advance(); @println(value); value = value + 4
        while true { value = value + 1; break; value = 99 }
        int answer = if { _ -> { break value; } }
        @println(answer)
        test "separators" { assert value == 8; assert answer == 8 }
    "#;
    let directory = ncc::temp::Directory::new().unwrap();
    let c_path = directory.path().join("program.c");
    let executable = directory.path().join("program");
    for release in [false, true] {
        for test_mode in [false, true] {
            let c = if test_mode {
                ncc::compile_test_source_with_options(source, Path::new("separators.nc"), release)
            } else {
                ncc::compile_source_with_options(source, Path::new("separators.nc"), release)
            }
            .unwrap();
            fs::write(&c_path, c).unwrap();
            assert!(
                Command::new("cc")
                    .arg(&c_path)
                    .arg("-o")
                    .arg(&executable)
                    .status()
                    .unwrap()
                    .success()
            );
            let output = Command::new(&executable).output().unwrap();
            assert!(
                output.status.success(),
                "release={release}, test_mode={test_mode}: {output:?}"
            );
            assert_eq!(
                output.stdout,
                if test_mode {
                    b"".as_slice()
                } else {
                    b"3\n8\n".as_slice()
                }
            );
        }
    }
}
