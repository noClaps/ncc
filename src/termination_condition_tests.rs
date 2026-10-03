use super::condition_loop;
use crate::ast::{Block, Item, Stmt};

fn block(source: &str) -> Block {
    let wrapped = format!("while true {{\n{source}\n}}");
    let module = crate::parser::parse(crate::lexer::lex(&wrapped).unwrap()).unwrap();
    let Item::Statement(statement) = module.items.into_iter().next().unwrap() else {
        panic!("expected statement");
    };
    let Stmt::While { body, .. } = statement.unlocated() else {
        panic!("expected wrapper loop");
    };
    body.clone()
}

fn certificate(
    comparison: &str,
    prefix: &str,
    body: &str,
    label: Option<&str>,
    initial: (i128, i128, i128),
    bound: i128,
) -> bool {
    let source = block(&format!("while {comparison} {{}}"));
    let Stmt::While { condition, .. } = source.statements[0].unlocated() else {
        panic!("expected loop");
    };
    condition_loop(
        condition,
        &block(prefix),
        &block(body),
        label,
        initial,
        bound,
    )
}

#[test]
fn conditional_prefix_and_body_strides() {
    let prefix = "if flag { true -> { i = i + 1 } false -> { i = i + 2 } }";
    for body in ["", "continue", "continue :outer", "i = i + 2\ncontinue"] {
        assert!(certificate(
            "i < b",
            prefix,
            body,
            Some("outer"),
            (0, 0, 100),
            10
        ));
    }
    let optional = "if flag { true -> { i = i + 1 } false -> {} }";
    assert!(certificate(
        "i <= b",
        optional,
        "i = i + 1\ncontinue",
        None,
        (0, 0, 100),
        10
    ));
    assert!(!certificate(
        "i < b",
        optional,
        "continue",
        None,
        (0, 0, 100),
        10
    ));
    assert!(certificate(
        "i > b",
        "if flag { true -> { i = i - 1 } false -> { i = i - 2 } }",
        "continue",
        None,
        (10, -100, 100),
        0
    ));
    assert!(!certificate(
        "i > b",
        "if flag { true -> { i = i - 1 } false -> { i = i - 2 } }",
        "continue",
        None,
        (10, 0, 100),
        0
    ));
}

#[test]
fn first_and_final_prefix_overflow() {
    assert!(!certificate(
        "i < b",
        "i = i + 1",
        "break",
        None,
        (100, 0, 100),
        100
    ));
    assert!(!certificate(
        "i <= b",
        "i = i + 1",
        "continue",
        None,
        (98, 0, 100),
        100
    ));
    assert!(certificate(
        "i <= b",
        "i = i + 1",
        "break",
        None,
        (98, 0, 100),
        100
    ));
    assert!(!certificate(
        "i >= b",
        "i = i - 1",
        "continue",
        None,
        (2, 0, 100),
        0
    ));
    let prefix = "if flag { true -> { i = i + 1 } false -> { i = i + 2 } }";
    assert!(!certificate(
        "i < b",
        prefix,
        "continue",
        None,
        (98, 0, 100),
        100
    ));
    assert!(certificate(
        "i < b",
        prefix,
        "break",
        None,
        (98, 0, 100),
        100
    ));
}

#[test]
fn continue_paths_do_not_skip_or_duplicate_prefixes() {
    assert!(certificate(
        "i != b",
        "i = i + 1",
        "continue\ni = i + 10",
        None,
        (0, 0, 10),
        10
    ));
    assert!(!certificate(
        "i != b",
        "i = i + 2",
        "continue\ni = i + 1",
        None,
        (0, 0, 100),
        9
    ));
    let body = "if flag { true -> { continue } false -> { i = i + 1 } }";
    assert!(certificate(
        "i < b",
        "i = i + 1",
        body,
        None,
        (0, 0, 100),
        10
    ));
    assert!(!certificate(
        "i != b",
        "i = i + 1",
        body,
        None,
        (0, 0, 100),
        10
    ));
}

#[test]
fn labels_consume_only_their_own_jumps() {
    assert!(certificate(
        "i <= b",
        "i = i + 1",
        "continue :outside",
        Some("outer"),
        (98, 0, 100),
        100
    ));
    assert!(!certificate(
        "i <= b",
        "i = i + 1",
        "continue :outer",
        Some("outer"),
        (98, 0, 100),
        100
    ));
    let body = "inner: if flag { true -> { break :inner } false -> {} }\ncontinue :outer";
    assert!(certificate(
        "i < b",
        "i = i + 1",
        body,
        Some("outer"),
        (0, 0, 100),
        10
    ));
    let body = "inner: if flag { true -> { break :outer } false -> { continue :outer } }";
    assert!(certificate(
        "i < b",
        "i = i + 1",
        body,
        Some("outer"),
        (0, 0, 100),
        10
    ));
}

#[test]
fn nested_loops_are_rank_neutral_and_preserve_outward_exits() {
    for nested in [
        "inner: while j < n { j = j + 1\ncontinue }",
        "inner: while j < n { continue :outer }",
        "inner: for key in values { continue :inner }",
    ] {
        assert!(certificate(
            "i < b",
            "i = i + 1",
            nested,
            Some("outer"),
            (0, 0, 100),
            10
        ));
    }
    assert!(!certificate(
        "i <= b",
        "i = i + 1",
        "while j < n { continue :outer }",
        Some("outer"),
        (98, 0, 100),
        100
    ));
    // The zero-iteration nested path can still reach the next prefix.
    assert!(!certificate(
        "i <= b",
        "i = i + 1",
        "while j < n { break :outer }",
        Some("outer"),
        (98, 0, 100),
        100
    ));
    for body in [
        "while j < n { i = i + 1 }",
        "while j < n { b = b + 1 }",
        "while j < n { i = 0\nbreak }",
    ] {
        assert!(!certificate(
            "i < b",
            "i = i + 1",
            body,
            Some("outer"),
            (0, 0, 100),
            10
        ));
    }
}

#[test]
fn large_horizons_and_exiting_path_excursions_are_proved_without_execution() {
    let prefix = "if flag { true -> { i = i + 1 } false -> { i = i + 2 } }";
    assert!(certificate(
        "i < b",
        prefix,
        "continue",
        None,
        (0, 0, 1_000_000_000_001),
        1_000_000_000_000,
    ));
    assert!(!certificate(
        "i < b",
        prefix,
        "continue",
        None,
        (0, 0, 1_000_000_000_000),
        1_000_000_000_000,
    ));
    for exit in ["break", "continue :outside"] {
        let body =
            format!("if flag {{ true -> {{ i = i + 99\n{exit} }} false -> {{ continue }} }}");
        assert!(!certificate(
            "i < b",
            "i = i + 1",
            &body,
            Some("outer"),
            (0, 0, 100),
            10,
        ));
        assert!(certificate(
            "i < b",
            "i = i + 1",
            &body,
            Some("outer"),
            (0, 0, 108),
            10,
        ));
    }
}

#[test]
fn unsupported_updates_and_nonexact_inequality_remain_unproven() {
    for prefix in [
        "i = 0",
        "b = b + 1",
        "i = i * 2",
        "i = i + step",
        "i = i + 2\ni = i - 1",
    ] {
        assert!(!certificate(
            "i < b",
            prefix,
            "continue",
            None,
            (1, 0, 100),
            10
        ));
    }
    let prefix = "if flag { true -> { i = i + 1 } false -> { i = i + 2 } }";
    assert!(!certificate(
        "i != b",
        prefix,
        "continue",
        None,
        (0, 0, 100),
        10
    ));
    let exact = "if flag { true -> { i = i + 1\ni = i + 1 } false -> { i = i + 2 } }";
    assert!(certificate(
        "i != b",
        exact,
        "continue",
        None,
        (0, 0, 100),
        10
    ));
    assert!(!certificate(
        "i != b",
        exact,
        "continue",
        None,
        (0, 0, 100),
        9
    ));
}
