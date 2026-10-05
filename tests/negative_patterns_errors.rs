use std::path::Path;

fn rejects(source: &str, expected: &str) {
    let mut failures = Vec::new();
    for release in [false, true] {
        match ncc::compile_source_with_options(
            source,
            Path::new("negative_patterns_errors.nc"),
            release,
        ) {
            Ok(_) => failures.push(format!(
                "release={release}, expected diagnostic containing {expected:?}, but compilation succeeded\nsource:\n{source}"
            )),
            Err(error) if !error.to_string().contains(expected) => failures.push(format!(
                "release={release}, expected diagnostic containing {expected:?}\nsource:\n{source}\ndiagnostic:\n{error}"
            )),
            Err(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn accepts(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("patterns_errors_control.nc"), release)
            .unwrap_or_else(|error| {
                panic!("release={release}\nsource:\n{source}\ndiagnostic:\n{error}")
            });
    }
}

#[test]
fn boolean_patterns_must_cover_both_values_even_for_known_subjects() {
    for source in [
        "bool value = true;if value { true -> {} }",
        "bool value = false;if value { false -> {} }",
        "fn missing(bool value) { if value { true, true -> {} } }",
        "fn missing(bool value) { if value { false, false -> {} } }",
        "int result = if true { true -> { 1 } }",
        "int result = if false { false -> { 0 } }",
    ] {
        rejects(source, "not exhaustive");
    }
    accepts(
        "fn complete(bool value) int { return if value { true -> { 1 } false -> { 0 } } };_ = complete(true);_ = complete(false)",
    );
    accepts(
        "fn complete(bool value) int { return if value { false -> { 0 } _ -> { 1 } } };_ = complete(true)",
    );
}

#[test]
fn enum_patterns_must_include_absent_variants_even_with_total_payload_patterns() {
    for source in [
        "enum Choice { Empty Number(int) Text(str) };fn missing(Choice value) { if value { Choice.Number(n) -> {} Choice.Text(_) -> {} } }",
        "enum Choice { Empty Number(int) Text(str) };fn missing(Choice value) { if value { Choice.Empty -> {} Choice.Number(_) -> {} } }",
        "enum Choice { Empty Number(int) Text(str) };Choice value = Choice.Number(1);if value { Choice.Number(_) -> {} Choice.Number(n) -> {} }",
    ] {
        rejects(source, "not exhaustive");
    }
    accepts(
        "enum Choice { Empty Number(int) Text(str) };fn complete(Choice value) { if value { Choice.Empty -> {} Choice.Number(n) -> {} Choice.Text(_) -> {} } };complete(Choice.Empty);complete(Choice.Number(1));complete(Choice.Text(\"text\"))",
    );
    accepts(
        "enum Choice { Empty Number(int) Text(str) };fn complete(Choice value) { if value { Choice.Number(_) -> {} _ -> {} } };complete(Choice.Empty)",
    );
}

#[test]
fn dynamic_array_patterns_cannot_exhaust_all_lengths() {
    // Cover every Boolean value at length one; length, not element coverage,
    // still requires a fallback for a dynamically sized array.
    for source in [
        "fn missing(bool[] value) { if value { [] -> {} [true], [false] -> {} [_, _] -> {} } }",
        "bool[] value = [];if value { [] -> {} }",
        "int[] value = [1, 2];if value { [a, b] -> {} }",
    ] {
        rejects(source, "not exhaustive");
    }
    accepts(
        "fn complete(bool[] value) { if value { [] -> {} [true], [false] -> {} [_, _] -> {} _ -> {} } };complete([]);complete([true]);complete([false]);complete([true, false, true])",
    );
}

#[test]
fn string_patterns_need_fallback_even_when_the_known_subject_matches() {
    for source in [
        "str value = \"\";if value { \"\" -> {} }",
        "str value = \"hello\";if value { \"hello\", \"hello\" -> {} }",
        "fn missing(str value) { if value { \"\", \"a\", \"aa\", \"aaa\" -> {} } }",
    ] {
        rejects(source, "not exhaustive");
    }
    accepts(
        "fn complete(str value) { if value { \"\", \"a\", \"aa\", \"aaa\" -> {} _ -> {} } };complete(\"\");complete(\"aaaa\")",
    );
}

#[test]
fn fixed_array_patterns_reject_empty_patterns_for_nonempty_subjects() {
    rejects(
        "fn bad(int[2] value) { if value { [] -> {} _ -> {} } }",
        "length",
    );
}

#[test]
fn fixed_array_patterns_reject_shorter_nonempty_patterns() {
    rejects(
        "fn bad(int[2] value) { if value { [_] -> {} _ -> {} } }",
        "length",
    );
}

#[test]
fn fixed_array_patterns_reject_nonempty_patterns_for_empty_subjects() {
    rejects(
        "fn bad(int[0] value) { if value { [_] -> {} _ -> {} } }",
        "length",
    );
}

#[test]
fn fixed_array_patterns_reject_longer_patterns_for_nonempty_subjects() {
    rejects(
        "fn bad(int[2] value) { if value { [_, _, _] -> {} _ -> {} } }",
        "length",
    );
}

#[test]
fn fixed_array_patterns_with_exact_lengths_are_valid_controls() {
    accepts(
        "fn empty(int[0] value) { if value { [] -> {} } };fn pair(int[2] value) { if value { [0, _] -> {} [a, b] -> {} } };empty([]);pair([0, 1]);pair([2, 3])",
    );
}

#[test]
fn tuple_patterns_require_exact_arity_even_with_fallback() {
    for source in [
        "fn bad((int, bool, str) value) { if value { (_, _) -> {} _ -> {} } }",
        "fn bad((int, bool) value) { if value { (_, _, _) -> {} _ -> {} } }",
    ] {
        rejects(source, "tuple pattern has wrong arity");
    }
    accepts(
        "fn complete((int, bool) value) { if value { (1, true) -> {} (number, flag) -> {} } };complete((1, true));complete((2, false))",
    );
}

#[test]
fn tuple_pattern_literals_must_match_each_member_type() {
    for (pattern, expected) in [
        ("(true, _)", "expected `int`, found `bool`"),
        ("(_, 1)", "expected `bool`, found `int`"),
        ("(\"one\", _)", "expected `int`, found `str`"),
        ("(_, \"true\")", "expected `bool`, found `str`"),
    ] {
        rejects(
            &format!("fn bad((int, bool) value) {{ if value {{ {pattern} -> {{}} _ -> {{}} }} }}"),
            expected,
        );
    }
    accepts(
        "fn complete((int, bool) value) { if value { (1, true) -> {} (_, _) -> {} } };complete((1, true));complete((2, false))",
    );
}

#[test]
fn full_struct_patterns_constrained_by_literals_are_not_exhaustive() {
    // All fields are present. No omitted-field or nested finite-partition rule
    // is needed to see that names other than these literals remain uncovered.
    for arms in [
        "Person{.name = \"Ada\", .age = 1} -> {}",
        "Person{.name = \"Ada\", .age = age} -> {}",
        "Person{.name = \"Ada\", .age = _} -> {} Person{.name = \"Grace\", .age = age} -> {}",
    ] {
        rejects(
            &format!(
                "struct Person {{ str name int age }};fn missing(Person value) {{ if value {{ {arms} }} }}"
            ),
            "not exhaustive",
        );
    }
    accepts(
        "struct Person { str name int age };fn complete(Person value) { if value { Person{.name = \"Ada\", .age = age} -> {} Person{.name = name, .age = _} -> {} } };complete(Person{.name = \"Grace\", .age = 2})",
    );
}

#[test]
fn full_tuple_patterns_constrained_by_literals_are_not_exhaustive() {
    for arms in [
        "(\"Ada\", 1) -> {}",
        "(\"Ada\", age) -> {}",
        "(\"Ada\", _) -> {} (\"Grace\", age) -> {}",
    ] {
        rejects(
            &format!("fn missing((str, int) value) {{ if value {{ {arms} }} }}"),
            "not exhaustive",
        );
    }
    accepts(
        "fn complete((str, int) value) { if value { (\"Ada\", age) -> {} (name, _) -> {} } };complete((\"Grace\", 2))",
    );
}

#[test]
fn error_values_cannot_be_constructed_directly_from_strings() {
    for source in [
        "error value = \"failure\"",
        "str message = \"failure\";error value = message",
        "fn bad() { error value = \"failure\" }",
    ] {
        rejects(source, "expected `error`, found `str`");
    }
    accepts(
        "fn fail(str message) int! { throw message };int! pending = fail(\"failure\");int recovered = pending catch err { break 0 }",
    );
}

#[test]
fn caught_errors_cannot_be_returned_as_ordinary_success_payloads() {
    for (result, expected) in [
        ("int!", "expected `int`, found `error`"),
        ("str!", "expected `str`, found `error`"),
    ] {
        rejects(
            &format!(
                "fn fail() int! {{ throw \"failure\" }};fn bad() {result} {{ _ = fail() catch err {{ return err }};throw \"other\" }}"
            ),
            expected,
        );
    }
    accepts(
        "fn fail() int! { throw \"failure\" };fn forward() int! { _ = fail() catch err { throw err };return 0 };int recovered = forward() catch _ { break 1 }",
    );
}

#[test]
fn caught_errors_cannot_be_returned_even_with_an_error_return_annotation() {
    // The success path diverges, so no unrelated return-type mismatch can mask
    // the forbidden ordinary return of the catch binding.
    rejects(
        "fn fail() int! { throw \"failure\" };fn bad() error { _ = fail() catch err { return err };while true {} }",
        "errors cannot be returned; they must be thrown",
    );
}

#[test]
fn caught_error_returns_reject_promotions_and_nominal_alias_chains() {
    for source in [
        "fn fail() int! { throw \"failure\" };fn bad() error! { _ = fail() catch err { return err };throw \"other\" }",
        "type Failure = error;type Wrapped = Failure;fn fail() int! { throw \"failure\" };fn bad() Wrapped { _ = fail() catch err { return @as(Wrapped, @as(Failure, err)) };while true {} }",
    ] {
        rejects(source, "errors cannot be returned; they must be thrown");
    }
    accepts(
        "fn fail() int! { throw \"failure\" };fn forward() int! { return fail() };type Result = int!;fn wrapped() Result { return @as(Result, fail()) };_ = forward();_ = wrapped()",
    );
}

#[test]
fn nested_fixed_array_patterns_check_lengths_and_preserve_nominal_elements() {
    for source in [
        "fn bad((int[2], bool) value) { if value { ([_], _) -> {} _ -> {} } }",
        "enum Choice { Pair(int[2]) };fn bad(Choice value) { if value { Choice.Pair([_]) -> {} _ -> {} } }",
        "struct Record { int[2] values };fn bad(Record value) { if value { Record{.values = [_]} -> {} _ -> {} } }",
        "type Number = int;fn bad(Number[2] value) { if value { [_] -> {} _ -> {} } }",
    ] {
        rejects(source, "fixed-size array pattern has wrong length");
    }
    accepts(
        "type Number = int;fn pair(Number[2] value) { if value { [a, b] -> { Number first = a;Number second = b } } };pair([1, 2])",
    );
}

#[test]
fn new_pattern_and_return_diagnostics_preserve_source_locations() {
    for (source, expected) in [
        (
            "fn bad(int[2] value) { if value { [_] -> {} _ -> {} } }",
            "if value { [_] -> {} _ -> {} }",
        ),
        (
            "fn fail() int! { throw \"failure\" };fn bad() error { _ = fail() catch err { return err };while true {} }",
            "return err",
        ),
    ] {
        for release in [false, true] {
            let path = Path::new("negative_patterns_errors.nc");
            let error = ncc::compile_source_with_options(source, path, release).unwrap_err();
            let diagnostic = &error.0[0];
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
            assert_eq!(&source[diagnostic.span.clone()], expected, "{error}");
        }
    }
}

#[test]
fn throw_operands_must_be_strings_or_caught_errors() {
    for (operand, actual) in [
        ("true", "bool"),
        ("1", "int"),
        ("1u", "uint"),
        ("1.5", "float"),
        ("'x'", "char"),
        ("[1, 2]", "int[2]"),
        ("(1, true)", "(int, bool)"),
    ] {
        rejects(
            &format!("fn bad() int! {{ throw {operand} }}"),
            &format!("expected `str`, found `{actual}`"),
        );
    }
    rejects(
        "fn bad(str? message) int! { throw message }",
        "expected `str`, found `str?`",
    );
    rejects(
        "fn bad(int! result) int! { throw result }",
        "expected `str`, found `int!`",
    );
    accepts("fn fail(str message) void! { throw message };fail(\"failure\") catch _ {}");
}

#[test]
fn nonthrowing_void_and_anonymous_functions_cannot_throw() {
    for source in [
        "fn bad() { throw \"failure\" }",
        "fn bad() void { throw \"failure\" }",
        "(fn() void) bad = fn() { throw \"failure\" }",
        "fn fail() int! { throw \"failure\" };fn bad() { _ = fail() catch err { throw err } }",
    ] {
        rejects(source, "throw requires a throwing function");
    }
    accepts(
        "fn fail() void! { throw \"failure\" };(fn() void!) callback = fn() void! { throw \"failure\" };fail() catch _ {};callback() catch _ {}",
    );
}
