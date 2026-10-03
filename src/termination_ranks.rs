//! Two-binding certificates; rank intervals are not reconstructed from marginal intervals.
use super::{Analysis, Displacement, Exit, Range};
use crate::ast::{BinaryOp, Block, Expr};
use std::collections::HashSet;

#[derive(Clone, Copy)]
struct RankingState {
    bindings: [Range; 2],
    // Displacement of bound - counter, retaining correlation across branches.
    rank: Range,
}

impl Displacement for RankingState {
    const ZERO: Self = Self {
        bindings: [Range::ZERO; 2],
        rank: Range::ZERO,
    };

    fn union(self, other: Self) -> Self {
        Self {
            bindings: std::array::from_fn(|index| {
                self.bindings[index].union(other.bindings[index])
            }),
            rank: self.rank.union(other.rank),
        }
    }

    fn shifted(mut self, binding: usize, delta: i128) -> Option<Self> {
        self.bindings[binding] = self.bindings[binding].add(delta)?;
        self.rank = self.rank.add(if binding == 0 {
            delta.checked_neg()?
        } else {
            delta
        })?;
        Some(self)
    }
}

/// Certify a comparison between distinct named integer bindings.
/// Each tuple is (initial value, minimum representable value, maximum representable value).
/// Condition-prefix effects and nested-loop termination obligations belong to the caller,
/// just as for `counted_loop`; this only proves progress and ranking-state safety.
/// Input must be a checked NC AST, as with `counted_loop`.
/// No loop execution or speculative evaluation is performed.
pub(crate) fn varying_loop(
    condition: &Expr,
    body: &Block,
    counter: (i128, i128, i128),
    bound: (i128, i128, i128),
) -> bool {
    varying_certificate(condition, body, counter, bound).is_some()
}

fn varying_certificate(
    condition: &Expr,
    body: &Block,
    counter: (i128, i128, i128),
    bound: (i128, i128, i128),
) -> Option<()> {
    let Expr::Binary { left, op, right } = condition.unlocated() else {
        return None;
    };
    let (Expr::Name(counter_name), Expr::Name(bound_name)) = (left.unlocated(), right.unlocated())
    else {
        return None;
    };
    if counter_name == bound_name
        || !(counter.1..=counter.2).contains(&counter.0)
        || !(bound.1..=bound.2).contains(&bound.0)
    {
        return None;
    }
    let active = match op {
        BinaryOp::Lt => counter.0 < bound.0,
        BinaryOp::Le => counter.0 <= bound.0,
        BinaryOp::Gt => counter.0 > bound.0,
        BinaryOp::Ge => counter.0 >= bound.0,
        BinaryOp::Ne => counter.0 != bound.0,
        _ => return None,
    };
    if !active {
        return Some(());
    }
    let mut analysis = Analysis {
        counter: counter_name,
        bound: Some(bound_name),
        protected: HashSet::from([counter_name.as_str(), bound_name.as_str()]),
        direction: 0,
        excursion: RankingState::ZERO,
    };
    let mut progress: Option<RankingState> = None;
    for (exit, state) in analysis.block(body, RankingState::ZERO, true)? {
        if matches!(exit, Exit::Next | Exit::Continue(_)) {
            progress = Some(progress.map_or(state, |prior| prior.union(state)));
        }
    }
    // With no continuing paths there is at most one body execution.
    let iterations = match progress {
        Some(state) => iteration_horizon(*op, counter.0, bound.0, state.rank)?,
        None => 1,
    };
    let progress = progress.unwrap_or(RankingState::ZERO);
    for (index, initial) in [counter, bound].into_iter().enumerate() {
        binding_safe(
            initial,
            progress.bindings[index],
            analysis.excursion.bindings[index],
            iterations,
        )?;
    }
    Some(())
}

fn iteration_horizon(
    comparison: BinaryOp,
    counter: i128,
    bound: i128,
    rank: Range,
) -> Option<i128> {
    let gap = bound.checked_sub(counter)?;
    if comparison == BinaryOp::Ne {
        if rank.min != rank.max || rank.min == 0 || rank.min.signum() == gap.signum() {
            return None;
        }
        let distance = gap.checked_abs()?;
        let stride = rank.min.checked_abs()?;
        return (distance.checked_rem(stride)? == 0).then_some(distance / stride);
    }
    let (distance, stride) = match comparison {
        BinaryOp::Lt | BinaryOp::Le if rank.max < 0 => (gap, rank.max.checked_neg()?),
        BinaryOp::Gt | BinaryOp::Ge if rank.min > 0 => (gap.checked_neg()?, rank.min),
        _ => return None,
    };
    let distance = distance.checked_add(i128::from(matches!(
        comparison,
        BinaryOp::Le | BinaryOp::Ge
    )))?;
    (distance / stride).checked_add(i128::from(distance % stride != 0))
}

fn binding_safe(
    initial: (i128, i128, i128),
    progress: Range,
    excursion: Range,
    iterations: i128,
) -> Option<()> {
    let preceding = iterations.checked_sub(1)?;
    // Entry displacements accumulate only on continuing paths. Excursions also
    // include every intermediate update on break, return, and continue paths.
    let low = initial
        .0
        .checked_add(preceding.checked_mul(progress.min.min(0))?)?
        .checked_add(excursion.min)?;
    let high = initial
        .0
        .checked_add(preceding.checked_mul(progress.max.max(0))?)?
        .checked_add(excursion.max)?;
    (low >= initial.1 && high <= initial.2).then_some(())
}

#[cfg(test)]
mod tests {
    use crate::ast::{Item, Stmt};

    fn certificate(source: &str, counter: (i128, i128, i128), bound: (i128, i128, i128)) -> bool {
        let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
        let Item::Statement(statement) = &module.items[0] else {
            panic!("expected loop statement");
        };
        let Stmt::While {
            condition, body, ..
        } = statement.unlocated()
        else {
            panic!("expected while loop");
        };
        crate::termination::varying_loop(condition, body, counter, bound)
    }

    #[test]
    fn same_direction_and_bound_only_progress() {
        let counter = (0, -100, 100);
        let bound = (10, -100, 100);
        assert!(certificate(
            "while i < b { i = i + 2\nb = b + 1 }",
            counter,
            bound
        ));
        assert!(certificate("while i < b { b = b - 1 }", counter, bound));
        assert!(certificate(
            "while i < b { i = i - 1\nb = b - 2 }",
            counter,
            bound
        ));
        assert!(!certificate(
            "while i < b { i = i + 1\nb = b + 2 }",
            counter,
            bound
        ));
    }

    #[test]
    fn comparisons_and_exact_inequality_stride() {
        for operator in ["<", "<=", "!="] {
            let source = format!("while i {operator} b {{ i = i + 3\nb = b + 1 }}");
            assert!(certificate(&source, (0, -100, 100), (10, -100, 100)));
        }
        for operator in [">", ">=", "!="] {
            let source = format!("while i {operator} b {{ i = i - 3\nb = b - 1 }}");
            assert!(certificate(&source, (10, -100, 100), (0, -100, 100)));
        }
        assert!(!certificate(
            "while i != b { i = i + 3\nb = b + 1 }",
            (0, -100, 100),
            (9, -100, 100),
        ));
        assert!(certificate(
            "while i <= b { b = b - 1 }",
            (0, 0, 0),
            (0, -1, 0)
        ));
        assert!(!certificate(
            "while i <= b { b = b - 1 }",
            (0, 0, 0),
            (0, 0, 0)
        ));
    }

    #[test]
    fn branch_correlation_and_continues() {
        let body = "if flag { true -> { i = i + 2\nb = b + 1\ncontinue }
                               false -> { i = i + 11\nb = b + 10 } }";
        assert!(certificate(
            &format!("while i < b {{ {body} }}"),
            (0, -1000, 1000),
            (10, -1000, 1000),
        ));
        // Both branches have rank stride -1 despite disjoint marginal intervals.
        assert!(certificate(
            &format!("while i != b {{ {body} }}"),
            (0, -1000, 1000),
            (10, -1000, 1000),
        ));
        assert!(!certificate(
            "while i < b { if flag { true -> { continue } false -> { i = i + 1 } } }",
            (0, -100, 100),
            (10, -100, 100),
        ));
    }

    #[test]
    fn all_exit_paths_and_intermediate_excursions_are_checked() {
        for exit in ["break", "continue", "return"] {
            let source = format!(
                "while i < b {{ if flag {{ true -> {{ b = b + 100\n{exit} }}
                                           false -> {{ i = i + 1 }} }} }}"
            );
            assert!(!certificate(&source, (0, -100, 100), (10, -100, 100)));
        }
        assert!(!certificate(
            "while i < b { b = b + 100\nb = b - 101 }",
            (0, -100, 100),
            (10, -100, 100),
        ));
        assert!(certificate(
            "while i < b { b = b + 100\nb = b - 101 }",
            (0, -100, 100),
            (10, -200, 200),
        ));
        assert!(!certificate(
            "while i < b { i = i + 100\ni = i - 99 }",
            (0, -100, 100),
            (10, -100, 100),
        ));
        assert!(certificate(
            "while i < b { break }",
            (0, 0, 0),
            (10, 10, 10)
        ));
    }

    #[test]
    fn each_binding_has_its_own_limits_and_full_horizon() {
        let source = "while i < b { i = i + 2\nb = b + 1 }";
        assert!(certificate(source, (0, 0, 20), (10, 10, 20)));
        assert!(!certificate(source, (0, 0, 19), (10, 10, 20)));
        assert!(!certificate(source, (0, 0, 20), (10, 10, 19)));
        let source = "while i > b { i = i - 2\nb = b - 1 }";
        assert!(certificate(source, (10, -10, 10), (0, -10, 0)));
        assert!(!certificate(source, (10, -9, 10), (0, -10, 0)));
        assert!(!certificate(source, (10, -10, 10), (0, -9, 0)));
    }

    #[test]
    fn reject_unknown_updates_shadows_and_nested_mutations() {
        for body in [
            "i = 1",
            "b = i + 1",
            "b = b + step",
            "mut int b = 1\ni = i + 1",
            "while flag { b = b - 1 }\ni = i + 1",
            "for b in values {}\ni = i + 1",
            "mut int x = unknown()\ni = i + 1",
        ] {
            assert!(
                !certificate(
                    &format!("while i < b {{ {body} }}"),
                    (0, -100, 100),
                    (10, -100, 100),
                ),
                "{body}"
            );
        }
        assert!(!certificate(
            "while i < i { i = i + 1 }",
            (0, -100, 100),
            (10, -100, 100)
        ));
    }

    #[test]
    fn variable_rank_stride_and_unbounded_proof_horizon() {
        let body = "if flag { true -> { i = i + 3\nb = b + 1 }
                               false -> { i = i + 11\nb = b + 10 } }";
        assert!(certificate(
            &format!("while i < b {{ {body} }}"),
            (0, -1000, 1000),
            (10, -1000, 1000),
        ));
        assert!(!certificate(
            &format!("while i != b {{ {body} }}"),
            (0, -1000, 1000),
            (10, -1000, 1000),
        ));
        assert!(certificate(
            "while i < b { b = b - 1 }",
            (0, 0, 0),
            (1_000_000_000_000, 0, 1_000_000_000_000),
        ));
    }

    #[test]
    fn inactive_loops_and_certificate_arithmetic_overflow() {
        assert!(certificate(
            "while i < b { unknown() }",
            (10, 0, 10),
            (0, 0, 10)
        ));
        assert!(!certificate(
            "while i < b { i = i + 1 }",
            (i128::MIN, i128::MIN, i128::MAX),
            (i128::MAX, i128::MIN, i128::MAX),
        ));
        assert!(!certificate(
            "while i < b { i = i + 1 }",
            (0, 1, 10),
            (10, 0, 10)
        ));
    }
}
