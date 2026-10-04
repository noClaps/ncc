//! Ordered value-container arguments and guarded Boolean effects for proofs.
use super::helpers::{integer, snapshot};
use super::{Block, Expr, Pattern, Proof, Scope, Stmt, Type};
use crate::ast::{BinaryOp, UnaryOp};

/// Only structural, built-in value types may lose their evaluator metadata.
/// Nominal types, callables and futures need identities/coercions not modeled here.
pub(super) fn value_type(ty: &Type) -> bool {
    match ty {
        Type::Named(name, args) => {
            args.is_empty()
                && matches!(
                    name.as_str(),
                    "int" | "uint" | "byte" | "bool" | "float" | "char" | "str"
                )
        }
        Type::Array(value, _) | Type::Optional(value) | Type::ErrorUnion(value) => {
            value_type(value)
        }
        Type::Map(key, value) => value_type(key) && value_type(value),
        Type::Tuple(values) => values.iter().all(value_type),
        _ => false,
    }
}

/// Closed literal copies can be substituted without borrowing caller storage.
pub(super) fn literal(value: &Expr) -> bool {
    match value.unlocated() {
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Bytes(_)
        | Expr::None => true,
        Expr::Array(values) | Expr::Tuple(values) => values.iter().all(literal),
        Expr::Map(entries) => entries
            .iter()
            .all(|(key, value)| literal(key) && literal(value)),
        Expr::Unary { value, .. } => integer(value).is_some(),
        _ => false,
    }
}

impl Proof<'_, '_> {
    fn known_boolean(&self, value: &Expr) -> Option<bool> {
        match value.unlocated() {
            Expr::Bool(value) => Some(*value),
            Expr::Name(name) => match self.values.get(name)? {
                super::super::Value::Bool(value) => Some(*value),
                // Mutable cells are not fixed values, even before their first write.
                _ => None,
            },
            Expr::Unary {
                op: UnaryOp::Not,
                value,
            } => Some(!self.known_boolean(value)?),
            Expr::Binary { left, op, right } => {
                let left = self.known_boolean(left)?;
                let right = self.known_boolean(right)?;
                match op {
                    BinaryOp::And => Some(left && right),
                    BinaryOp::Or => Some(left || right),
                    BinaryOp::Eq => Some(left == right),
                    BinaryOp::Ne => Some(left != right),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn aggregate_expression(
        &mut self,
        value: &Expr,
        scope: &Scope,
    ) -> Option<(Block, Expr)> {
        let operands: Vec<_> = match value.unlocated() {
            Expr::Array(values) | Expr::Tuple(values) => values.iter().collect(),
            Expr::Map(entries) => entries
                .iter()
                .flat_map(|(key, value)| [key, value])
                .collect(),
            Expr::Index { object, index } => vec![object, index],
            Expr::Member { object, .. } => vec![object],
            _ => return None,
        };
        let (prefix, values) = if let Expr::Index { object, index } = value.unlocated() {
            let (mut prefix, object) = self.effect_expression(object, scope)?;
            let (suffix, index) = self.effect_expression(index, scope)?;
            // The evaluator and C backend currently disagree when index effects
            // replace the object binding. Do not newly certify that case until
            // the intended read-index semantics are resolved.
            if !super::effects::stable_expression(&object, &suffix) {
                return None;
            }
            prefix.statements.extend(suffix.statements);
            (prefix, vec![object, index])
        } else {
            self.effect_operands(&operands, scope)?
        };
        let mut values = values.into_iter();
        let result = match value.unlocated() {
            Expr::Array(_) => Expr::Array(values.collect()),
            Expr::Tuple(_) => Expr::Tuple(values.collect()),
            Expr::Map(_) => {
                let mut entries = Vec::new();
                while let Some(key) = values.next() {
                    entries.push((key, values.next()?));
                }
                Expr::Map(entries)
            }
            Expr::Index { .. } => Expr::Index {
                object: Box::new(values.next()?),
                index: Box::new(values.next()?),
            },
            Expr::Member { name, .. } => Expr::Member {
                object: Box::new(values.next()?),
                name: name.clone(),
            },
            _ => unreachable!(),
        };
        Some((prefix, projection(result)))
    }

    pub(super) fn binary_effects(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        scope: &Scope,
    ) -> Option<(Block, Expr)> {
        let (mut prefix, mut left_value) = self.effect_expression(left, scope)?;
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            if let Some(known) = self.known_boolean(&left_value) {
                if known == (op == BinaryOp::Or) {
                    return Some((prefix, Expr::Bool(known)));
                }
                let (suffix, result) = self.effect_expression(right, scope)?;
                prefix.statements.extend(suffix.statements);
                return Some((prefix, result));
            }
            let (mut suffix, right_value) = self.effect_expression(right, scope)?;
            if !suffix.statements.is_empty() {
                // Each path supplies an exact sample. The continuation consumer
                // must see both paths; conditional-only progress is not progress.
                let name = format!("<helper-result:{}>", self.fresh());
                suffix.statements.push(snapshot(
                    &name,
                    &Type::Named("bool".into(), vec![]),
                    right_value,
                ));
                let shortcut = Block {
                    statements: vec![snapshot(
                        &name,
                        &Type::Named("bool".into(), vec![]),
                        Expr::Bool(op == BinaryOp::Or),
                    )],
                };
                let (yes, no) = if op == BinaryOp::And {
                    (suffix, shortcut)
                } else {
                    (shortcut, suffix)
                };
                prefix.statements.push(Stmt::Expr(Expr::If {
                    subject: Some(Box::new(left_value)),
                    arms: vec![
                        (vec![Pattern::Literal(Box::new(Expr::Bool(true)))], yes),
                        (vec![Pattern::Wildcard], no),
                    ],
                }));
                return Some((prefix, Expr::Name(name)));
            }
            return Some((
                prefix,
                Expr::Binary {
                    left: Box::new(left_value),
                    op,
                    right: Box::new(right_value),
                },
            ));
        }
        let (suffix, right_value) = self.effect_expression(right, scope)?;
        left_value = self.capture_before(&mut prefix, left, left_value, &suffix)?;
        prefix.statements.extend(suffix.statements);
        Some((
            prefix,
            Expr::Binary {
                left: Box::new(left_value),
                op,
                right: Box::new(right_value),
            },
        ))
    }
}

/// Projections of expanded value copies only: never interpret calls or arithmetic.
/// Omitted operands still run in the unchanged source evaluator.
pub(super) fn projection(value: Expr) -> Expr {
    let projected = match value.unlocated() {
        Expr::Index { object, index } => match object.unlocated() {
            Expr::String(text) => string_index(text, index),
            Expr::Array(values) | Expr::Tuple(values) => integer(index)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| values.get(index))
                .cloned(),
            Expr::Map(entries)
                if literal(index)
                    && entries.iter().all(|(key, _)| {
                        key_kind(key) == key_kind(index) && key_kind(key).is_some()
                    }) =>
            {
                entries
                    .iter()
                    .rev()
                    .find(|(key, _)| same_key(key, index))
                    .map(|(_, value)| value.clone())
            }
            _ => None,
        },
        Expr::Member { object, name } => match object.unlocated() {
            Expr::Tuple(values) => name
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index))
                .cloned(),
            Expr::String(text) if name == "len" => Some(Expr::Int(format!(
                "{}u",
                crate::unicode::boundaries(text).len(),
            ))),
            Expr::Array(values) if name == "len" => Some(Expr::Int(format!("{}u", values.len()))),
            Expr::Bytes(values) if name == "len" => Some(Expr::Int(format!("{}u", values.len()))),
            _ => None,
        },
        _ => None,
    };
    projected.unwrap_or(value)
}

fn string_index(text: &str, index: &Expr) -> Option<Expr> {
    let index = usize::try_from(integer(index)?).ok()?;
    let boundaries = crate::unicode::boundaries(text);
    let start = *boundaries.get(index)?;
    let end = boundaries.get(index + 1).copied().unwrap_or(text.len());
    Some(Expr::Char(text.get(start..end)?.to_owned()))
}

fn key_kind(value: &Expr) -> Option<u8> {
    match value.unlocated() {
        Expr::String(_) => Some(0),
        Expr::Int(_) if integer(value).is_some() => Some(1),
        Expr::Bool(_) => Some(2),
        Expr::Char(_) => Some(3),
        _ => None,
    }
}

fn same_key(left: &Expr, right: &Expr) -> bool {
    match (left.unlocated(), right.unlocated()) {
        (Expr::String(left), Expr::String(right)) | (Expr::Char(left), Expr::Char(right)) => {
            left == right
        }
        (Expr::Int(_), Expr::Int(_)) => integer(left) == integer(right),
        (Expr::Bool(left), Expr::Bool(right)) => left == right,

        _ => false,
    }
}

pub(super) fn project_frame(
    mut body: Block,
    prefix: &Block,
    snapshots: &[(usize, String, Expr)],
    returns: &mut [Expr],
) -> Block {
    for (start, name, argument) in snapshots.iter().rev() {
        let suffix = Block {
            statements: prefix.statements[*start..]
                .iter()
                .chain(&body.statements)
                .cloned()
                .collect(),
        };
        let mut writes = super::HashSet::new();
        super::block_writes(&suffix, &mut writes);
        if writes.contains(name) {
            continue;
        }
        let mut copies = super::HashMap::from([(name.to_owned(), argument.clone())]);
        collect_copies(&body, &writes, &mut copies);
        let Stmt::Block(projected) = copy_projections(Stmt::Block(body), &copies) else {
            unreachable!()
        };
        body = projected;
        for value in &mut *returns {
            let Stmt::Expr(projected) = copy_projections(Stmt::Expr(value.clone()), &copies) else {
                unreachable!()
            };
            *value = projected;
        }
    }
    let mut writes = super::HashSet::new();
    super::block_writes(&body, &mut writes);
    let mut copies = super::HashMap::new();
    collect_copies(&body, &writes, &mut copies);
    let Stmt::Block(projected) = copy_projections(Stmt::Block(body), &copies) else {
        unreachable!()
    };
    for value in returns {
        let Stmt::Expr(projected) = copy_projections(Stmt::Expr(value.clone()), &copies) else {
            unreachable!()
        };
        *value = projected;
    }
    projected
}

// Templates describe copied shapes, not aliases. Only their closed projections
// are used, so unknown elements can never borrow later caller/global values.
fn collect_copies(
    body: &Block,
    writes: &super::HashSet<String>,
    copies: &mut super::HashMap<String, Expr>,
) {
    for statement in &body.statements {
        match statement.unlocated() {
            Stmt::Var(declaration)
                if !declaration.mutable && !declaration.mutex && value_type(&declaration.ty) =>
            {
                if let Pattern::Name(name) = &declaration.pattern
                    && !writes.contains(name)
                {
                    let Stmt::Expr(value) = super::substitute_proof_statement(
                        Stmt::Expr(declaration.value.clone()),
                        copies,
                    ) else {
                        unreachable!()
                    };
                    if !name.starts_with("<helper-result:")
                        && matches!(
                            value.unlocated(),
                            Expr::Array(_)
                                | Expr::Tuple(_)
                                | Expr::Map(_)
                                | Expr::String(_)
                                | Expr::Bytes(_)
                        )
                    {
                        copies.insert(name.clone(), value);
                    }
                }
            }
            Stmt::Block(body) | Stmt::While { body, .. } | Stmt::For { body, .. } => {
                collect_copies(body, writes, copies);
            }
            Stmt::Expr(value) | Stmt::LabeledIf { value, .. } => {
                if let Expr::If { arms, .. } = value.unlocated() {
                    for (_, body) in arms {
                        collect_copies(body, writes, copies);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A writable parameter copy must keep its storage. For an unwritten copy,
/// closed projections are independent of later writes to the initializer's names.
fn copy_projections(statement: Stmt, substitutions: &super::HashMap<String, Expr>) -> Stmt {
    let mut item = super::super::Item::Statement(statement);
    crate::visit::rewrite(&mut item, &mut |value| {
        if matches!(value.unlocated(), Expr::Index { .. } | Expr::Member { .. }) {
            let rewritten = simplify(super::substitute_proof_statement(
                Stmt::Expr(value.clone()),
                substitutions,
            ));
            if let Stmt::Expr(projected) = rewritten
                && literal(&projected)
            {
                *value = projected;
            }
        }
    });
    let super::super::Item::Statement(statement) = item else {
        unreachable!()
    };
    statement
}

pub(super) fn simplify(statement: Stmt) -> Stmt {
    let mut item = super::super::Item::Statement(statement);
    crate::visit::rewrite(&mut item, &mut |value| *value = projection(value.clone()));
    let super::super::Item::Statement(statement) = item else {
        unreachable!()
    };
    statement
}

#[cfg(test)]
mod tests {
    use super::{Type, value_type};

    #[test]
    fn identity_bearing_values_are_never_structural_snapshot_types() {
        let int = Type::Named("int".into(), vec![]);
        for excluded in [
            Type::Function(vec![], Box::new(int.clone())),
            Type::Future(Box::new(int.clone())),
            Type::Named("Nominal".into(), vec![]),
        ] {
            assert!(!value_type(&excluded));
            assert!(!value_type(&Type::Array(Box::new(excluded.clone()), None)));
            assert!(!value_type(&Type::Map(
                Box::new(int.clone()),
                Box::new(excluded.clone())
            )));
            assert!(!value_type(&Type::Tuple(vec![int.clone(), excluded])));
        }
    }
}
