//! Bounded lexicographic certificates for an outer counter and a resettable counter.
use super::growth::support;

use crate::ast::{BinaryOp, Block, Expr};
use std::collections::HashMap;
use support::{Exit, Interval, Path, Step, Update};

type Values = HashMap<String, (i128, i128, i128)>;

/// Prove progress on every continuing path, with integer safety on all paths.
/// Requires a checked AST, canonical names, substituted unwritten constants,
/// and an effect-free condition. Condition-prefix updates are not supported here.
pub(crate) fn reset_loop(condition: &Expr, body: &Block, values: &Values) -> bool {
    certificate(condition, body, values).is_some()
}

struct Model<'a> {
    names: [&'a str; 2],
    limits: [(i128, i128, i128); 2],
    directions: [i128; 2],
    entries: [Interval; 2],
}

fn certificate(condition: &Expr, body: &Block, values: &Values) -> Option<()> {
    let (outer, op, threshold) = support::comparison(condition)?;
    let outer_limits = *values.get(outer)?;
    let threshold = support::constant(threshold, outer_limits)?;
    if !support::valid(&outer_limits) {
        return None;
    }
    let direction = match op {
        BinaryOp::Gt | BinaryOp::Ge => -1,
        BinaryOp::Lt | BinaryOp::Le => 1,
        _ => return None,
    };
    let paths = support::paths(body)?;
    let secondary = secondary_name(&paths, outer)?;
    let secondary_limits = *values.get(secondary)?;
    if !support::valid(&secondary_limits) {
        return None;
    }
    let secondary_direction = secondary_direction(&paths, secondary)?;
    let mut outer_range = if direction < 0 {
        Interval {
            low: outer_limits.1,
            high: outer_limits.0,
        }
    } else {
        Interval {
            low: outer_limits.0,
            high: outer_limits.2,
        }
    };
    outer_range.refine(op, threshold, true)?;
    outer_range = congruent_entries(
        outer_range,
        outer_limits.0,
        direction,
        outer_stride(&paths, outer)?,
    )?;
    let model = Model {
        names: [outer, secondary],
        limits: [outer_limits, secondary_limits],
        directions: [direction, secondary_direction],
        // The secondary's full representable interval is a finite invariant.
        // Guards narrow it before decrements; bounded resets preserve it.
        entries: [
            outer_range,
            Interval {
                low: secondary_limits.1,
                high: secondary_limits.2,
            },
        ],
    };
    if outer_range.low > outer_range.high {
        return Some(());
    }
    for path in &paths {
        prove_path(path, &model)?;
    }
    Some(())
}

/// Each continuing path preserves the initial residue modulo the gcd of its
/// net outer displacement. Break paths do not produce another loop entry.
fn outer_stride(paths: &[Path<'_>], outer: &str) -> Option<i128> {
    let mut stride = 0;
    for path in paths {
        if path.exit == Exit::Break {
            continue;
        }
        let mut net = 0_i128;
        for step in &path.steps {
            if let Step::Update(name, update) = step
                && *name == outer
            {
                let Update::Add(delta) = update else {
                    return None;
                };
                net = net.checked_add(*delta)?;
            }
        }
        stride = gcd(stride, net.checked_abs()?);
    }
    Some(stride)
}

fn gcd(mut left: i128, mut right: i128) -> i128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn congruent_entries(
    mut range: Interval,
    initial: i128,
    direction: i128,
    stride: i128,
) -> Option<Interval> {
    if range.low > range.high {
        return Some(range);
    }
    if stride == 0 {
        return Some(Interval {
            low: initial,
            high: initial,
        });
    }
    // Subtract residues, not endpoints: the full signed interval's width may
    // exceed i128 even though the required adjustment is smaller than stride.
    let residue = initial.rem_euclid(stride);
    if direction < 0 {
        let adjustment = (residue - range.low.rem_euclid(stride)).rem_euclid(stride);
        range.low = range.low.checked_add(adjustment)?;
    } else {
        let adjustment = (range.high.rem_euclid(stride) - residue).rem_euclid(stride);
        range.high = range.high.checked_sub(adjustment)?;
    }
    Some(range)
}

fn secondary_name<'a>(paths: &[Path<'a>], outer: &str) -> Option<&'a str> {
    let mut secondary = None;
    for path in paths {
        for step in &path.steps {
            if let Step::Update(name, _) = step
                && *name != outer
            {
                if secondary.is_some_and(|prior| prior != *name) {
                    return None;
                }
                secondary = Some(*name);
            }
        }
    }
    secondary
}

fn secondary_direction(paths: &[Path<'_>], secondary: &str) -> Option<i128> {
    let mut direction = 0;
    for path in paths {
        for step in &path.steps {
            if let Step::Update(name, Update::Add(delta)) = step
                && *name == secondary
                && *delta != 0
            {
                if direction != 0 && direction != delta.signum() {
                    return None;
                }
                direction = delta.signum();
            }
        }
    }
    // If every continuing path decreases the outer rank, no secondary progress
    // is needed, but its bounded resets still need arithmetic validation.
    Some(if direction == 0 { -1 } else { direction })
}

fn prove_path(path: &Path<'_>, model: &Model<'_>) -> Option<()> {
    let mut ranges = model.entries;
    let mut displacement = [Some(0_i128); 2];
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
                match update {
                    Update::Add(delta)
                        if *delta == 0 || delta.signum() == model.directions[index] =>
                    {
                        if let Some(prior) = displacement[index] {
                            displacement[index] = Some(prior.checked_add(*delta)?);
                        }
                    }
                    Update::Set(_) if index == 1 => displacement[1] = None,
                    _ => return None,
                }
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
    if path.exit == Exit::Break {
        return Some(());
    }
    let progresses = |index: usize| {
        displacement[index]
            .is_some_and(|delta| delta != 0 && delta.signum() == model.directions[index])
    };
    // The outer never moves backwards. If it is unchanged, the secondary must
    // progress without a reset, so the finite lexicographic rank strictly falls.
    (progresses(0) || progresses(1)).then_some(())
}

#[cfg(test)]
mod tests {
    use super::reset_loop;
    use crate::ast::{Item, Stmt};
    use std::collections::HashMap;

    fn check(source: &str, rows: (i128, i128, i128), columns: (i128, i128, i128)) -> bool {
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
        reset_loop(
            condition,
            body,
            &HashMap::from([("rows".into(), rows), ("columns".into(), columns)]),
        )
    }

    const RESET: &str = "while rows > 0 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 1\ncolumns = 4 } } }";

    #[test]
    fn reset_strides_and_inclusive_guards() {
        assert!(check(RESET, (10, 0, 255), (4, 0, 255)));
        assert!(check(
            "while 0 < rows { if { columns >= 2 -> { columns = columns - 2\ncontinue } _ -> { rows = rows - 3\ncolumns = 8u } } }",
            (10, -100, 100),
            (8, 0, 255)
        ));
        assert!(check(
            "while rows >= 0 { if columns > 0 { true -> { columns = columns - 1 } false -> { columns = 4\nrows = rows - 2 } } }",
            (10, -100, 100),
            (4, 0, 255)
        ));
    }

    #[test]
    fn byte_outer_stride_preserves_the_initial_congruence() {
        let source = "while rows > 0 { { if columns >= 2u { true -> { columns = columns - 2u } false -> { rows = rows - 2u\ncolumns = 4u } } } }";
        assert!(check(source, (6, 0, 255), (4, 0, 255)));
        assert!(!check(source, (1, 0, 255), (4, 0, 255)));
        assert!(!check(source, (5, 0, 255), (4, 0, 255)));
        let inclusive = source.replace("rows > 0", "rows >= 0");
        assert!(!check(&inclusive, (6, 0, 255), (4, 0, 255)));
        // The stride is the NET path displacement, not each individual update.
        let split = source.replace("rows = rows - 2u", "rows = rows - 1u\nrows = rows - 1u");
        assert!(check(&split, (6, 0, 255), (4, 0, 255)));
        let excessive = source.replace("rows = rows - 2u", "rows = rows - 4u");
        assert!(!check(&excessive, (6, 0, 255), (4, 0, 255)));
    }

    #[test]
    fn congruence_uses_all_continuations_and_checks_break_excursions() {
        assert!(!check(
            "while rows > 0 { if { columns > 1 -> { rows = rows - 2\ncolumns = 4 } columns > 0 -> { rows = rows - 3\ncolumns = 4 } _ -> { columns = 4\nrows = rows - 2 } } }",
            (6, 0, 255),
            (4, 0, 255)
        ));
        assert!(check(
            "while rows > 0 { if { columns > 4 -> { rows = rows - 1\nbreak } columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 2\ncolumns = 4 } } }",
            (6, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 4 -> { rows = rows - 3\nbreak } columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 2\ncolumns = 4 } } }",
            (6, 0, 255),
            (4, 0, 255)
        ));
    }

    #[test]
    fn increasing_congruence_checks_the_last_update() {
        let source = "while rows < 255 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = rows + 2\ncolumns = 4 } } }";
        assert!(check(source, (1, 0, 255), (4, 0, 255)));
        assert!(!check(source, (0, 0, 255), (4, 0, 255)));
        assert!(!check(
            &source.replace("rows < 255", "rows <= 255"),
            (1, 0, 255),
            (4, 0, 255)
        ));
    }

    #[test]
    fn symmetric_increasing_counters() {
        assert!(check(
            "while rows < 10 { if { columns < 0 -> { columns = columns + 2 } _ -> { rows = rows + 2\ncolumns = -8 } } }",
            (0, -100, 100),
            (-8, -100, 100)
        ));
        assert!(check(
            "while rows <= 10 { if { columns <= -2 -> { columns = columns + 2 } _ -> { rows = rows + 1\ncolumns = -8 } } }",
            (0, -100, 100),
            (-8, -100, 100)
        ));
    }

    #[test]
    fn break_arithmetic_and_intermediate_excursions() {
        assert!(check(
            "while rows > 0 { if { columns > 0 -> { break } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 0 -> { columns = 256\nbreak } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 1\ncolumns = 256\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows >= 0 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 0 -> { columns = columns - 2 } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
    }

    #[test]
    fn rejects_rank_resets_missing_progress_and_unknown_bounds() {
        for source in [
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = 4\ncolumns = 4 } } }",
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1\ncolumns = 4 } _ -> { rows = rows - 1 } } }",
            "while rows > 0 { if { columns > 0 -> { continue } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 1\ncolumns = rows } } }",
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1 } } }",
            "while rows > 0 { columns = columns - 1\nrows = rows - 1 }",
        ] {
            assert!(!check(source, (10, 0, 255), (4, 0, 255)), "{source}");
        }
    }

    #[test]
    fn rejects_hidden_effects_and_control() {
        for body in [
            "unknown()",
            "rows = unknown()",
            "while columns > 0 { columns = columns - 1 }",
            "return",
            "break :outer",
            "continue :outer",
            "if { unknown() -> { columns = columns - 1 } _ -> { rows = rows - 1 } }",
            "if unknown() { _ -> { rows = rows - 1 } }",
        ] {
            let source = format!("while rows > 0 {{ {body}\ncolumns = 4\nrows = rows - 1 }}");
            assert!(!check(&source, (10, 0, 255), (4, 0, 255)), "{source}");
        }
    }

    #[test]
    fn ordered_guards_track_intermediate_state_and_varying_strides() {
        assert!(check(
            "while rows > 0 { if { columns >= 3 -> { columns = columns - 3 } columns > 0 -> { columns = columns - 1 } _ -> { rows = rows - 1\ncolumns = 0b100 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1\ncolumns = columns - 1 } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(check(
            "while rows > 0 { if { columns > 0 -> { columns = columns - 1\nif { columns > 0 -> { columns = columns - 1 } } } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
    }

    #[test]
    fn output_pure_work_and_labeled_paths() {
        assert!(check(
            "while rows > 0 { @print(\"rows\", rows)\nassert rows > 0\nint probe = rows + 1\nprobe\nif { columns > 0 -> { columns = columns - 1\n@println(columns)\ncontinue :outer } _ -> { rows = rows - 1\ncolumns = 4u } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(check(
            "while rows > 0 { if { columns > 0 -> { @println(columns)\nbreak :outer } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
        assert!(!check(
            "while rows > 0 { if { columns > 0 -> { @println(columns)\ncontinue :outer } _ -> { rows = rows - 1\ncolumns = 4 } } }",
            (10, 0, 255),
            (4, 0, 255)
        ));
    }

    #[test]
    fn probes_and_declarations_cannot_invalidate_the_rank() {
        for work in [
            "int rows = 10",
            "int columns = 4",
            "int probe = unknown()",
            "assert unknown()",
            "@print(unknown())",
            "@println(columns + 1)",
            "int probe = columns * 1000",
            "int probe = if { true -> { rows = 10\nbreak 1 } }",
        ] {
            let source = RESET.replace("{ if", &format!("{{ {work}\nif"));
            assert!(!check(&source, (10, 0, 255), (4, 0, 255)), "{source}");
        }
    }

    #[test]
    fn signed_thresholds_and_unsigned_reset_literals() {
        assert!(check(
            "while rows > -10 { if { columns > 0u -> { columns = columns - 1u } _ -> { rows = rows - 2\ncolumns = 0xFFu } } }",
            (10, -100, 100),
            (4, 0, i128::from(u64::MAX))
        ));
    }

    #[test]
    fn unsigned_negative_operands_cannot_hide_in_reset_certificates() {
        for negative in ["(-1u)", "(-1)"] {
            let source = format!(
                "while rows < 10 {{ if {{ columns > 0 -> {{ columns = columns - 1 }} _ -> {{ rows = rows - {negative}\ncolumns = 4 }} }} }}"
            );
            assert!(!check(&source, (0, 0, 255), (4, 0, 255)), "{source}");
            for source in [
                format!(
                    "while rows > {negative} {{ if {{ columns > 0 -> {{ break }} _ -> {{ rows = rows - 1\ncolumns = 4 }} }} }}"
                ),
                format!(
                    "while rows > 0 {{ if columns > {negative} {{ _ -> {{ rows = rows - 1\ncolumns = 4 }} }} }}"
                ),
                format!(
                    "while rows > 0 {{ if {{ columns > {negative} -> {{ rows = rows - 1\ncolumns = 4 }} _ -> {{ rows = rows - 1\ncolumns = 4 }} }} }}"
                ),
                RESET.replace(
                    "columns = columns - 1",
                    &format!("columns = columns + {negative}"),
                ),
                RESET.replace("columns = 4", &format!("columns = {negative}")),
                RESET.replace("{ if", &format!("{{ uint probe = columns - {negative}\nif")),
                RESET.replace("{ if", &format!("{{ byte probe = {negative}\nif")),
            ] {
                assert!(!check(&source, (6, 0, 255), (4, 0, 255)), "{source}");
            }
        }
    }

    #[test]
    fn reset_literals_preserve_zero_negation_and_signed_minimum() {
        let zero_reset = RESET.replace("columns = 4", "columns = -0u");
        assert!(check(&zero_reset, (6, 0, 255), (4, 0, 255)));
        let zero_guard = RESET
            .replace("rows > 0", "rows > -0u")
            .replace("columns > 0", "columns > -0u");
        assert!(check(&zero_guard, (6, 0, 255), (4, 0, 255)));
        let minimum = i128::from(i64::MIN);
        let maximum = i128::from(i64::MAX);
        let signed_reset = RESET.replace("columns = 4", "columns = -9223372036854775808");
        assert!(check(
            &signed_reset,
            (6, minimum, maximum),
            (4, minimum, maximum)
        ));
        let signed_bound = RESET.replace("rows > 0", "rows > -9223372036854775808");
        assert!(check(&signed_bound, (0, minimum, maximum), (4, 0, 255)));
    }

    #[test]
    fn checked_finite_ranges_not_initial_state_speculation() {
        assert!(check(
            RESET,
            (1_000_000, 0, i128::from(i64::MAX)),
            (4, 0, i128::from(u64::MAX))
        ));
        assert!(!check(RESET, (10, 0, 255), (256, 0, 255)));
        assert!(!check(RESET, (10, 0, 255), (4, 0, 3)));
    }
}
