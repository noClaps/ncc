//! Interval helper-loop summaries for checked, storage-renamed proof trees.
//!
//! These statements are nondeterministic proof envelopes, NOT executable rewrites.
//! Monotone updates make every intermediate excursion lie between entry and exit.
//! Independent envelopes deliberately discard correlations: this can lose proofs,
//! but cannot invent progress or conceal a protected binding's excursion.
use std::collections::BTreeMap;

use super::helpers::integer;
use super::{BinaryOp, Block, Expr, Pattern, Stmt, Type};

#[derive(Clone, Copy)]
struct Interval {
    low: i128,
    high: i128,
}

impl Interval {
    const ZERO: Self = Self { low: 0, high: 0 };

    fn add(self, other: Self) -> Option<Self> {
        Some(Self {
            low: self.low.checked_add(other.low)?,
            high: self.high.checked_add(other.high)?,
        })
    }

    fn union(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }

    fn repeated(self, counts: Self) -> Option<Self> {
        if self.low >= 0 {
            Some(Self {
                low: self.low.checked_mul(counts.low)?,
                high: self.high.checked_mul(counts.high)?,
            })
        } else if self.high <= 0 {
            Some(Self {
                low: self.low.checked_mul(counts.high)?,
                high: self.high.checked_mul(counts.low)?,
            })
        } else {
            None
        }
    }
}

type Updates = BTreeMap<String, Interval>;

/// Summarize only universally finite, monotone scalar helper loops.
/// `prefix` must be the ordered statements preceding this loop in its expanded
/// helper block. Canonical storage names, checked types, and unchanged source
/// execution are required, as for the literal helper summary.
pub(super) fn summarize_loop(statement: &Stmt, prefix: &[Stmt]) -> Option<Vec<Stmt>> {
    let Stmt::While {
        condition, body, ..
    } = statement.unlocated()
    else {
        return None;
    };
    let Expr::Binary { left, op, right } = condition.unlocated() else {
        return None;
    };
    let Expr::Name(counter) = left.unlocated() else {
        return None;
    };
    let (start, representation) = local_range(counter, prefix)?;
    let bound = expression_range(right, prefix)?;
    let mut directions = BTreeMap::new();
    let updates = block_updates(body, &mut directions)?;
    // A variable bound must remain invariant, including across all branches.
    if let Expr::Name(name) = right.unlocated()
        && updates.contains_key(name)
    {
        return None;
    }
    let step = *updates.get(counter)?;
    let counts = iteration_counts(start, bound, step, *op, representation)?;
    updates
        .iter()
        .map(|(name, delta)| envelope(name, delta.repeated(counts)?))
        .collect::<Option<Vec<_>>>()
        .map(|statements| statements.into_iter().flatten().collect())
}

fn type_range(ty: &Type) -> Option<Interval> {
    let Type::Named(name, arguments) = ty else {
        return None;
    };
    if !arguments.is_empty() {
        return None;
    }
    let (low, high) = match name.as_str() {
        "int" => (i128::from(i64::MIN), i128::from(i64::MAX)),
        "uint" => (0, i128::from(u64::MAX)),
        "byte" => (0, i128::from(u8::MAX)),
        _ => return None,
    };
    Some(Interval { low, high })
}

fn local_range(name: &str, prefix: &[Stmt]) -> Option<(Interval, Interval)> {
    let (index, declaration) = prefix
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, statement)| {
            let Stmt::Var(declaration) = statement.unlocated() else {
                return None;
            };
            matches!(&declaration.pattern, Pattern::Name(binding) if binding == name)
                .then_some((index, declaration))
        })?;
    if declaration.mutex {
        return None;
    }
    let mut writes = super::HashSet::new();
    super::block_writes(
        &Block {
            statements: prefix[index + 1..].to_vec(),
        },
        &mut writes,
    );
    if writes.contains(name) {
        return None;
    }
    let representation = type_range(&declaration.ty)?;
    // Unknown initializers need not be executed. Successful checked evaluation
    // produces a value in this type's range; failures stay in the source prefix.
    let value = initializer_range(&declaration.value, &prefix[..index]).unwrap_or(representation);
    (value.low >= representation.low && value.high <= representation.high)
        .then_some((value, representation))
}

fn initializer_range(value: &Expr, prefix: &[Stmt]) -> Option<Interval> {
    if let Expr::Cast { ty, value, .. } = value.unlocated() {
        let destination = type_range(ty)?;
        let source = initializer_range(value, prefix).unwrap_or(destination);
        // Only range-preserving widening retains the source interval. Narrowing
        // and otherwise unknown conversions use the entire destination range.
        return Some(
            if source.low >= destination.low && source.high <= destination.high {
                source
            } else {
                destination
            },
        );
    }
    expression_range(value, prefix)
}

fn expression_range(value: &Expr, prefix: &[Stmt]) -> Option<Interval> {
    if let Some(value) = integer(value) {
        return Some(Interval {
            low: value,
            high: value,
        });
    }
    let Expr::Name(name) = value.unlocated() else {
        return None;
    };
    local_range(name, prefix).map(|(value, _)| value)
}

fn block_updates(body: &Block, directions: &mut BTreeMap<String, i128>) -> Option<Updates> {
    let mut result = Updates::new();
    for statement in &body.statements {
        let updates = statement_updates(statement, directions)?;
        for (name, delta) in updates {
            let previous = result.entry(name).or_insert(Interval::ZERO);
            *previous = previous.add(delta)?;
        }
    }
    Some(result)
}

fn statement_updates(statement: &Stmt, directions: &mut BTreeMap<String, i128>) -> Option<Updates> {
    match statement.unlocated() {
        Stmt::Assign { target, .. } => {
            let Expr::Name(name) = target.unlocated() else {
                return None;
            };
            let delta = super::counter_delta(statement, name)?;
            let direction = directions.entry(name.clone()).or_insert(delta.signum());
            if *direction != delta.signum() {
                return None;
            }
            Some(BTreeMap::from([(
                name.clone(),
                Interval {
                    low: delta,
                    high: delta,
                },
            )]))
        }
        Stmt::Block(body) => block_updates(body, directions),
        Stmt::Expr(value) => conditional_updates(value, directions),
        // Local declarations, jumps, nested loops, and nonadditive assignments
        // require richer identity/exit summaries and are deliberately rejected.
        _ => None,
    }
}

fn conditional_updates(value: &Expr, directions: &mut BTreeMap<String, i128>) -> Option<Updates> {
    let Expr::If { subject, arms } = value.unlocated() else {
        return None;
    };
    if arms.is_empty() || !subject.as_deref().is_none_or(pure) {
        return None;
    }
    let mut paths = Vec::new();
    for (patterns, body) in arms {
        if !patterns.iter().all(pure_pattern) {
            return None;
        }
        paths.push(block_updates(body, directions)?);
    }
    let mut result = Updates::new();
    for path in &paths {
        for name in path.keys() {
            let range = paths.iter().fold(None, |prior: Option<Interval>, path| {
                let range = path.get(name).copied().unwrap_or(Interval::ZERO);
                Some(prior.map_or(range, |prior| prior.union(range)))
            })?;
            result.insert(name.clone(), range);
        }
    }
    Some(result)
}

fn pure_pattern(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Literal(value) => pure(value),
        _ => false,
    }
}

fn pure(value: &Expr) -> bool {
    match value.unlocated() {
        Expr::Name(_) | Expr::Int(_) | Expr::Bool(_) => true,
        Expr::Unary { value, .. } => pure(value),
        Expr::Binary { left, right, .. } => pure(left) && pure(right),
        _ => false,
    }
}

fn iteration_counts(
    start: Interval,
    bound: Interval,
    step: Interval,
    comparison: BinaryOp,
    representation: Interval,
) -> Option<Interval> {
    let (start, bound, step, representation, inclusive) = match comparison {
        BinaryOp::Lt | BinaryOp::Le if step.low > 0 => (
            start,
            bound,
            step,
            representation,
            comparison == BinaryOp::Le,
        ),
        BinaryOp::Gt | BinaryOp::Ge if step.high < 0 => (
            negate(start)?,
            negate(bound)?,
            negate(step)?,
            negate(representation)?,
            comparison == BinaryOp::Ge,
        ),
        // Endpoint intervals do not retain the residue classes required by !=.
        _ => return None,
    };
    let adjustment = i128::from(inclusive);
    let counts = Interval {
        low: ceiling(
            bound.low.checked_sub(start.high)?.checked_add(adjustment)?,
            step.high,
        )?,
        high: ceiling(
            bound.high.checked_sub(start.low)?.checked_add(adjustment)?,
            step.low,
        )?,
    };
    if counts.high != 0 {
        // Any true sample is at most this boundary. Include the largest whole
        // update there, even on the final iteration and on a nonuniform path.
        let last = bound.high.checked_sub(i128::from(!inclusive))?;
        if last.checked_add(step.high)? > representation.high {
            return None;
        }
    }
    Some(counts)
}

fn negate(range: Interval) -> Option<Interval> {
    Some(Interval {
        low: range.high.checked_neg()?,
        high: range.low.checked_neg()?,
    })
}

fn ceiling(distance: i128, stride: i128) -> Option<i128> {
    if distance <= 0 {
        Some(0)
    } else {
        distance
            .checked_div(stride)?
            .checked_add(i128::from(distance % stride != 0))
    }
}

fn envelope(name: &str, delta: Interval) -> Option<Vec<Stmt>> {
    let low = displacement(name, delta.low)?;
    if delta.low == delta.high {
        return Some(low);
    }
    // No condition is evaluated. The termination analyser visits both arms.
    // Independent per-binding choices are an overapproximation of real paths.
    Some(vec![Stmt::Expr(Expr::If {
        subject: None,
        arms: vec![
            (
                vec![Pattern::Literal(Box::new(Expr::Bool(true)))],
                Block { statements: low },
            ),
            (
                vec![Pattern::Wildcard],
                Block {
                    statements: displacement(name, delta.high)?,
                },
            ),
        ],
    })])
}

fn displacement(name: &str, delta: i128) -> Option<Vec<Stmt>> {
    if delta == 0 {
        return Some(vec![]);
    }
    let magnitude = u64::try_from(delta.checked_abs()?).ok()?;
    Some(vec![Stmt::Assign {
        target: Expr::Name(name.to_owned()),
        value: Expr::Binary {
            left: Box::new(Expr::Name(name.to_owned())),
            op: if delta < 0 {
                BinaryOp::Sub
            } else {
                BinaryOp::Add
            },
            right: Box::new(Expr::Int(magnitude.to_string())),
        },
    }])
}
