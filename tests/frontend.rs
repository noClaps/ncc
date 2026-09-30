#[test]
fn type_errors_use_language_syntax_instead_of_rust_debug_output() {
    for (source, expected) in [
        ("int value = true", "expected `int`, found `bool`"),
        ("int[] values = [true]", "expected `int`, found `bool`"),
        (
            "fn f(int[] values) {} f([true])",
            "expected `int`, found `bool`",
        ),
        (
            "(fn(int) int) f = fn(str s) str { return s }",
            "expected `(fn(int) int)`, found `(fn(str) str)`",
        ),
    ] {
        let error = ncc::compile_source(source, std::path::Path::new("types.nc")).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn unicode_escape_validation_is_strict() {
    for source in [
        r#""\u{}""#,
        r#""\u{D800}""#,
        r#""\u{110000}""#,
        r#""\u{0000000}""#,
        r#""\u{xyz}""#,
        r#""\u1234""#,
        r#""\u{41""#,
        r#""\'""#,
        r#"'\"'"#,
        r"'\{'",
    ] {
        assert!(ncc::lexer::lex(source).is_err(), "{source}");
    }
    for source in [
        r"'\u{0}'",
        r"'\u{10FFFF}'",
        r"'\u{1f36a}'",
        r"'o\u{308}'",
        r#""\e\n\r\t\\\"\{""#,
    ] {
        assert!(ncc::lexer::lex(source).is_ok(), "{source}");
    }
}

#[test]
fn multiline_strings_preserve_content_and_handle_escapes() {
    use ncc::lexer::{TokenKind, lex};
    for (source, expected) in [
        (
            "\"\"\"\n  Hello\n    indented\n  \"\"\"",
            "Hello\n  indented\n",
        ),
        ("\"\"\"\n界\n  \"\"\"", "界\n"),
        (
            "\"\"\"inline \"quotes\" and \\t tab\"\"\"",
            "inline \"quotes\" and \t tab",
        ),
        ("\"\"\"\n  \\{literal}\n  \"\"\"", "\\{literal}\n"),
        ("\"\"\"\n{\"quoted\"}\n\"\"\"", "{\"quoted\"}\n"),
        ("\"\"\"\r\n  hello\r\n  \"\"\"", "hello\r\n"),
    ] {
        assert_eq!(
            lex(source).unwrap()[0].kind,
            TokenKind::String(expected.into())
        );
    }
}

#[test]
fn char_literals_are_extended_graphemes() {
    for character in ["o\u{308}", "👩‍👩‍👧‍👦", "🇮🇳"] {
        assert!(ncc::lexer::lex(&format!("'{character}'")).is_ok());
    }
    assert!(ncc::lexer::lex("'ab'").is_err());
}

#[test]
fn semantic_errors_point_to_the_failing_expression_or_statement() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    for (prefix, statement, suffix) in [
        ("fn broken() int {", "return missing", "}"),
        ("fn broken() { int x = 1", "x = 2", "}"),
        ("fn broken() {", "break", "}"),
        ("fn broken() {", "continue", "}"),
        ("fn broken() {", "assert true", "}"),
        ("fn target(int x) {} fn broken() {", "target(true)", "}"),
        (
            "fn target<T>(T x) {} fn broken() {",
            "target<int, bool>(1)",
            "}",
        ),
        (
            "fn broken() int { return if { true -> {",
            "false",
            "} _ -> { 1 } } }",
        ),
        ("fn broken() { while true {", "return missing", "} }"),
        ("", "@println(missing)", ""),
    ] {
        let source = format!("// header\n{prefix}\n  {statement}\n{suffix}\n");
        for from_import in [false, true] {
            std::fs::write(&imported, &source).unwrap();
            let (text, path) = if from_import {
                ("import { \"library\" as lib }", &imported)
            } else {
                (source.as_str(), &root)
            };
            let error = ncc::compile_source(text, &root).unwrap_err();
            let diagnostic = &error.0[0];
            assert_eq!(diagnostic.path.as_ref(), Some(path), "{source}: {error}");
            let expected = match statement {
                "return missing" | "@println(missing)" => "missing",
                "x = 2" => "x",
                "target(true)" => "true",
                _ => statement,
            };
            assert_eq!(
                &source[diagnostic.span.clone()],
                expected,
                "{source}: {error}"
            );
            assert!(
                error.render(text, &root).contains(&format!(
                    ":3:{}: error:",
                    statement.find(expected).unwrap() + 3
                )),
                "{source}: {error}"
            );
        }
    }
}

#[test]
fn nested_expression_errors_keep_exact_ranges_through_specialization() {
    for (source, expected) in [
        ("int value = 1 + missing * 2", "missing"),
        ("fn take(int n) {} take(1 + true)", "1 + true"),
        ("fn take(int n) {} take(false)", "false"),
        ("int[] values = [1, false, 3]", "false"),
        ("struct S { int value } S s = S{.value = false}", "false"),
        (
            "int[] values = [1] _ = values.missing.len",
            "values.missing",
        ),
        (
            "fn generic<T>(T value) T { return missing } _ = generic<int>(1)",
            "missing",
        ),
        ("str text = \"🍪\"\nint value = 1 + missing", "missing"),
        (
            "fn f() {}\nint x = if true { true -> { 1 } false -> { false } }",
            "false",
        ),
    ] {
        for release in [false, true] {
            let path = std::path::Path::new("expressions.nc");
            let error = ncc::compile_source_with_options(source, path, release).unwrap_err();
            let diagnostic = &error.0[0];
            assert_eq!(
                &source[diagnostic.span.clone()],
                expected,
                "{source}: {error}"
            );
            assert_eq!(diagnostic.path.as_deref(), Some(path));
        }
    }
}

#[test]
fn interpolation_errors_point_to_the_original_literal() {
    for literal in [
        r#""value {missing}""#,
        r#""🍪\u{41}\n{missing}""#,
        r#""{1 + }""#,
        r#""{`}""#,
        "\"\"\"\n  🍪 {missing}\n  \"\"\"",
    ] {
        let source = format!("// header\n\n@println({literal})");
        let error = ncc::compile_source(&source, std::path::Path::new("format.nc")).unwrap_err();
        assert_eq!(&source[error.0[0].span.clone()], literal, "{error}");
    }
}

#[test]
fn constant_evaluation_errors_retain_imported_expression_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    let source = "// header\npub fn bad() int {\n  return 1 / 0\n}\n";
    std::fs::write(&imported, source).unwrap();
    let error = ncc::compile_source_with_options(
        "import { \"library\" as lib } @println(lib.bad())",
        &root,
        true,
    )
    .unwrap_err();
    assert_eq!(error.0[0].path.as_ref(), Some(&imported));
    assert_eq!(&source[error.0[0].span.clone()], "1 / 0");
}

#[test]
fn imported_type_and_external_errors_retain_declaration_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    std::fs::write(directory.path().join("native.c"), "").unwrap();
    for declaration in [
        "pub struct Bad { Missing field }",
        "pub enum Bad { Value(Missing) }",
        "pub type Bad = Missing",
        "pub type Bad = Bad",
        "pub struct Bad { Bad field }",
        "extern \"native.c\" as c { fn bad(Missing value) = \"bad\" }",
        "extern \"missing.c\" as c { fn bad() = \"bad\" }",
        "struct Box<T> { T value }\npub type Bad = Box<int, bool>",
    ] {
        let text = format!("// header\n\n{declaration}\n");
        std::fs::write(&imported, &text).unwrap();
        let error = ncc::compile_source("import { \"library\" as lib }", &root).unwrap_err();
        assert_eq!(
            error.0[0].path.as_deref(),
            Some(imported.as_path()),
            "{error}"
        );
        assert!(!error.0[0].span.is_empty(), "{error}");
        assert!(error.0[0].span.start >= "// header\n\n".len(), "{error}");
        let rendered = error.render("import { \"library\" as lib }", &root);
        assert!(rendered.contains("library.nc:"), "{rendered}");
        assert!(!rendered.contains("library.nc:1:"), "{rendered}");
    }
}

#[test]
fn imported_errors_retain_source_paths_and_declaration_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    std::fs::write(
        &imported,
        "// header\n\npub fn broken() int {\n  int n = false\n  return n\n}\n",
    )
    .unwrap();
    let source = "import { \"library\" as lib }";
    let error = ncc::compile_source(source, &root).unwrap_err();
    assert_eq!(error.0[0].path.as_deref(), Some(imported.as_path()));
    let rendered = error.render(source, &root);
    assert!(
        rendered.contains(&format!("{}:4:11:", imported.display())),
        "{rendered}"
    );
    assert!(rendered.contains("int n = false"));
    std::fs::write(&imported, "pub fn broken( { }").unwrap();
    assert_eq!(
        ncc::compile_source(source, &root).unwrap_err().0[0]
            .path
            .as_deref(),
        Some(imported.as_path())
    );
}

#[test]
fn imported_generic_type_errors_retain_nested_helper_expression_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let helper = nested.join("helper.nc");
    let library = nested.join("library.nc");
    let helper_source =
        "// helper header\n\npub fn double<T>(T value) T {\n  return value + value\n}\n";
    std::fs::write(&helper, helper_source).unwrap();
    std::fs::write(
        &library,
        "import { \"helper\" as helper }\npub fn double<T>(T value) T {\n  return helper.double<T>(value)\n}\n",
    )
    .unwrap();
    let source = "import { \"nested/library\" as lib } _ = lib.double<str>(\"text\")";
    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, &root, release).unwrap_err();
        let diagnostic = &error.0[0];
        assert_eq!(
            diagnostic.path.as_deref(),
            Some(helper.as_path()),
            "{error}"
        );
        let start = helper_source.find("value + value").unwrap();
        assert_eq!(
            diagnostic.span,
            start..start + "value + value".len(),
            "{error}"
        );
        let rendered = error.render(source, &root);
        assert!(
            rendered.contains(&format!("{}:4:10:", helper.display())),
            "{rendered}"
        );
        assert!(rendered.contains("return value + value"), "{rendered}");
    }
}

#[test]
fn unused_generic_bodies_are_typechecked_only_when_specialized() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    let library =
        "// library header\npub fn get_field<T>(T value) int {\n  return value.field\n}\n";
    std::fs::write(&imported, library).unwrap();
    for release in [false, true] {
        for source in [
            "import { \"library\" as lib }",
            "import { \"library\" as lib } struct Record { int field } _ = lib.get_field<Record>(Record{.field = 7})",
        ] {
            ncc::compile_source_with_options(source, &root, release)
                .unwrap_or_else(|error| panic!("release={release}, source={source}: {error}"));
        }
        let source = "import { \"library\" as lib } _ = lib.get_field<int>(7)";
        let error = ncc::compile_source_with_options(source, &root, release).unwrap_err();
        let diagnostic = &error.0[0];
        assert_eq!(
            diagnostic.path.as_deref(),
            Some(imported.as_path()),
            "{error}"
        );
        let start = library.find("value.field").unwrap();
        assert_eq!(
            diagnostic.span,
            start..start + "value.field".len(),
            "{error}"
        );
        let rendered = error.render(source, &root);
        assert!(
            rendered.contains(&format!("{}:3:10:", imported.display())),
            "{rendered}"
        );
        assert!(rendered.contains("return value.field"), "{rendered}");
    }
}

#[test]
fn imported_generic_mutex_reads_report_the_original_expression() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("library.nc");
    let library = "mutex int value = 1\npub fn read<T>(T ignored) int {\n  return value\n}\n";
    std::fs::write(&imported, library).unwrap();
    let source = "import { \"library\" as lib } _ = lib.read<int>(0)";
    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, &root, release).unwrap_err();
        assert_eq!(error.0[0].path.as_deref(), Some(imported.as_path()));
        assert_eq!(&library[error.0[0].span.clone()], "value");
        assert!(error.to_string().contains("outside a lock scope"));
    }
}
