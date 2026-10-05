use std::path::Path;

fn rejects(source: &str, expected: &str) {
    let mut failures = Vec::new();
    for release in [false, true] {
        match ncc::compile_test_source_with_options(
            source,
            Path::new("negative_nominal_interactions.nc"),
            release,
        ) {
            Ok(_) => failures.push(format!(
                "release={release}, expected {expected:?}, but compilation succeeded\n{source}"
            )),
            Err(error) if !error.to_string().contains(expected) => failures.push(format!(
                "release={release}, expected {expected:?}\n{source}\ndiagnostic:\n{error}"
            )),
            Err(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn accepts(source: &str) {
    for release in [false, true] {
        ncc::compile_test_source_with_options(
            source,
            Path::new("nominal_interactions_control.nc"),
            release,
        )
        .unwrap_or_else(|error| panic!("release={release}\n{source}\ndiagnostic:\n{error}"));
    }
}

fn scalar_cases(statements: &[&str]) {
    for (underlying, initial) in [("int", "7"), ("str", "\"same\""), ("int[]", "[7]")] {
        for statement in statements {
            let prefix = format!(
                "type A = {underlying}\ntype B = {underlying}\n\
                 test \"nominal payload\" {{ A first = {initial}; B second = {initial}; "
            );
            let expected = if statement.contains("B value") {
                "expected `B`, found `A`"
            } else {
                "expected `A`, found `B`"
            };
            rejects(&format!("{prefix}{statement} }}"), expected);
            let control = statement
                .replace("second", "first")
                .replace("B value", "A value");
            accepts(&format!("{prefix}{control} }}"));
        }
    }
}

#[test]
fn container_literals_and_indexed_writes_preserve_nominal_element_identity() {
    scalar_cases(&[
        "A[] values = [first, second]",
        "A[2] values = [first, second]",
        "A[][] values = [[first], [second]]",
        "[str]A values = [\"ok\": first, \"bad\": second]",
        "[str]A[] values = [\"bad\": [second]]",
        "(int, A) values = (1, second)",
        "mut A[] values = [first]; values[0] = second",
        "mut A[1] values = [first]; values[0] = second",
        "mut [str]A values = [\"key\": first]; values[\"key\"] = second",
        "mut (int, A) values = (1, first); values[1] = second",
        "mut [str](A[], int) values = [\"key\": ([first], 1)]; values[\"key\"][0][0] = second",
    ]);
}

#[test]
fn container_aliases_do_not_implicitly_unwrap_or_exchange_identity() {
    for (underlying, initial) in [
        ("int[]", "[7]"),
        ("int[1]", "[7]"),
        ("[str]int", "[\"key\": 7]"),
        ("(int, str)", "(7, \"same\")"),
    ] {
        let prefix = format!(
            "type A = {underlying}\ntype B = {underlying}\n\
             test \"nominal container\" {{ A first = {initial}; "
        );
        for (statement, expected) in [
            (
                "B target = first".to_owned(),
                "expected `B`, found `A`".to_owned(),
            ),
            (
                format!("{underlying} target = first"),
                format!("expected `{underlying}`, found `A`"),
            ),
            (
                format!("{underlying} raw = {initial}; A target = raw"),
                format!("expected `A`, found `{underlying}`"),
            ),
        ] {
            rejects(&format!("{prefix}{statement} }}"), &expected);
        }
        accepts(&format!(
            "{prefix}{underlying} raw = @as({underlying}, first); \
             B target = @as(B, raw); A copy = first }}"
        ));
    }
}

#[test]
fn optional_and_error_success_promotions_require_the_nominal_payload() {
    scalar_cases(&[
        "A? value = second",
        "A! value = second",
        "A?[] values = [first, second]",
        "[str]A? values = [\"key\": second]",
        "fn take(A? value) {}; take(second)",
        "fn take(A! value) {}; take(second)",
        "fn wrong() A? { return second }; _ = wrong()",
        "fn wrong() A! { return second }; _ = wrong()",
        "mut A? value = first; value = second",
        "mut A! value = first; value = second",
    ]);
}

#[test]
fn already_wrapped_payloads_remain_distinct_in_bindings_calls_and_returns() {
    for wrapper in ["?", "!", "[]?", "[]!", "?[]"] {
        let a = format!("A{wrapper}");
        let b = format!("B{wrapper}");
        let initial = if wrapper.contains("[]") { "[7]" } else { "7" };
        let prefix = format!(
            "type A = int\ntype B = int\n\
             test \"wrapped nominal identity\" {{ {b} source = {initial}; "
        );
        for statement in [
            format!("{a} target = source"),
            format!("mut {a} target = {initial}; target = source"),
            format!("fn take({a} value) {{}}; take(source)"),
            format!("fn wrong() {a} {{ return source }}; _ = wrong()"),
        ] {
            // Array compatibility diagnoses its elements rather than the outer array.
            let diagnosed = if wrapper == "?[]" { "B?" } else { &b };
            rejects(
                &format!("{prefix}{statement} }}"),
                &format!("found `{diagnosed}`"),
            );
            accepts(&format!("{prefix}{} }}", statement.replace('A', "B")));
        }
    }
}

#[test]
fn optional_fallback_catch_and_try_preserve_unwrapped_nominal_identity() {
    scalar_cases(&[
        "A? pending = none; A value = pending else second",
        "A? pending = first; B value = pending else first",
        "A! pending = first; A value = pending catch _ { break second }",
        "A! pending = first; B value = pending catch _ { break first }",
        "A! pending = first; B value = try pending",
    ]);
}

#[test]
fn closure_parameters_and_results_require_exact_nominal_signatures() {
    for (expected, actual, body) in [
        ("(fn(A) A)", "(fn(B) A)", "fn(B value) A { return first }"),
        ("(fn(A) A)", "(fn(A) B)", "fn(A value) B { return second }"),
        (
            "(fn(A[]) A[])",
            "(fn(B[]) A[])",
            "fn(B[] value) A[] { return [first] }",
        ),
        (
            "(fn(A?) A!)",
            "(fn(A?) B!)",
            "fn(A? value) B! { return second }",
        ),
    ] {
        let prefix = format!(
            "type A = int\ntype B = int\n\
             test \"closure signature\" {{ A first = 7; B second = 7; \
             {actual} callback = {body}; "
        );
        for statement in [
            format!("{expected} target = callback"),
            format!("fn take({expected} operation) {{}}; take(callback)"),
            format!("fn factory() {expected} {{ return callback }}; _ = factory()"),
            format!("{expected}[] values = [callback]"),
            format!("[str]{expected} values = [\"key\": callback]"),
            format!("(int, {expected}) pair = (1, callback)"),
        ] {
            rejects(
                &format!("{prefix}{statement} }}"),
                &format!("expected `{expected}`, found `{actual}`"),
            );
            accepts(&format!(
                "{prefix}{} }}",
                statement.replace(expected, actual)
            ));
        }
    }
}

#[test]
fn invoking_captured_closures_does_not_erase_nominal_arguments_or_results() {
    scalar_cases(&[
        "fn callback = fn(A value) A { return first }; _ = callback(second)",
        "fn callback = fn() A { return first }; B value = callback()",
        "fn callback = fn(A[] values) A { return first }; _ = callback([second])",
        "fn callback = fn(A? value) A { return first }; _ = callback(second)",
        "fn callback = fn() A! { return first }; B value = try callback()",
    ]);
}
