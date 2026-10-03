//! Exact multiplicative-counter / additive-bound certificates, without body execution.
#[path = "termination_growth_support.rs"]
pub(super) mod support;

use crate::ast::{BinaryOp, Block, Expr};
use std::collections::HashMap;
use support::{Exit, Interval, Path, Step, Update};

type Values = HashMap<String, (i128, i128, i128)>;

/// Input is a checked AST with canonical storage names and unwritten constants
/// substituted by the caller. Tuples are (initial, integer minimum, integer maximum).
/// Condition-prefix effects must already have been normalized by the caller.
pub(crate) fn growth_loop(condition: &Expr, body: &Block, values: &Values) -> bool {
    let Some((name, op, bound)) = support::comparison(condition) else {
        return false;
    };
    if certificate(name, op, bound, body, values).is_some() {
        return true;
    }
    let Expr::Name(other) = bound.unlocated() else {
        return false;
    };
    certificate(
        other,
        support::reverse(op),
        &Expr::Name(name.into()),
        body,
        values,
    )
    .is_some()
}

struct Model<'a> {
    names: [&'a str; 2],
    limits: [(i128, i128, i128); 2],
    inclusive: bool,
}

fn certificate(
    name: &str,
    op: BinaryOp,
    bound: &Expr,
    body: &Block,
    values: &Values,
) -> Option<()> {
    if !matches!(op, BinaryOp::Lt | BinaryOp::Le) {
        return None;
    }
    let counter = *values.get(name)?;
    let (bound_name, bound) = match bound.unlocated() {
        Expr::Name(other) if other != name => (other.as_str(), *values.get(other)?),
        _ => {
            let value = support::constant(bound, counter)?;
            ("", (value, value, value))
        }
    };
    if !support::valid(&counter) || !support::valid(&bound) || counter.0 <= 0 {
        return None;
    }
    let model = Model {
        names: [name, bound_name],
        limits: [counter, bound],
        inclusive: op == BinaryOp::Le,
    };
    let paths = support::paths(body)?;
    let mut recurrence = None;
    for path in &paths {
        let update = summary(path, &model)?;
        if path.exit != Exit::Break {
            if update.0 <= 1 || recurrence.is_some_and(|prior| prior != update) {
                return None;
            }
            recurrence = Some(update);
        }
    }
    let entries = if let Some((factor, delta)) = recurrence {
        horizon(&model, factor, delta)?
    } else {
        [
            Interval {
                low: counter.0,
                high: counter.0,
            },
            Interval {
                low: bound.0,
                high: bound.0,
            },
        ]
    };
    for path in &paths {
        safe_path(path, &model, entries)?;
    }
    Some(())
}

fn summary(path: &Path<'_>, model: &Model<'_>) -> Option<(i128, i128)> {
    let mut factor = 1_i128;
    let mut delta = 0_i128;
    for step in &path.steps {
        if let Step::Update(name, update) = step {
            match update {
                Update::Mul(value) if *name == model.names[0] && *value > 1 => {
                    factor = factor.checked_mul(*value)?;
                }
                Update::Add(value) if *name == model.names[1] && !name.is_empty() => {
                    delta = delta.checked_add(*value)?;
                }
                _ => return None,
            }
        }
    }
    Some((factor, delta))
}

/// Solve x(k)=seed*factor^k and b(k)=bound+k*delta using only recurrence
/// arithmetic. Positive seed and factor > 1 make the number of terms logarithmic
/// in integer magnitude, even when the gap initially grows. Checked multiplication
/// bounds this structural calculation; there is no execution budget or AST evaluator.
fn horizon(model: &Model<'_>, factor: i128, delta: i128) -> Option<[Interval; 2]> {
    let [counter, bound] = model.limits;
    let mut power = counter.0;
    let mut additive = bound.0;
    let mut entries = [
        Interval {
            low: power,
            high: power,
        },
        Interval {
            low: additive,
            high: additive,
        },
    ];
    while power < additive || (model.inclusive && power == additive) {
        entries[0].high = power;
        entries[1].low = entries[1].low.min(additive);
        entries[1].high = entries[1].high.max(additive);
        power = power.checked_mul(factor)?;
        additive = additive.checked_add(delta)?;
        if power > counter.2 || additive < bound.1 || additive > bound.2 {
            return None;
        }
    }
    Some(entries)
}

fn safe_path(path: &Path<'_>, model: &Model<'_>, mut ranges: [Interval; 2]) -> Option<()> {
    for step in &path.steps {
        match step {
            Step::Guard(value, truth) => {
                if !support::guard(value, *truth, model.names, &mut ranges, &model.limits)? {
                    return Some(());
                }
            }
            Step::Constant(name, operand) => {
                let index = model.names.iter().position(|candidate| candidate == name)?;
                support::constant(operand, model.limits[index])?;
            }
            Step::Update(name, update) => {
                let index = model.names.iter().position(|candidate| candidate == name)?;
                ranges[index] = ranges[index].checked(*update, model.limits[index])?;
            }
            Step::Read(value) => support::read_safe(value, model.names, &ranges, &model.limits)?,
            Step::Declare(declaration) => {
                if declaration
                    .binding_names()
                    .iter()
                    .any(|name| model.names.contains(name))
                {
                    return None;
                }
                support::declaration_safe(declaration, model.names, &ranges, &model.limits)?;
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::growth_loop;
    use crate::ast::{Item, Stmt};
    use std::collections::HashMap;

    fn check(source: &str, counter: (i128, i128, i128), bound: (i128, i128, i128)) -> bool {
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
        growth_loop(
            condition,
            body,
            &HashMap::from([("i".into(), counter), ("n".into(), bound)]),
        )
    }

    #[test]
    fn factors_strides_and_comparisons() {
        for source in [
            "while i < n { i = i * 2\nn = n + 1 }",
            "while n > i { i = 3 * i\nn = 2 + n }",
            "while i <= n { n = n - 2\ni = i * 4 }",
            "while i < 100u { i = i * 2u }",
            "while i <= 0x64 { i = i * 0x02 }",
        ] {
            assert!(check(source, (1, 0, 1000), (100, -1000, 1000)), "{source}");
        }
    }

    #[test]
    fn uniform_branches_and_exits() {
        assert!(check(
            "while i < n { if { i < 10 -> { i = i * 2\nn = n + 1\ncontinue } _ -> { n = n + 1\ni = i * 2 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(check(
            "while i < n { if { i > 50 -> { break } _ -> { i = i * 2 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { if { i < 10 -> { i = i * 2 } _ -> { i = i * 3 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { if { i < 10 -> { continue } _ -> { i = i * 2 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn rejects_overflow_including_final_update_and_excursions() {
        assert!(!check(
            "while i < n { i = i * 2 }",
            (1, 0, 127),
            (100, 0, 255)
        ));
        assert!(!check(
            "while i <= n { i = i * 2 }",
            (1, 0, 128),
            (128, 0, 255)
        ));
        assert!(check(
            "while i < n { i = i * 2 }",
            (1, 0, 128),
            (128, 0, 255)
        ));
        assert!(!check(
            "while i < n { n = n + 1000\nn = n - 999\ni = i * 2 }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { if { i > 50 -> { n = n + 1000\nbreak } _ -> { i = i * 2 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn rejects_unproved_states_and_effects() {
        for source in [
            "while i < n { i = i * 1 }",
            "while i < n { i = i * n }",
            "while i < n { i = 2 }",
            "while i < n { i = i * 2\nunknown() }",
            "while i < n { while i < n { i = i * 2 } }",
            "while i < n { if { unknown() -> { i = i * 2 } _ -> { i = i * 2 } } }",
            "while i < n { if unknown() { _ -> { i = i * 2 } } }",
        ] {
            assert!(!check(source, (1, 0, 1000), (100, 0, 1000)), "{source}");
        }
        assert!(!check(
            "while i < n { i = i * 2 }",
            (0, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { i = i * 2 }",
            (-1, -1000, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn output_reads_assertions_and_declarations_preserve_progress() {
        assert!(check(
            "while i < n { @print(\"iteration\", i)\n@println(n)\ni\nassert i > 0\nint probe = i + 1\n@println(probe)\n{ i = i * 2\nn = n + 1 } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { @println(i)\nint probe = 1\nassert true }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { i = i * 2\nint i = 1 }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn read_work_cannot_hide_effects_or_overflow() {
        for work in [
            "@println(unknown())",
            "@println(\"{unknown()}\")",
            "@other(i)",
            "assert unknown()",
            "int probe = unknown()",
            "int probe = i * 1000",
            "int probe = (i + 1000) - 1000",
            "i / 0",
            "int probe = if { true -> { i = 1\nbreak 1 } }",
        ] {
            let source = format!("while i < n {{ {work}\ni = i * 2 }}");
            assert!(!check(&source, (1, 0, 1000), (100, 0, 1000)), "{source}");
        }
        assert!(!check(
            "while i < n { if { i > 50 -> { @println(i * 1000)\nbreak } _ -> { i = i * 2 } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn labeled_paths_and_implicit_fallthrough_are_not_lost() {
        assert!(check(
            "while i < n { i = i * 2\ncontinue :outer }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(check(
            "while i < n { i = i * 2\nbreak :outer }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { if { i < 10 -> { i = i * 2\ncontinue :outer } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { if { i < 10 -> { break :outer } } }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
        assert!(!check(
            "while i < n { scope: if { true -> { break :scope } }\ni = i * 2 }",
            (1, 0, 1000),
            (100, 0, 1000)
        ));
    }

    #[test]
    fn suffixed_and_negative_substituted_constants() {
        assert!(check(
            "while i < 100u { i = i * 2u }",
            (1, 0, i128::from(u64::MAX)),
            (100, 0, 1000)
        ));
        assert!(check(
            "while i < n { i = i * 2\nn = n + (-1) }",
            (1, 0, 1000),
            (100, -1000, 1000)
        ));
    }

    #[test]
    fn unsigned_original_operands_are_checked_before_delta_reduction() {
        let maximum = i128::from(u64::MAX);
        for negative in ["(-1u)", "(-1)"] {
            for source in [
                format!("while i < n {{ i = i * 2u\nn = n - {negative} }}"),
                format!("while i < {negative} {{ i = i * 2u }}"),
                format!(
                    "while i < n {{ if {{ i > {negative} -> {{ i = i * 2u }} _ -> {{ i = i * 2u }} }} }}"
                ),
                format!("while i < n {{ if i > {negative} {{ _ -> {{ i = i * 2u }} }} }}"),
                format!("while i < n {{ @println(n - {negative})\ni = i * 2u }}"),
                format!("while i < n {{ assert n > {negative}\ni = i * 2u }}"),
                format!("while i < n {{ uint probe = {negative}\ni = i * 2u }}"),
                format!("while i < n {{ byte probe = {negative}\ni = i * 2u }}"),
                format!("while i < n {{ @println(2u - {negative})\ni = i * 2u }}"),
            ] {
                assert!(
                    !check(&source, (1, 0, maximum), (100, 0, maximum)),
                    "{source}"
                );
            }
        }
        assert!(!check(
            "while i < n { @println(-1u)\ni = i * 2 }",
            (1, 0, maximum),
            (100, 0, maximum)
        ));
        assert!(!check(
            "while i < 256 { break }",
            (1, 0, 255),
            (100, 0, 255)
        ));
    }

    #[test]
    fn unsigned_decrement_and_zero_negation_still_certify() {
        let maximum = i128::from(u64::MAX);
        for source in [
            "while i < n { i = i * 2u\nn = n - 1u }",
            "while i < n { i = i * 2u\nn = n - (-0u) }",
            "while i < n { if i >= -0u { true -> { i = i * 2u } false -> { i = i * 2u } } }",
            "while i < n { uint zero = -0u\n@println(-0u, n - 1)\ni = i * 2u }",
        ] {
            assert!(
                check(source, (1, 0, maximum), (100, 0, maximum)),
                "{source}"
            );
        }
    }

    #[test]
    fn signed_negative_steps_and_signed_minimum_remain_valid() {
        let minimum = i128::from(i64::MIN);
        let maximum = i128::from(i64::MAX);
        for source in [
            "while i < n { i = i * 2\nn = n - (-1) }",
            "while i < n { i = i * 2\nn = n + (-9223372036854775808) }",
            "while i < n { int smallest = -9223372036854775808\n@println(-9223372036854775808, -1)\ni = i * 2 }",
        ] {
            assert!(
                check(source, (1, minimum, maximum), (100, minimum, maximum)),
                "{source}"
            );
        }
        assert!(!check(
            "while i < n { i = i * 2\nn = n - (-1u) }",
            (1, minimum, maximum),
            (100, minimum, maximum)
        ));
        assert!(!check(
            "while i < n { i = i * 2\nn = n + (-(-9223372036854775808)) }",
            (1, minimum, maximum),
            (100, minimum, maximum)
        ));
    }

    #[test]
    fn small_recurrences_match_arithmetic_safety() {
        for factor in 2..=4 {
            for delta in -3..=3 {
                let source = format!("while i < n {{ i = i * {factor}\nn = n + ({delta}) }}");
                for seed in 1..=8 {
                    for bound in 1..=16 {
                        let accepted = check(&source, (seed, 0, 31), (bound, -32, 31));
                        let mut counter = seed;
                        let mut limit = bound;
                        let mut safe = true;
                        while counter < limit {
                            counter *= factor;
                            limit += delta;
                            if counter > 31 || !(-32..=31).contains(&limit) {
                                safe = false;
                                break;
                            }
                        }
                        assert!(!accepted || safe, "{source}: {seed}, {bound}");
                    }
                }
            }
        }
    }

    #[test]
    fn long_initial_gap_growth_is_not_a_fuel_limit() {
        assert!(check(
            "while i < n { i = i * 2\nn = n + 1000000000 }",
            (1, 0, i128::from(i64::MAX)),
            (100, 0, i128::from(i64::MAX))
        ));
    }
}
