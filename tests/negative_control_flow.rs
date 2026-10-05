use std::path::Path;

fn rejects(source: &str, expected: &str) {
    let mut failures = Vec::new();
    for release in [false, true] {
        match ncc::compile_source_with_options(
            source,
            Path::new("negative_control_flow.nc"),
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
        ncc::compile_source_with_options(source, Path::new("control_flow_control.nc"), release)
            .unwrap_or_else(|error| {
                panic!("release={release}\nsource:\n{source}\ndiagnostic:\n{error}")
            });
    }
}

#[test]
fn every_reachable_value_branch_must_supply_a_value() {
    for arm in [
        "{}",
        "{ int local = 1 }",
        "{ while false {} }",
        "{ while true { break } }",
        "{ for index in [1] { break } }",
        "{ done: while true { break :done } }",
    ] {
        for subject in ["true", "false"] {
            rejects(
                &format!("int value = if {subject} {{ true -> {{ break 1 }} false -> {arm} }}"),
                "value-producing branch must provide a value",
            );
        }
    }
    accepts(
        "fn choose(bool flag) int { return if flag { true -> { break 1 } false -> { while true { break };break 2 } } };_ = choose(true);_ = choose(false)",
    );
    accepts(
        "fn choose(bool flag) int { int value = if flag { true -> { return 1 } false -> { break 2 } };return value };_ = choose(false)",
    );
}

#[test]
fn conditional_values_reject_incompatible_branches_and_nonoptional_none() {
    for source in [
        "int value = if true { true -> { break 1 } false -> { break false } }",
        "int value = if false { true -> { 1 } false -> { false } }",
        "fn choose(bool flag) int { return if flag { true -> { break 1 } false -> { break false } } }",
    ] {
        rejects(source, "expected `int`, found `bool`");
    }
    for source in [
        "int value = if true { true -> { break 1 } false -> { break none } }",
        "int value = if false { true -> { break none } false -> { break 1 } }",
    ] {
        rejects(source, "cannot infer type of none");
    }
    accepts(
        "fn choose(bool flag) int? { return if flag { true -> { break 1 } false -> { break none } } };int? present = choose(true);int? absent = choose(false)",
    );
}

#[test]
fn value_break_operands_are_checked_through_nested_control_flow() {
    for body in [
        "while true { break false }",
        "for index in [1] { break false };break 1",
        "if false { true -> { break false } false -> { break 1 } }",
        "{ { break false } }",
    ] {
        rejects(
            &format!("int value = if true {{ true -> {{ {body} }} false -> {{ break 2 }} }}"),
            "expected `int`, found `bool`",
        );
    }
    accepts("int value = if true { true -> { while true { break 1 } } false -> { break 2 } }");
    accepts(
        "int value = if false { true -> { for index in [1] { break 1 };break 2 } false -> { { { break 3 } } } }",
    );
}

#[test]
fn bare_if_predicates_and_wrapped_while_conditions_require_booleans() {
    for (predicate, actual) in [
        ("1", "int"),
        ("1u", "uint"),
        ("1.5", "float"),
        ("\"true\"", "str"),
        ("'t'", "char"),
    ] {
        rejects(
            &format!("if {{ {predicate} -> {{}} _ -> {{}} }}"),
            &format!("expected `bool`, found `{actual}`"),
        );
    }
    rejects(
        "bool? flag = true;while flag {}",
        "expected `bool`, found `bool?`",
    );
    rejects(
        "fn condition() bool! { return false };bool! flag = condition();while flag {}",
        "expected `bool`, found `bool!`",
    );
    accepts(
        "if { 1 < 2 -> {} _ -> {} };bool? flag = false;while flag else false {};fn condition() bool! { return false };while condition() catch _ { break false } {}",
    );
}

#[test]
fn optional_and_error_containers_require_unwrapping_before_iteration() {
    for source in [
        "int[]? values = [1];for index in values {}",
        "str? value = \"a\";for index in value {}",
        "int[1]? values = [1];for index in values {}",
        "fn values() int[]! { return [1] };int[]! pending = values();for index in pending {}",
        "fn text() str! { return \"a\" };str! pending = text();for index in pending {}",
    ] {
        rejects(source, "for loop expects an array, map, or str");
    }
    accepts(
        "int[]? values = [1];for index in values else [] { uint position = index };str? text = \"a\";for index in text else \"\" { uint position = index };fn get() int[]! { return [1] };for index in get() catch _ { break [] } { uint position = index }",
    );
}

#[test]
fn bare_jumps_need_a_loop_not_just_a_block_or_conditional() {
    for jump in ["break", "continue"] {
        for source in [
            jump.to_owned(),
            format!("{{ {{ {jump} }} }}"),
            format!("if false {{ true -> {{ {jump} }} false -> {{}} }}"),
            format!("fn bad() {{ {jump} }}"),
        ] {
            rejects(&source, "no valid target");
        }
        accepts(&format!(
            "for index in [1] {{ {{ {jump} }} }};while false {{ if true {{ true -> {{ {jump} }} false -> {{}} }} }}"
        ));
    }
}

#[test]
fn labels_are_unavailable_after_their_target_or_in_sibling_scopes() {
    for jump in ["break", "continue"] {
        for source in [
            format!("for index in [1] {{ {jump} :later }};later: while false {{}}"),
            format!("finished: for index in [1] {{}};while false {{ {jump} :finished }}"),
            format!(
                "if true {{ true -> {{ sibling: while false {{}} }} false -> {{ while false {{ {jump} :sibling }} }} }}"
            ),
        ] {
            rejects(&source, "no valid target");
        }
        accepts(&format!(
            "outer: for index in [1] {{ while true {{ {jump} :outer }} }}"
        ));
    }
    rejects(
        "done: if true { true -> { while false { continue :done } } false -> {} }",
        "no valid target",
    );
    accepts("done: if true { true -> { while true { break :done } } false -> {} }");
}

#[test]
fn named_and_anonymous_functions_cannot_jump_to_enclosing_loops() {
    for jump in ["break", "continue", "break :outer", "continue :outer"] {
        for declaration in [
            format!("fn nested() {{ {jump} }}"),
            format!("(fn() void) nested = fn() {{ {jump} }}"),
        ] {
            rejects(
                &format!("outer: for index in [1] {{ {declaration} }}"),
                "no valid target",
            );
        }
    }
    accepts(
        "outer: for index in [1] { fn nested() { inner: while true { break :inner } };(fn() void) callback = fn() { inner: for item in [1] { continue :inner } };nested();callback();break :outer }",
    );
}

#[test]
fn anonymous_functions_cannot_use_enclosing_value_break_targets() {
    for body in ["break 1", "while true { break 1 }"] {
        rejects(
            &format!(
                "int value = if true {{ true -> {{ (fn() void) callback = fn() {{ {body} }};break 1 }} false -> {{ break 2 }} }}"
            ),
            "break with a value requires a value-producing",
        );
    }
    accepts(
        "int value = if true { true -> { (fn() int) callback = fn() int { return if true { true -> { break 1 } false -> { break 2 } } };break callback() } false -> { break 3 } }",
    );
}

#[test]
fn bare_returns_require_void_results_and_return_requires_a_function() {
    for source in [
        "return",
        "return 1",
        "while false { return 1 }",
        "if false { true -> { return } false -> {} }",
    ] {
        rejects(source, "return outside function");
    }
    for source in [
        "fn bad() int { return }",
        "fn bad() int! { return }",
        "(fn() int) bad = fn() int { return }",
    ] {
        rejects(source, "expected `int`, found `void`");
    }
    accepts(
        "fn plain() { return };fn explicit() void { return };fn throwing() void! { return };(fn() void) callback = fn() { return };plain();explicit();throwing() catch _ {};callback()",
    );
}

#[test]
fn zero_trip_loops_and_unreachable_returns_do_not_complete_return_paths() {
    for body in [
        "int[] values = [];for index in values { return 1 }",
        "while false { return 1 }",
        "while true { break;return 1 }",
        "for index in [1] { continue;return 1 }",
        "outer: while true { for index in [1] { break :outer;return 1 } }",
        "outer: for index in [1] { while true { continue :outer;return 1 } }",
    ] {
        for declaration in [
            format!("fn bad() int {{ {body} }}"),
            format!("(fn() int) bad = fn() int {{ {body} }}"),
        ] {
            rejects(&declaration, "may finish without returning");
        }
        accepts(&format!("fn good() int {{ {body};return 2 }}"));
    }
    // Infinite loops warn but do not reject void functions; never execute this control.
    accepts("fn diverges() { while true { continue } }");
}

#[test]
fn invalid_jump_and_return_diagnostics_identify_the_original_statement() {
    for (source, statement, expected) in [
        (
            "while false {\n    break :missing\n}",
            "break :missing",
            "no valid target",
        ),
        (
            "for index in [1] {\n    fn nested() { continue }\n}",
            "continue",
            "no valid target",
        ),
        (
            "if false {\n    true -> { return 1 }\n    false -> {}\n}",
            "return 1",
            "return outside function",
        ),
        (
            "fn bad() int {\n    return\n}",
            "return",
            "expected `int`, found `void`",
        ),
    ] {
        for release in [false, true] {
            let path = Path::new("negative_control_flow.nc");
            let error = ncc::compile_source_with_options(source, path, release)
                .expect_err("invalid control flow must not compile");
            let diagnostic = &error.0[0];
            assert!(
                diagnostic.message.contains(expected),
                "release={release}, expected {expected:?}\n{error}"
            );
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
            assert_eq!(&source[diagnostic.span.clone()], statement, "{error}");
        }
    }
}
