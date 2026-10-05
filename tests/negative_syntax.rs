use std::path::Path;

fn rejects(source: &str, expected: &str) {
    rejects_at(source, expected, None);
}

fn rejects_at(source: &str, expected: &str, literal: Option<&str>) {
    for release in [false, true] {
        let path = Path::new("negative_syntax.nc");
        let error = ncc::compile_source_with_options(source, path, release).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\nexpected {expected:?}: {error}"
        );
        assert!(!error.0.is_empty(), "release={release}: {source}");
        if let Some(literal) = literal {
            assert_eq!(
                &source[error.0[0].span.clone()],
                literal,
                "release={release}: {error}"
            );
        }
        for diagnostic in &error.0 {
            assert!(
                source.get(diagnostic.span.clone()).is_some(),
                "release={release}: {source}\ninvalid diagnostic range: {diagnostic:?}"
            );
        }
        assert!(
            error.render(source, path).contains("negative_syntax.nc:"),
            "release={release}: {source}\n{error}"
        );
    }
}

fn accepts(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("syntax_control.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {source}\n{error}"));
    }
}

#[test]
fn unknown_and_quote_specific_escapes_reject_through_compiler_api() {
    for (literal, expected) in [
        (r#""\q""#, r"unknown escape `\q`"),
        (r"'\q'", r"unknown escape `\q`"),
        (r#""\0""#, r"unknown escape `\0`"),
        (r"'\0'", r"unknown escape `\0`"),
        (r#""\'""#, r"unknown escape `\'`"),
        (r#"'\"'"#, r#"unknown escape `\"`"#),
        (r"'\{'", r"unknown escape `\{`"),
        (r#""\}""#, r"unknown escape `\}`"),
        (r#""""\q""""#, r"unknown escape `\q`"),
    ] {
        rejects(&format!("_ = {literal}"), expected);
    }
    accepts(
        r#"str text = "\e\n\r\t\\\"\{literal}\u{0}"
char quote = '\''
char slash = '\\'
char nul = '\u{0}'
str multiline = """\e\n\r\t\\\"\{literal}\u{0}""""#,
    );
}

#[test]
fn malformed_unicode_escapes_reject_in_strings_and_characters() {
    for (escape, expected) in [
        (r"\u1234", "Unicode escapes require"),
        (r"\u{}", "Unicode escape requires 1 to 6 hex digits"),
        (r"\u{xyz}", "Unicode escape requires 1 to 6 hex digits"),
        (r"\u{12G}", "Unicode escape requires 1 to 6 hex digits"),
        (r"\u{0000000}", "Unicode escape requires 1 to 6 hex digits"),
        (r"\u{41", "Unicode escape requires 1 to 6 hex digits"),
        (r"\u{D800}", "not a valid Unicode scalar value"),
        (r"\u{DFFF}", "not a valid Unicode scalar value"),
        (r"\u{110000}", "not a valid Unicode scalar value"),
        (r"\u{FFFFFF}", "not a valid Unicode scalar value"),
    ] {
        for quote in ["\"", "'", "\"\"\""] {
            rejects(&format!("_ = {quote}{escape}{quote}"), expected);
        }
    }
    for escape in [
        r"\u{0}",
        r"\u{00001B}",
        r"\u{D7FF}",
        r"\u{E000}",
        r"\u{10FFFF}",
    ] {
        accepts(&format!("char value = '{escape}'\nstr text = \"{escape}\""));
    }
}

#[test]
fn character_literals_require_exactly_one_extended_grapheme() {
    for literal in ["''", "'ab'", "'🍪🍪'", r"'\u{41}\u{42}'", r"'\n\t'"] {
        rejects(
            &format!("char value = {literal}"),
            "a char literal must contain one Unicode grapheme cluster",
        );
    }
    for literal in ["'a'", "'🍪'", "'🇮🇳'", "'👩‍👩‍👧‍👦'", "'o\u{308}'", r"'o\u{308}'"]
    {
        accepts(&format!("char value = {literal}"));
    }
}

#[test]
fn unterminated_quoted_literals_and_interpolations_reject() {
    for source in [
        "str value = \"text",
        "char value = 'x",
        "str value = \"text\\",
        "char value = '\\",
        "str value = \"\"\"\n  text\n",
        "str value = \"value {1 + 2\"",
        "str value = \"\"\"value {1 + 2\"\"\"",
    ] {
        rejects(source, "unterminated string literal");
    }
    accepts(
        "str text = \"text\"\nchar character = 'x'\nstr multiline = \"\"\"\n  text\n  \"\"\"\nstr formatted = \"value {1 + 2}\"",
    );
}

#[test]
fn malformed_interpolation_expressions_reject_at_the_original_literal() {
    for (literal, expected) in [
        (r#""{}""#, "expected expression"),
        (r#""{1 + }""#, "expected expression"),
        (r#""{1 2}""#, "expected Eof"),
        (r#""{1; 2}""#, "expected Eof"),
        (r#""{`}""#, "unexpected character"),
        (r#""🍪\u{41}\n{1 + }""#, "expected expression"),
        ("\"\"\"\n  🍪 {1 + }\n  \"\"\"", "expected expression"),
    ] {
        let source = format!("// header\nstr value = {literal}");
        rejects_at(&source, expected, Some(literal));
    }
    accepts(
        r#"str text = "🍪\u{41}\n{1 + 2} \{literal}"
str nested = "{(1 + 2) * 3}"
str quoted = "{"quoted"}""#,
    );
}

#[test]
fn radix_literals_require_digits_valid_for_the_specified_base() {
    for literal in [
        "0b", "0o", "0x", "0b2", "0b102", "0o8", "0o789", "0xG", "0xFG",
    ] {
        rejects(
            &format!("int value = {literal}"),
            "invalid or overflowing integer literal",
        );
    }
    for literal in ["0b1011", "0o777", "0xFF", "0xff", "0xFf"] {
        accepts(&format!("int value = {literal}"));
        accepts(&format!("uint value = {literal}u"));
    }
}

#[test]
fn expressions_require_closing_delimiters_and_comma_separated_elements() {
    for (source, expected) in [
        ("_ = (1]", "expected RParen"),
        ("_ = (1", "expected RParen"),
        ("_ = (1, 2]", "expected RParen"),
        ("_ = [1, 2)", "expected Comma"),
        ("_ = [1, 2", "expected Comma"),
        ("_ = [1 2]", "expected Comma"),
        ("_ = (1; 2)", "expected RParen"),
        ("_ = [1; 2]", "expected Comma"),
        ("@println(1 2)", "expected Comma"),
        ("@println(1; 2)", "expected Comma"),
        ("@println(1]", "expected Comma"),
        ("int[] values = [1]\n_ = values[0)", "expected RBracket"),
        ("int[] values = [1]\n_ = values[0", "expected RBracket"),
        ("int value = 1 +", "expected expression"),
        ("int[] values = [1]\n_ = values.", "expected identifier"),
    ] {
        rejects(source, expected);
    }
    accepts(
        "int[] values = [1, 2]\n(int, int) pair = (1, 2)\n_ = (1 + 2)\n_ = values[0]\n_ = values.len\n@println(1, 2)",
    );
}

#[test]
fn maps_and_arrays_cannot_mix_entry_syntax() {
    for (source, expected) in [
        ("_ = [1, 2: 3]", "cannot mix map entries and array elements"),
        ("_ = [1: 2, 3]", "expected `:` after map key"),
        ("_ = [1: ]", "expected expression"),
        ("_ = [1: 2 3: 4]", "expected Comma"),
        ("_ = [1: 2; 3: 4]", "expected Comma"),
    ] {
        rejects(source, expected);
    }
    accepts(
        "int[] array = [1, 2]\n[str]int map = [\"first\": 1, \"second\": 2,]\nint[] empty_array = []",
    );
}

#[test]
fn bindings_and_type_declarations_require_names_equals_and_values() {
    for (source, expected) in [
        ("int value", "expected Assign"),
        ("int value 1", "expected Assign"),
        ("int value =", "expected expression"),
        ("mut int value =", "expected expression"),
        ("type Count int", "expected Assign"),
        ("type = int", "expected identifier"),
        ("type Count =", "expected identifier"),
        ("struct { int value }", "expected identifier"),
        ("struct Record { int }", "expected identifier"),
        ("enum { Empty }", "expected identifier"),
    ] {
        rejects(source, expected);
    }
    accepts(
        "type Count = int\nCount count = 1\nmut int value = 2\nvalue = 3\nstruct Record { int value str name }\nenum Choice { Empty Number(int) }",
    );
}

#[test]
fn struct_initializers_require_designators_equals_and_commas() {
    for (initializer, expected) in [
        (
            "Record{value = 1, .name = \"ok\"}",
            "expected newline or `;`",
        ),
        ("Record{.value: 1, .name = \"ok\"}", "expected Assign"),
        ("Record{.value = 1 .name = \"ok\"}", "expected Comma"),
        ("Record{.value = 1; .name = \"ok\"}", "expected Comma"),
        ("Record{.value = , .name = \"ok\"}", "expected expression"),
    ] {
        rejects(
            &format!("struct Record {{ int value str name }}\nRecord record = {initializer}"),
            expected,
        );
    }
    accepts(
        "struct Record { int value str name }\nRecord record = Record{.value = 1, .name = \"ok\"}",
    );
}

#[test]
fn control_and_function_syntax_require_keywords_and_braced_bodies() {
    for (source, expected) in [
        ("for index [1] {}", "expected `in`"),
        ("for index in [1] @println(index)", "expected LBrace"),
        ("while false @println(1)", "expected LBrace"),
        (
            "if true { true { @println(1) } false -> {} }",
            "expected `->` after conditional pattern(s)",
        ),
        (
            "if true { true -> @println(1) false -> {} }",
            "expected LBrace",
        ),
        ("fn missing_body() int", "expected LBrace"),
        (
            "fn unclosed(int value) int { return value",
            "expected newline or `;`",
        ),
        ("test \"ignored\" { _ = [1 2] }", "expected Comma"),
        ("test \"ignored\" { str value = \"\\q\" }", "unknown escape"),
    ] {
        rejects(source, expected);
    }
    accepts(
        "fn identity(int value) int { return value }\n_ = identity(1)\nfor index in [1] { @println(index) }\nwhile false {}\nif true { true -> { @println(1) } false -> {} }\ntest \"ignored\" { assert true }",
    );
}
