//! Ordered proof-only statement effects and immutable helper-loop inputs.
use super::helpers::{integer, scalar, snapshot, summarize_loop};
use super::{Binding, Block, Expr, Pattern, Proof, Scope, Stmt};
use crate::ast::VarDecl;

impl Proof<'_, '_> {
    pub(super) fn capture_before(
        &mut self,
        prefix: &mut Block,
        original: &Expr,
        value: Expr,
        suffix: &Block,
    ) -> Option<Expr> {
        if stable_expression(&value, suffix) {
            // This is a proof-only substitution. Pure failures remain evaluated
            // in their original order by the unchanged source evaluator.
            return Some(value);
        }
        let ty = self.evaluator.expr_type(original)?;
        if !scalar(ty) {
            return None;
        }
        let name = self.fresh();
        prefix.statements.push(snapshot(&name, ty, value));
        Some(Expr::Name(name))
    }

    pub(super) fn effect_assignment(
        &mut self,
        target: &Expr,
        value: &Expr,
        scope: &Scope,
    ) -> Option<Vec<Stmt>> {
        // NC copies the RHS before evaluating any target index/key. The target
        // path keeps binding identities, not snapshots of ancestor containers.
        let (mut prefix, rhs) = self.effect_expression(value, scope)?;
        let (suffix, target) = self.effect_target(target, scope)?;
        let value = self.capture_before(&mut prefix, value, rhs, &suffix)?;
        prefix.statements.extend(suffix.statements);
        let name = super::root_name(&target)?;
        if self
            .values
            .get(name)
            .is_some_and(|value| !self.evaluator.recursion_targets(value).is_empty())
        {
            return None;
        }
        self.writes.insert(name.to_owned());
        prefix.statements.push(Stmt::Assign { target, value });
        Some(prefix.statements)
    }

    fn effect_target(&mut self, target: &Expr, scope: &Scope) -> Option<(Block, Expr)> {
        match target.unlocated() {
            Expr::Name(_) | Expr::Discard => Some((
                Block { statements: vec![] },
                self.expression(target, scope)?,
            )),
            Expr::Member { object, name } => {
                let (prefix, object) = self.effect_target(object, scope)?;
                Some((
                    prefix,
                    Expr::Member {
                        object: Box::new(object),
                        name: name.clone(),
                    },
                ))
            }
            Expr::Index { .. } => self.indexed_target(target, scope),
            _ => None,
        }
    }

    fn indexed_target(&mut self, target: &Expr, scope: &Scope) -> Option<(Block, Expr)> {
        let mut indices = Vec::new();
        let root = target_indices(target, &mut indices);
        let root = self.expression(root, scope)?;
        let (prefix, indices) = self.effect_operands(&indices, scope)?;
        let mut indices = indices.into_iter();
        Some((prefix, rebuild_target(target, &root, &mut indices)?))
    }

    pub(super) fn effect_operands(
        &mut self,
        operands: &[&Expr],
        scope: &Scope,
    ) -> Option<(Block, Vec<Expr>)> {
        let mut prefix = Block { statements: vec![] };
        let mut values: Vec<Expr> = Vec::new();
        for original in operands {
            let (effects, value) = self.effect_expression(original, scope)?;
            // A later operand may change an earlier one's source binding.
            for (earlier, previous) in values.iter_mut().enumerate() {
                *previous = self.capture_before(
                    &mut prefix,
                    operands[earlier],
                    previous.clone(),
                    &effects,
                )?;
            }
            prefix.statements.extend(effects.statements);
            values.push(value);
        }
        Some((prefix, values))
    }

    pub(super) fn effect_declaration(
        &mut self,
        declaration: &VarDecl,
        scope: &mut Scope,
    ) -> Option<Vec<Stmt>> {
        if declaration.mutex {
            return None;
        }
        // Initializers use the old scope, even for a same-spelled declaration.
        let (mut prefix, value) = self.effect_expression(&declaration.value, scope)?;
        let mut rewritten = declaration.clone();
        rewritten.value = value.clone();
        rewritten.pattern = self.declaration_pattern(&declaration.pattern, scope)?;
        if !declaration.mutable
            && scalar(&declaration.ty)
            && self.evaluator.expr_type(&declaration.value)? == &declaration.ty
            && (integer(&value).is_some() || matches!(value, Expr::Bool(_)))
            && let Pattern::Name(name) = &declaration.pattern
            && name != "_"
        {
            scope.insert(
                name.clone(),
                Binding {
                    expression: value,
                    value: None,
                },
            );
        }
        prefix.statements.push(Stmt::Var(rewritten));
        Some(prefix.statements)
    }

    pub(super) fn certify_helper_loops(&self, body: &Block) -> Option<Block> {
        let mut statements = Vec::new();
        for statement in &body.statements {
            let rewritten = self.certify_helper_statement(statement)?;
            if matches!(rewritten, Stmt::While { .. }) {
                let mut prefix = Block {
                    statements: statements.clone(),
                };
                let mut loop_block = Block {
                    statements: vec![rewritten],
                };
                let mut unused = Expr::None;
                // Only non-cell integer values with no writes in the complete
                // expanded frame are substituted. Mutable global storage and
                // copied writable parameters can never borrow these constants.
                self.substitute_constants(&mut unused, &mut prefix, &mut loop_block);
                statements.extend(summarize_loop(
                    &loop_block.statements[0],
                    &prefix.statements,
                )?);
            } else {
                statements.push(rewritten);
            }
        }
        Some(Block { statements })
    }

    fn certify_helper_statement(&self, statement: &Stmt) -> Option<Stmt> {
        Some(match statement.unlocated() {
            Stmt::While {
                label,
                condition,
                body,
            } => Stmt::While {
                label: label.clone(),
                condition: condition.clone(),
                body: self.certify_helper_loops(body)?,
            },
            Stmt::For {
                label,
                name,
                iterable,
                body,
            } => Stmt::For {
                label: label.clone(),
                name: name.clone(),
                iterable: iterable.clone(),
                body: self.certify_helper_loops(body)?,
            },
            Stmt::Block(body) => Stmt::Block(self.certify_helper_loops(body)?),
            Stmt::Expr(value) => Stmt::Expr(self.certify_helper_expression(value)?),
            Stmt::LabeledIf { label, value } => Stmt::LabeledIf {
                label: label.clone(),
                value: self.certify_helper_expression(value)?,
            },
            _ => statement.clone(),
        })
    }

    fn certify_helper_expression(&self, value: &Expr) -> Option<Expr> {
        let Expr::If { subject, arms } = value.unlocated() else {
            return Some(value.clone());
        };
        Some(Expr::If {
            subject: subject.clone(),
            arms: arms
                .iter()
                .map(|(patterns, body)| Some((patterns.clone(), self.certify_helper_loops(body)?)))
                .collect::<Option<_>>()?,
        })
    }
}

fn stable_expression(value: &Expr, suffix: &Block) -> bool {
    let mut writes = super::HashSet::new();
    super::block_writes(suffix, &mut writes);
    let mut stable = true;
    crate::visit::item(
        &super::super::Item::Statement(Stmt::Expr(value.clone())),
        &mut |value| {
            if let Expr::Name(name) = value.unlocated() {
                stable &= !writes.contains(name);
            }
        },
    );
    stable
}

fn target_indices<'a>(target: &'a Expr, indices: &mut Vec<&'a Expr>) -> &'a Expr {
    match target.unlocated() {
        Expr::Index { object, index } => {
            let root = target_indices(object, indices);
            indices.push(index);
            root
        }
        Expr::Member { object, .. } => target_indices(object, indices),
        _ => target,
    }
}

fn rebuild_target(
    target: &Expr,
    root: &Expr,
    indices: &mut impl Iterator<Item = Expr>,
) -> Option<Expr> {
    Some(match target.unlocated() {
        Expr::Index { object, .. } => Expr::Index {
            object: Box::new(rebuild_target(object, root, indices)?),
            index: Box::new(indices.next()?),
        },
        Expr::Member { object, name } => Expr::Member {
            object: Box::new(rebuild_target(object, root, indices)?),
            name: name.clone(),
        },
        _ => root.clone(),
    })
}
