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
        r#"'\{'"#,
    ] {
        assert!(ncc::lexer::lex(source).is_err(), "{source}");
    }
    for source in [
        r#"'\u{0}'"#,
        r#"'\u{10FFFF}'"#,
        r#"'\u{1f36a}'"#,
        r#"'o\u{308}'"#,
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
fn semantic_errors_point_to_the_failing_statement() {
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
            assert_eq!(
                &source[diagnostic.span.clone()],
                statement,
                "{source}: {error}"
            );
            assert!(
                error.render(text, &root).contains(":3:3: error:"),
                "{source}: {error}"
            );
        }
    }
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
        rendered.contains(&format!("{}:4:3:", imported.display())),
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
