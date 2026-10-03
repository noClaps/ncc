use super::affine_loop;
use crate::ast::{Item, Stmt};
use std::collections::HashMap;

fn certificate(source: &str, values: &[(&str, (i128, i128, i128))]) -> bool {
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let Item::Statement(statement) = &module.items[0] else {
        panic!("expected statement")
    };
    let Stmt::While {
        condition, body, ..
    } = statement.unlocated()
    else {
        panic!("expected loop")
    };
    let values = values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), *value))
        .collect::<HashMap<_, _>>();
    affine_loop(condition, body, &values)
}

fn integers<'a>(values: &[(&'a str, i128)]) -> Vec<(&'a str, (i128, i128, i128))> {
    values
        .iter()
        .map(|&(name, initial)| (name, (initial, -100_000, 100_000)))
        .collect()
}

#[test]
fn compound_bounds_and_arbitrary_binding_count() {
    let values = integers(&[("i", 0), ("n", 10), ("extra", 3)]);
    assert!(certificate(
        "while i < 2*n + extra { i = i + 3\nn = n + 1 }",
        &values
    ));
    let values = integers(&[("i", 0), ("n", 10), ("m", 8), ("extra", 3)]);
    assert!(certificate(
        "while i + m < 2*n + extra { i = i + 3\nn = n + 1\nm = m + 1 }",
        &values
    ));
    assert!(certificate(
        "while 2*n + extra > i + m { i = i + 3\nn = n + 1\nm = m + 1 }",
        &values
    ));
    assert!(!certificate(
        "while i + m < 2*n + extra { i = i + 1\nn = n + 1 }",
        &values
    ));
}

#[test]
fn invariant_steps_and_checked_constant_algebra() {
    let values = integers(&[("i", 0), ("n", 10), ("step", 3)]);
    assert!(certificate(
        "while i < n { i = i + step\nn = n + 1 }",
        &values
    ));
    assert!(certificate(
        "while i < n { i = i + (2*3 - 3)\nn = n + 1 }",
        &values
    ));
    assert!(certificate(
        "while i < n { i = step + i\nn = n + 1 }",
        &values
    ));
    assert!(!certificate(
        "while i < n { i = i + step\nstep = step + 1 }",
        &values
    ));
    assert!(!certificate("while i < n { i = i + unknown }", &values));
}

#[test]
fn correlated_branches_and_continues() {
    let values = integers(&[("i", 0), ("n", 10)]);
    let body = "if flag { true -> { i = i + 2\nn = n + 1\ncontinue }
                            false -> { i = i + 11\nn = n + 10 } }";
    for op in ["<", "<=", "!="] {
        assert!(certificate(
            &format!("while i {op} n {{ {body} }}"),
            &values
        ));
    }
    assert!(!certificate(
        "while i < n { if flag { true -> { continue } false -> { i = i + 1 } } }",
        &values
    ));
    assert!(!certificate(
        "while i != n { if flag { true -> { i = i + 2 } false -> { i = i + 3 } } }",
        &values
    ));
}

#[test]
fn exact_closing_and_inclusive_horizons() {
    let values = integers(&[("i", 0), ("n", 10)]);
    assert!(certificate(
        "while i != n { i = i + 3\nn = n + 1 }",
        &values
    ));
    let values = integers(&[("i", 0), ("n", 9)]);
    assert!(!certificate(
        "while i != n { i = i + 3\nn = n + 1 }",
        &values
    ));
    let values = [("i", (0, 0, 10)), ("n", (10, 0, 10))];
    assert!(certificate("while i < n { i = i + 1 }", &values));
    assert!(!certificate("while i <= n { i = i + 1 }", &values));
    let values = [("i", (10, 0, 10)), ("n", (0, 0, 10))];
    assert!(certificate("while i > n { i = i - 1 }", &values));
    assert!(!certificate("while i >= n { i = i - 1 }", &values));
}

#[test]
fn excursions_on_every_exit_and_unrelated_binding() {
    let values = [
        ("i", (0, -100, 100)),
        ("n", (10, -100, 100)),
        ("m", (90, -100, 100)),
    ];
    for exit in ["break", "continue", "return"] {
        assert!(!certificate(
            &format!(
                "while i < n {{ if flag {{ true -> {{ m = m + 20\n{exit} }} false -> {{ i = i + 1 }} }} }}"
            ),
            &values
        ));
    }
    assert!(!certificate(
        "while i < n { i = i + 100\ni = i - 99 }",
        &values
    ));
    assert!(certificate("while i < n { break }", &values));
}

#[test]
fn arithmetic_checks_keep_canceled_intermediates() {
    let values = [("i", (0, -100, 100)), ("n", (60, -100, 100))];
    assert!(!certificate("while i < (2*n - n) { i = i + 1 }", &values));
    assert!(!certificate("while i < n { i = (i + 100) - 99 }", &values));
    assert!(!certificate(
        "while i < n { @println(2*n - n)\ni = i + 1 }",
        &values
    ));
    assert!(!certificate(
        "while i < n { if 2*n - n > 0 { true -> { i = i + 1 } false -> { break } } }",
        &values
    ));
}

#[test]
fn final_false_condition_arithmetic_is_checked() {
    let values = [("i", (0, -100, 100)), ("n", (40, -100, 100))];
    // One iteration closes the rank, but the next condition computes 120.
    assert!(!certificate(
        "while i < 2*n { i = i + 100\nn = n + 20 }",
        &values
    ));
    assert!(certificate("while i < 2*n { i = i + 80 }", &values));
}

#[test]
fn distinct_numeric_domains_and_unsigned_underflow() {
    let values = [("i", (0, 0, 255)), ("n", (10, 0, 255))];
    assert!(certificate("while i < n { i = i + 1 }", &values));
    assert!(!certificate("while i < n { i = i - 1 }", &values));
    assert!(!certificate("while i < n { i = i + (256 - 255) }", &values));
    let values = [("i", (0, 0, 255)), ("n", (10, 0, i128::from(u64::MAX)))];
    assert!(!certificate("while i < n { i = i + 1 }", &values));
}

#[test]
fn unsupported_effects_never_hide_arithmetic_or_calls() {
    let values = integers(&[("i", 0), ("n", 10)]);
    for body in [
        "unknown()\ni = i + 1",
        "@println(unknown())\ni = i + 1",
        "while false {}\ni = i + 1",
        "for j in data {}\ni = i + 1",
        "i = int(i + 1)",
        "i = i * i",
        "i = 1",
        "i = i / 1",
        "mut int i = 2\ni = i + 1",
        "i = if flag { true -> { 1 } false -> { 2 } }",
    ] {
        assert!(
            !certificate(&format!("while i < n {{ {body} }}"), &values),
            "{body}"
        );
    }
    assert!(certificate(
        "while i < n { @println(i)\ni = i + 1 }",
        &values
    ));
    assert!(certificate(
        "while i < n { mut int local = 1\ni = i + 1 }",
        &values
    ));
}

#[test]
fn labeled_conditionals_preserve_break_scope() {
    let values = integers(&[("i", 0), ("n", 10)]);
    assert!(certificate(
        "while i < n { chosen: if flag { true -> { break :chosen } false -> {} }\ni = i + 1 }",
        &values
    ));
    assert!(!certificate(
        "while i < n { chosen: if flag { true -> { break :chosen } false -> { i = i + 1 } } }",
        &values
    ));
}

#[test]
fn large_horizons_need_no_execution_or_fuel() {
    let values = [
        ("i", (0, 0, i128::from(i64::MAX))),
        ("n", (1_000_000_000_000, 0, i128::from(i64::MAX))),
    ];
    assert!(certificate("while i < n { i = i + 1 }", &values));
    let values = [
        ("i", (i128::MIN, i128::MIN, i128::MAX)),
        ("n", (i128::MAX, i128::MIN, i128::MAX)),
    ];
    assert!(!certificate("while i < n { i = i + 1 }", &values));
}

#[test]
fn empty_execution_still_checks_condition() {
    let values = integers(&[("i", 20), ("n", 10)]);
    assert!(certificate("while i < n { unknown() }", &values));
    let values = [("i", (100, -100, 100)), ("n", (60, -100, 100))];
    assert!(!certificate("while i > 2*n - n { unknown() }", &values));
}

#[test]
fn unary_negative_literals_and_uint_constants() {
    let signed = [("i", (0, i128::from(i64::MIN), i128::from(i64::MAX)))];
    assert!(certificate("while i > -10 { i = i + -2 }", &signed));
    assert!(certificate("while -10 < i { i = i - (2*3 - 4) }", &signed));
    assert!(certificate(
        "while i > -9223372036854775808 { break }",
        &signed
    ));
    assert!(!certificate(
        "while i > -9223372036854775808 { i = i - 3 }",
        &signed
    ));
    let unsigned = [("i", (0, 0, i128::from(u64::MAX)))];
    assert!(certificate("while i < 10u { i = i + 2u }", &unsigned));
    assert!(!certificate(
        "while i < 10u { i = i + (18446744073709551615u + 1u) }",
        &unsigned
    ));
}

#[test]
fn rank_cancellation_does_not_erase_binding_obligations() {
    let values = [
        ("i", (0, -100, 100)),
        ("n", (10, -100, 100)),
        ("m", (90, -100, 100)),
    ];
    assert!(!certificate(
        "while i < n + (m - m) { i = i + 1\nm = m + 2 }",
        &values
    ));
    assert!(certificate("while i < n + (m - m) { i = i + 1 }", &values));
}

#[test]
fn every_continuing_path_must_close_the_rank() {
    let values = integers(&[("i", 0), ("n", 10)]);
    assert!(!certificate(
        "while i < n { if flag { true -> { i = i + 1\nn = n + 2 } false -> { i = i + 2\nn = n + 1 } } }",
        &values
    ));
    assert!(certificate(
        "while i < n { if flag { true -> { return } false -> { i = i + 1\ncontinue :outer } } }",
        &values
    ));
}

#[test]
fn incomplete_conditionals_preserve_unmatched_progress_obligations() {
    let values = integers(&[("i", 0), ("n", 10)]);
    for body in [
        "if { i > 0 -> { i = i + 1 } }",
        "if { i > 0 -> { continue } }",
        "if { false -> { i = i + 1 } }",
        "if flag { true -> { i = i + 1 } }",
        "if flag { false -> { i = i + 1 } }",
        "if i { 0 -> { i = i + 1 } }",
        "if {}",
    ] {
        assert!(
            !certificate(&format!("while i < n {{ {body} }}"), &values),
            "{body}"
        );
    }
    assert!(certificate(
        "while i < n { if { i > 0 -> { @println(i) } }\ni = i + 1 }",
        &values
    ));
}

#[test]
fn exhaustive_conditionals_keep_correlated_rank_progress() {
    let values = integers(&[("i", 0), ("n", 10)]);
    for body in [
        "if { i > 0 -> { i = i + 2\nn = n + 1 } _ -> { i = i + 11\nn = n + 10 } }",
        "if flag { true -> { i = i + 2\nn = n + 1 } false -> { i = i + 11\nn = n + 10 } }",
        "if flag { true, false -> { i = i + 2\nn = n + 1 } }",
        "if flag { value -> { i = i + 2\nn = n + 1 } }",
        "if { true -> { i = i + 2\nn = n + 1 } }",
    ] {
        assert!(
            certificate(&format!("while i != n {{ {body} }}"), &values),
            "{body}"
        );
    }
}

#[test]
fn independent_signed_arithmetic_never_inherits_unsigned_range() {
    let values = [("i", (0, 0, i128::from(u64::MAX)))];
    let overflow = "(9223372036854775807 + 1) - 9223372036854775807";
    assert!(!certificate(
        &format!("while i < 10u {{ i = i + ({overflow}) }}"),
        &values
    ));
    assert!(!certificate(
        &format!("while i < 10u + ({overflow}) {{ i = i + 1u }}"),
        &values
    ));
    assert!(!certificate(
        &format!("while i < 10u {{ @println(i + ({overflow}))\ni = i + 1u }}"),
        &values
    ));
    assert!(certificate(
        "while i < 10u { i = i + (2u*3u - 4u) }",
        &values
    ));
    // The ordinary signed subtree stays supported when its arithmetic is safe.
    let signed = [("i", (0, i128::from(i64::MIN), i128::from(i64::MAX)))];
    assert!(certificate("while i < 10 { i = i + (2*3 - 4) }", &signed));
}

#[test]
fn modulus_guards_preserve_correlated_progress() {
    let values = integers(&[("i", 0), ("n", 10), ("extra", 3)]);
    assert!(certificate(
        "while i < 2*n + extra { if i%2 == 0 { true -> { i = i + 3\nn = n + 1\ncontinue } false -> { i = i + 21\nn = n + 10 } } }",
        &values
    ));
    assert!(!certificate(
        "while i < 2*n + extra { if i%2 == 0 { true -> { continue } false -> { i = i + 3\nn = n + 1 } } }",
        &values
    ));
}

#[test]
fn pure_non_affine_reads_do_not_require_arithmetic_success() {
    let values = integers(&[("i", 0), ("n", 10)]);
    for read in [
        "1/(n-n)",
        "i%2",
        "i << 1",
        "i >> 1",
        "i & 1",
        "i | 1",
        "!i",
        "@as(int, i)",
        "@as(uint, n)",
        "-(i%2)",
        "@as(int, i)%2 + 1",
    ] {
        assert!(
            certificate(
                &format!("while i < n {{ @println({read})\ni = i + 1 }}"),
                &values
            ),
            "{read}"
        );
    }
    assert!(certificate(
        "while i < n { if @as(int, i)%2 == 0 { true -> { i = i + 1 } false -> { i = i + 2 } } }",
        &values
    ));
    assert!(!certificate("while i < n { int fail=1/(n-n) }", &values));
    assert!(!certificate("while i < n { i = i + 1/(n-n) }", &values));
    assert!(!certificate("while i < n { i = @as(int, i+1) }", &values));
    // Nonlinear multiplication remains outside the deliberately narrow read extension.
    assert!(!certificate(
        "while i < n { int local=i*i\ni=i+1 }",
        &values
    ));
}

#[test]
fn opaque_integer_reads_cannot_hide_effects_or_control_flow() {
    let values = integers(&[("i", 0), ("n", 10)]);
    for read in [
        "1/unknown()",
        "@as(int, unknown())",
        "unknown()%2 == 0",
        "1/(if flag { true -> { 1 } false -> { 2 } })",
        "@as(int, async work())",
    ] {
        assert!(
            !certificate(
                &format!("while i < n {{ int local = {read}\ni = i + 1 }}"),
                &values
            ),
            "{read}"
        );
    }
}

#[test]
fn certified_division_failure_keeps_original_source_location() {
    let source = "mut int i=0\nmut int n=3\nint extra=1\nwhile i<2*n+extra {\nint fail=1/(n-n)\ni=i+3\nn=n+1\n}\n@println(i)";
    let path = std::path::Path::new("affine-read.nc");
    assert!(crate::compile_source_with_options(source, path, false).is_ok());
    let errors = crate::compile_source_with_options(source, path, true).unwrap_err();
    assert!(
        errors.0.iter().any(|diagnostic| {
            diagnostic.message.contains("division by zero")
                && diagnostic.path.as_deref() == Some(path)
                && source.get(diagnostic.span.clone()) == Some("1/(n-n)")
        }),
        "{errors:?}"
    );
}
