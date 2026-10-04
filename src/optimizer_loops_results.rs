//! Exact return-site samples and their proof-only continuations.
//!
//! No tree here may replace source code or be interpreted. Branch continuations
//! retain every possible return, rather than choosing a representative result.
use super::super::arguments::{simplify, value_type};
use super::super::{Block, Expr, HashMap, HashSet, Pattern, Stmt};

type Samples = HashMap<String, Expr>;
type Exits = HashMap<String, Continuation>;

#[derive(Clone)]
struct Continuation {
    statements: Vec<Stmt>,
    exits: Exits,
}

/// Consume exact helper snapshots in ordered proof statements. Call this after
/// appending the assignment/declaration consuming an `effect_expression` result,
/// not on the prefix alone: local helper exits must carry that continuation.
/// Unsupported loop-carried samples remain storage reads and lose the proof.
pub(crate) fn consume_results(body: &Block) -> Block {
    let mut found = false;
    crate::visit::item(
        &super::super::super::Item::Statement(Stmt::Block(body.clone())),
        &mut |value| {
            if let Expr::Name(name) = value.unlocated() {
                found |= name.starts_with("<helper-result:");
            }
        },
    );
    if !found {
        return body.clone();
    }
    sequence(&body.statements, &Samples::new(), &Exits::new())
}

fn sequence(statements: &[Stmt], samples: &Samples, exits: &Exits) -> Block {
    let Some((first, tail)) = statements.split_first() else {
        return Block { statements: vec![] };
    };
    match first.unlocated() {
        Stmt::Block(body) => sequence(&joined(&body.statements, tail), samples, exits),
        Stmt::Expr(value) if matches!(value.unlocated(), Expr::If { .. }) => {
            conditional(value, tail, samples, exits)
        }
        Stmt::LabeledIf { label, value }
            if matches!(value.unlocated(), Expr::If { .. }) && !blocked_exit(value, label) =>
        {
            let mut nested = exits.clone();
            nested.insert(
                label.clone(),
                Continuation {
                    statements: tail.to_vec(),
                    exits: exits.clone(),
                },
            );
            // Fallthrough leaves the label's scope just as an explicit
            // break does; caller labels must not see this frame's exit map.
            let fallthrough = [Stmt::Break(None, Some(label.clone()))];
            conditional(value, &fallthrough, samples, &nested)
        }
        Stmt::Break(None, Some(label)) if exits.contains_key(label) => {
            let continuation = &exits[label];
            sequence(&continuation.statements, samples, &continuation.exits)
        }
        Stmt::Break(..) | Stmt::Continue(..) | Stmt::Return(..) => Block {
            statements: vec![first.clone()],
        },
        _ => ordinary(first, tail, samples, exits),
    }
}

fn joined(body: &[Stmt], tail: &[Stmt]) -> Vec<Stmt> {
    body.iter().chain(tail).cloned().collect()
}

fn conditional(value: &Expr, tail: &[Stmt], samples: &Samples, exits: &Exits) -> Block {
    let Expr::If { subject, arms } = value.unlocated() else {
        unreachable!()
    };
    let mut rewritten: Vec<_> = arms
        .iter()
        .map(|(patterns, body)| {
            (
                patterns.clone(),
                sequence(&joined(&body.statements, tail), samples, exits),
            )
        })
        .collect();
    // Include implicit fallthrough. An impossible extra arm only loses proofs.
    let exhaustive = arms.iter().any(|(patterns, _)| {
        patterns.iter().any(|pattern| matches!(pattern, Pattern::Wildcard | Pattern::Name(_)))
    }) || [true, false].into_iter().all(|expected| {
        arms.iter().flat_map(|(patterns, _)| patterns).any(|pattern| {
            matches!(pattern, Pattern::Literal(value) if matches!(value.unlocated(), Expr::Bool(actual) if *actual == expected))
        })
    });
    if !exhaustive {
        rewritten.push((vec![Pattern::Wildcard], sequence(tail, samples, exits)));
    }
    Block {
        statements: vec![Stmt::Expr(Expr::If {
            subject: subject
                .as_ref()
                .map(|value| Box::new(substitute(value, samples))),
            arms: rewritten,
        })],
    }
}

fn ordinary(first: &Stmt, tail: &[Stmt], samples: &Samples, exits: &Exits) -> Block {
    let mut next = samples.clone();
    let statement = match first.unlocated() {
        Stmt::Var(declaration) => {
            let mut declaration = declaration.clone();
            declaration.value = substitute(&declaration.value, samples);
            if !declaration.mutable
                && !declaration.mutex
                && value_type(&declaration.ty)
                && pure_sample(&declaration.value)
                && let Pattern::Name(name) = &declaration.pattern
            {
                next.insert(name.clone(), declaration.value.clone());
            }
            Stmt::Var(declaration)
        }
        Stmt::Assign { target, value } => Stmt::Assign {
            // Canonical target identities are never replaced by copied values.
            target: target.clone(),
            value: substitute(value, samples),
        },
        _ => first.clone(),
    };
    invalidate(&statement, &mut next);
    let mut rest = sequence(tail, &next, exits);
    rest.statements.insert(0, statement);
    rest
}

// Sampling may copy a pure calculation, never defer or duplicate an effect.
fn pure_sample(value: &Expr) -> bool {
    match value.unlocated() {
        Expr::Name(_)
        | Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::String(_)
        | Expr::Char(_)
        | Expr::Bytes(_)
        | Expr::None => true,
        Expr::Array(values) | Expr::Tuple(values) => values.iter().all(pure_sample),
        Expr::Map(entries) => entries
            .iter()
            .all(|(key, value)| pure_sample(key) && pure_sample(value)),
        Expr::Index { object, index } => pure_sample(object) && pure_sample(index),
        Expr::Member { object, .. } => pure_sample(object),
        Expr::Unary { value, .. } | Expr::Cast { value, .. } => pure_sample(value),
        Expr::Binary { left, right, .. } => pure_sample(left) && pure_sample(right),
        _ => false,
    }
}

fn substitute(value: &Expr, samples: &Samples) -> Expr {
    let Stmt::Expr(value) = simplify(super::super::substitute_proof_statement(
        Stmt::Expr(value.clone()),
        samples,
    )) else {
        unreachable!()
    };
    value
}

fn invalidate(statement: &Stmt, samples: &mut Samples) {
    let mut writes = HashSet::new();
    super::super::block_writes(
        &Block {
            statements: vec![statement.clone()],
        },
        &mut writes,
    );
    samples.retain(|name, value| {
        if writes.contains(name) {
            return false;
        }
        let mut stable = true;
        crate::visit::item(
            &super::super::super::Item::Statement(Stmt::Expr(value.clone())),
            &mut |value| {
                if let Expr::Name(name) = value.unlocated() {
                    stable &= !writes.contains(name);
                }
            },
        );
        stable
    });
}

// Moving a continuation into a loop would execute it repeatedly. Keep such
// frames intact; the normal certificate must reject unsupported result reads.
// Value-carrying jumps also retain their value-producing frame.
fn blocked_exit(value: &Expr, label: &str) -> bool {
    let Expr::If { arms, .. } = value.unlocated() else {
        return false;
    };
    arms.iter().any(|(_, body)| block_blocked_exit(body, label))
}

fn block_blocked_exit(body: &Block, label: &str) -> bool {
    body.statements
        .iter()
        .any(|statement| match statement.unlocated() {
            Stmt::While { body, .. } | Stmt::For { body, .. } => {
                super::has_exit(&body.statements, label) || block_blocked_exit(body, label)
            }
            Stmt::Break(Some(_), Some(target)) => target == label,
            Stmt::Block(body) => block_blocked_exit(body, label),
            Stmt::Expr(value) | Stmt::LabeledIf { value, .. } => blocked_exit(value, label),
            _ => false,
        })
}

pub(super) fn remove_samples(body: Block, name: &str) -> Block {
    Block {
        statements: body
            .statements
            .into_iter()
            .filter_map(|statement| {
                if matches!(statement.unlocated(), Stmt::Var(declaration)
                if matches!(&declaration.pattern, Pattern::Name(binding) if binding == name))
                {
                    None
                } else {
                    Some(map_blocks(statement, &mut |body| {
                        remove_samples(body, name)
                    }))
                }
            })
            .collect(),
    }
}

fn map_blocks(statement: Stmt, rewrite: &mut impl FnMut(Block) -> Block) -> Stmt {
    match statement {
        Stmt::Block(body) => Stmt::Block(rewrite(body)),
        Stmt::While {
            label,
            condition,
            body,
        } => Stmt::While {
            label,
            condition,
            body: rewrite(body),
        },
        Stmt::For {
            label,
            name,
            iterable,
            body,
        } => Stmt::For {
            label,
            name,
            iterable,
            body: rewrite(body),
        },
        Stmt::Expr(value) => Stmt::Expr(map_arms(value, rewrite)),
        Stmt::LabeledIf { label, value } => Stmt::LabeledIf {
            label,
            value: map_arms(value, rewrite),
        },
        _ => statement,
    }
}

fn map_arms(value: Expr, rewrite: &mut impl FnMut(Block) -> Block) -> Expr {
    match value {
        Expr::If { subject, arms } => Expr::If {
            subject,
            arms: arms
                .into_iter()
                .map(|(patterns, body)| (patterns, rewrite(body)))
                .collect(),
        },
        _ => value,
    }
}
