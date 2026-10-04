//! Proof-only call frames, value snapshots, and certified helper-loop summaries.
use super::{Binding, Block, Expr, Function, HashMap, Pattern, Proof, Scope, Stmt, Type};
use crate::ast::{BinaryOp, UnaryOp, VarDecl};

#[path = "optimizer_loops_results.rs"]
mod results;
pub(super) use results::consume_results;

struct ReturnSamples {
    name: String,
    values: Vec<Expr>,
}

#[cfg(test)]
#[path = "optimizer_loops_helpers_tests.rs"]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultUse {
    Exact,
    Void,
    Condition,
}

impl Proof<'_, '_> {
    pub(super) fn expanded_call(
        &mut self,
        call: &Expr,
        caller: &Scope,
        void: bool,
    ) -> Option<(Block, Expr)> {
        self.expand_call(
            call,
            caller,
            if void {
                ResultUse::Void
            } else {
                ResultUse::Exact
            },
        )
    }

    pub(super) fn condition_call(&mut self, call: &Expr, caller: &Scope) -> Option<(Block, Expr)> {
        self.expand_call(call, caller, ResultUse::Condition)
    }

    fn expand_call(
        &mut self,
        call: &Expr,
        caller: &Scope,
        result_use: ResultUse,
    ) -> Option<(Block, Expr)> {
        let Expr::Call {
            callee,
            args,
            generics,
        } = call.unlocated()
        else {
            return None;
        };
        if !generics.is_empty() {
            return None;
        }
        // Match call_value: resolve the callable before *any* argument effects.
        let (function, mut scope) = self.callable(callee, caller)?;
        if function.params.len() != args.len()
            || (function.return_type == Type::void()) != (result_use == ResultUse::Void)
        {
            return None;
        }
        let identity = std::ptr::from_ref(function) as usize;
        if self.active.contains(&identity) {
            return None;
        }
        self.call_frame(function, args, caller, &mut scope, result_use)
    }

    fn call_frame(
        &mut self,
        function: &Function,
        args: &[Expr],
        caller: &Scope,
        scope: &mut Scope,
        result_use: ResultUse,
    ) -> Option<(Block, Expr)> {
        let mut prefix = Block { statements: vec![] };
        let mut snapshots = Vec::new();
        for (parameter, original) in function.params.iter().zip(args) {
            // Different types require evaluator coercion. Do not erase it in a
            // proof tree with no expression-type metadata.
            if !super::arguments::value_type(&parameter.ty)
                || self.evaluator.expr_type(original)? != &parameter.ty
            {
                return None;
            }
            let (effects, argument) = self.effect_expression(original, caller)?;
            prefix.statements.extend(effects.statements);
            let name = self.local(&parameter.name, scope);
            prefix
                .statements
                .push(snapshot(&name, &parameter.ty, argument.clone()));
            if !parameter_written(&function.body, &parameter.name)
                && super::arguments::literal(&argument)
            {
                scope.insert(
                    parameter.name.clone(),
                    Binding {
                        expression: argument.clone(),
                        value: None,
                    },
                );
            }
            snapshots.push((prefix.statements.len(), name, argument));
        }
        // Argument evaluation precedes entry: f(f(x)) is not recursion.
        let identity = std::ptr::from_ref(function) as usize;
        if !self.active.insert(identity) {
            return None;
        }
        let result = self.finish_frame(function, scope, prefix, snapshots, result_use);
        self.active.remove(&identity);
        result
    }

    fn finish_frame(
        &mut self,
        function: &Function,
        scope: &Scope,
        mut prefix: Block,
        snapshots: Vec<(usize, String, Expr)>,
        result_use: ResultUse,
    ) -> Option<(Block, Expr)> {
        // This spelling cannot collide with a source-language label.
        let label = format!("<helper-exit:{}>", self.fresh());
        let mut returns = ReturnSamples {
            name: format!("<helper-result:{}>", self.fresh()),
            values: Vec::new(),
        };
        let body = self.helper_block(&function.body, scope, function, &label, &mut returns)?;
        let body = super::arguments::project_frame(body, &prefix, &snapshots, &mut returns.values);
        let uniform = helper_result(&returns.values, result_use);
        let body = if uniform.is_some() || result_use != ResultUse::Exact {
            results::remove_samples(body, &returns.name)
        } else {
            body
        };
        let body = consume_results(&body);
        // Collect the entire frame's writes before using known immutable values
        // in loop certificates, including writes occurring after a nested loop.
        let mut body = self.certify_helper_loops(&body)?;
        let last_exit = matches!(body.statements.last(), Some(Stmt::Break(None, Some(target))) if target == &label);
        if last_exit && !has_exit(&body.statements[..body.statements.len() - 1], &label) {
            body.statements.pop();
        }
        if has_exit(&body.statements, &label) {
            prefix.statements.push(Stmt::LabeledIf {
                label,
                value: Expr::If {
                    subject: None,
                    arms: vec![(vec![Pattern::Wildcard], body)],
                },
            });
        } else {
            prefix.statements.extend(body.statements);
        }
        let mut result = if function.return_type == Type::void() {
            Expr::None
        } else {
            // Exact nonuniform results retain their return-site storage. Only
            // outer conditions may use a termination-only Boolean envelope.
            uniform.or_else(|| {
                (result_use == ResultUse::Exact
                    && super::arguments::value_type(&function.return_type))
                .then(|| Expr::Name(returns.name.clone()))
            })?
        };
        for (start, name, argument) in snapshots.into_iter().rev() {
            let suffix = Block {
                statements: prefix.statements[start..].to_vec(),
            };
            let mut writes = super::HashSet::new();
            super::block_writes(&suffix, &mut writes);
            // A copied parameter is not an alias, even when its initializer
            // stays stable. Writes to the copy must retain its own storage.
            let mut stable = !writes.contains(&name);
            crate::visit::item(
                &super::super::Item::Statement(Stmt::Expr(argument.clone())),
                &mut |value| {
                    if let Expr::Name(name) = value.unlocated() {
                        stable &= !writes.contains(name);
                    }
                },
            );
            if stable {
                let substitutions = HashMap::from([(name, argument)]);
                let rewritten = super::arguments::simplify(super::substitute_proof_statement(
                    Stmt::Block(prefix),
                    &substitutions,
                ));
                let Stmt::Block(block) = rewritten else {
                    unreachable!()
                };
                prefix = block;
                // A substituted snapshot has no remaining storage identity.
                // Its pure value computation cannot affect a termination rank;
                // the original evaluator still performs it and checks failures.
                prefix.statements.remove(start - 1);
                let Stmt::Expr(value) = super::arguments::simplify(
                    super::substitute_proof_statement(Stmt::Expr(result), &substitutions),
                ) else {
                    unreachable!()
                };
                result = value;
            }
        }
        Some((prefix, result))
    }

    pub(super) fn effect_expression(
        &mut self,
        value: &Expr,
        scope: &Scope,
    ) -> Option<(Block, Expr)> {
        match value.unlocated() {
            Expr::Call { .. } => self.return_call(value, scope),
            Expr::Cast {
                ty,
                value,
                implicit,
            } => {
                let (prefix, value) = self.effect_expression(value, scope)?;
                Some((
                    prefix,
                    Expr::Cast {
                        ty: ty.clone(),
                        value: Box::new(value),
                        implicit: *implicit,
                    },
                ))
            }
            Expr::Unary { op, value } => {
                let (prefix, value) = self.effect_expression(value, scope)?;
                Some((
                    prefix,
                    Expr::Unary {
                        op: *op,
                        value: Box::new(value),
                    },
                ))
            }
            Expr::Binary { left, op, right } => self.binary_effects(left, *op, right, scope),
            Expr::Array(_)
            | Expr::Tuple(_)
            | Expr::Map(_)
            | Expr::Index { .. }
            | Expr::Member { .. } => self.aggregate_expression(value, scope),
            _ => Some((Block { statements: vec![] }, self.expression(value, scope)?)),
        }
    }

    fn helper_block(
        &mut self,
        body: &Block,
        scope: &Scope,
        function: &Function,
        label: &str,
        returns: &mut ReturnSamples,
    ) -> Option<Block> {
        let mut scope = scope.clone();
        let mut statements = Vec::new();
        for statement in &body.statements {
            match statement.unlocated() {
                Stmt::Return(value) => {
                    if let Some(value) = value {
                        if self.evaluator.expr_type(value)? != &function.return_type {
                            return None;
                        }
                        let (effects, value) = self.effect_expression(value, &scope)?;
                        statements.extend(effects.statements);
                        statements.push(snapshot(
                            &returns.name,
                            &function.return_type,
                            value.clone(),
                        ));
                        returns.values.push(value);
                    } else if function.return_type != Type::void() {
                        return None;
                    }
                    statements.push(Stmt::Break(None, Some(label.to_owned())));
                    break;
                }
                Stmt::Block(body) => statements.push(Stmt::Block(
                    self.helper_block(body, &scope, function, label, returns)?,
                )),
                Stmt::Expr(value) if matches!(value.unlocated(), Expr::If { .. }) => {
                    statements.push(Stmt::Expr(
                        self.helper_if(value, &scope, function, label, returns)?,
                    ));
                }
                Stmt::LabeledIf {
                    label: conditional_label,
                    value,
                } if matches!(value.unlocated(), Expr::If { .. }) => {
                    statements.push(Stmt::LabeledIf {
                        label: conditional_label.clone(),
                        value: self.helper_if(value, &scope, function, label, returns)?,
                    });
                }
                Stmt::While { .. } => {
                    let Stmt::While {
                        label: loop_label,
                        condition,
                        body,
                    } = statement.unlocated()
                    else {
                        unreachable!()
                    };
                    let expanded = Stmt::While {
                        label: loop_label.clone(),
                        condition: self.expression(condition, &scope)?,
                        body: self.helper_block(body, &scope, function, label, returns)?,
                    };
                    statements.push(expanded);
                }
                Stmt::For { .. } => {
                    statements
                        .extend(self.helper_for(statement, &scope, function, label, returns)?);
                }
                _ => statements.extend(self.statement(statement, &mut scope)?),
            }
        }
        Some(Block { statements })
    }

    fn helper_for(
        &mut self,
        statement: &Stmt,
        scope: &Scope,
        function: &Function,
        exit_label: &str,
        returns: &mut ReturnSamples,
    ) -> Option<Vec<Stmt>> {
        let Stmt::For {
            label,
            name,
            iterable,
            body,
        } = statement.unlocated()
        else {
            return None;
        };
        let iterable = self.expression(iterable, scope)?;
        let Expr::Array(elements) = &iterable else {
            return None;
        };
        let count = i128::try_from(elements.len()).ok()?;
        let mut inner = scope.clone();
        let name = self.local(name, &mut inner);
        let body = self.helper_block(body, &inner, function, exit_label, returns)?;
        Some(summarize_updates(&body, count).unwrap_or_else(|| {
            vec![Stmt::For {
                label: label.clone(),
                name,
                iterable,
                body,
            }]
        }))
    }

    fn helper_if(
        &mut self,
        value: &Expr,
        scope: &Scope,
        function: &Function,
        label: &str,
        returns: &mut ReturnSamples,
    ) -> Option<Expr> {
        let Expr::If { subject, arms } = value.unlocated() else {
            return None;
        };
        let subject = match subject.as_deref() {
            Some(value) => Some(self.expression(value, scope)?),
            None => None,
        };
        let mut expanded = Vec::new();
        for (patterns, body) in arms {
            let mut inner = scope.clone();
            let patterns = patterns
                .iter()
                .map(|pattern| self.comparison_pattern(pattern, &mut inner))
                .collect::<Option<_>>()?;
            expanded.push((
                patterns,
                self.helper_block(body, &inner, function, label, returns)?,
            ));
        }
        Some(Expr::If {
            subject: subject.map(Box::new),
            arms: expanded,
        })
    }
}

fn helper_result(returns: &[Expr], result_use: ResultUse) -> Option<Expr> {
    let first = returns.first()?;
    if returns
        .iter()
        .all(|value| format!("{value:?}") == format!("{first:?}"))
    {
        return Some(first.clone());
    }
    if result_use != ResultUse::Condition {
        return None;
    }
    condition_envelope(returns)
}

fn condition_envelope(returns: &[Expr]) -> Option<Expr> {
    let mut envelope: Option<(String, BinaryOp, i128)> = None;
    for value in returns {
        // False stops the actual loop, so it adds no true sample to the rank.
        if matches!(value.unlocated(), Expr::Bool(false)) {
            continue;
        }
        let Expr::Binary { left, op, right } = value.unlocated() else {
            return None;
        };
        let Expr::Name(counter) = left.unlocated() else {
            return None;
        };
        if !matches!(
            op,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        ) {
            return None;
        }
        let bound = integer(right)?;
        if let Some((prior_counter, prior_op, prior_bound)) = &mut envelope {
            if prior_counter != counter || prior_op != op {
                return None;
            }
            *prior_bound = if matches!(op, BinaryOp::Lt | BinaryOp::Le) {
                (*prior_bound).max(bound)
            } else {
                (*prior_bound).min(bound)
            };
        } else {
            envelope = Some((counter.clone(), *op, bound));
        }
    }
    let (counter, op, bound) = envelope?;
    // Every real true sample satisfies this comparison. The ordinary condition
    // certificate checks all prefix paths and excursions against its wider
    // horizon, including hypothetical extra iterations and the final check.
    Some(Expr::Binary {
        left: Box::new(Expr::Name(counter)),
        op,
        right: Box::new(integer_expression(bound)?),
    })
}

// Track direct writes through lexical blocks before substituting known arguments.
// Unsupported expression-local control flow is rejected by expansion separately.
fn parameter_written(body: &Block, parameter: &str) -> bool {
    for statement in &body.statements {
        let written = match statement.unlocated() {
            Stmt::Assign { target, .. } => super::root_name(target) == Some(parameter),
            Stmt::Block(body) | Stmt::While { body, .. } => parameter_written(body, parameter),
            Stmt::For { name, body, .. } => name != parameter && parameter_written(body, parameter),
            Stmt::Expr(value) | Stmt::LabeledIf { value, .. } => {
                if let Expr::If { arms, .. } = value.unlocated() {
                    arms.iter()
                        .any(|(_, body)| parameter_written(body, parameter))
                } else {
                    false
                }
            }
            Stmt::Var(declaration) if declaration.binding_names().contains(&parameter) => {
                // This declaration shadows the parameter for the rest of this
                // block, but never changes the binding in its enclosing scope.
                return false;
            }
            _ => false,
        };
        if written {
            return true;
        }
    }
    false
}

fn has_exit(statements: &[Stmt], label: &str) -> bool {
    statements
        .iter()
        .any(|statement| match statement.unlocated() {
            Stmt::Break(None, Some(target)) => target == label,
            Stmt::Block(body) | Stmt::While { body, .. } | Stmt::For { body, .. } => {
                has_exit(&body.statements, label)
            }
            Stmt::Expr(Expr::If { arms, .. })
            | Stmt::LabeledIf {
                value: Expr::If { arms, .. },
                ..
            } => arms
                .iter()
                .any(|(_, body)| has_exit(&body.statements, label)),
            _ => false,
        })
}

pub(super) fn snapshot(name: &str, ty: &Type, value: Expr) -> Stmt {
    Stmt::Var(VarDecl {
        source_path: "<proof>".into(),
        span: 0..0,
        public: false,
        mutable: false,
        mutex: false,
        pattern: Pattern::Name(name.to_owned()),
        ty: ty.clone(),
        value,
    })
}

pub(super) fn summarize_loop(statement: &Stmt, prefix: &[Stmt]) -> Option<Vec<Stmt>> {
    literal_summary(statement, prefix)
        .or_else(|| super::summaries::summarize_loop(statement, prefix))
}

fn literal_summary(statement: &Stmt, prefix: &[Stmt]) -> Option<Vec<Stmt>> {
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
    // Only a literal-initialized local is invariant at every helper entry.
    // Never borrow a caller cell's current value for subsequent invocations.
    let declaration = prefix.iter().rev().find_map(|statement| match statement.unlocated() {
        Stmt::Var(declaration) if matches!(&declaration.pattern, Pattern::Name(name) if name == counter) => Some(declaration),
        _ => None,
    })?;
    let start = integer(&declaration.value)?;
    let bound = integer(right)?;
    let range = integer_range(&declaration.ty)?;
    let declaration_index = prefix.iter().position(|statement| {
        matches!(statement.unlocated(), Stmt::Var(value) if std::ptr::eq(value, declaration))
    })?;
    let mut writes = super::HashSet::new();
    super::block_writes(
        &Block {
            statements: prefix[declaration_index + 1..].to_vec(),
        },
        &mut writes,
    );
    if writes.contains(counter) {
        return None;
    }
    let certificate = crate::termination::counted_loop(condition, body)?;
    if !certificate.terminates(start, bound, range.0, range.1) {
        return None;
    }
    let summary = update_deltas(body).and_then(|deltas| {
        let step = *deltas.get(counter)?;
        let count = iterations(start, bound, step, *op)?;
        repeated_updates(deltas, count)
    });
    // Certified rank-neutral loops may retain local jumps and early helper exits.
    // The enclosing certificate rejects any unsummarized protected-state write.
    Some(summary.unwrap_or_else(|| vec![statement.clone()]))
}

fn summarize_updates(body: &Block, count: i128) -> Option<Vec<Stmt>> {
    repeated_updates(update_deltas(body)?, count)
}

fn update_deltas(body: &Block) -> Option<HashMap<String, i128>> {
    let mut deltas: HashMap<String, i128> = HashMap::new();
    for statement in &body.statements {
        let Stmt::Assign { target, .. } = statement.unlocated() else {
            return None;
        };
        let Expr::Name(name) = target.unlocated() else {
            return None;
        };
        let delta = super::counter_delta(statement, name)?;
        let previous = deltas.entry(name.clone()).or_default();
        if *previous != 0 && previous.signum() != delta.signum() {
            return None;
        }
        *previous = previous.checked_add(delta)?;
    }
    Some(deltas)
}

fn repeated_updates(deltas: HashMap<String, i128>, count: i128) -> Option<Vec<Stmt>> {
    if count == 0 {
        return Some(vec![]);
    }
    deltas
        .into_iter()
        .map(|(name, delta)| {
            let delta = delta.checked_mul(count)?;
            Some(Stmt::Assign {
                target: Expr::Name(name.clone()),
                value: Expr::Binary {
                    left: Box::new(Expr::Name(name)),
                    op: BinaryOp::Add,
                    right: Box::new(integer_expression(delta)?),
                },
            })
        })
        .collect()
}

pub(super) fn integer(value: &Expr) -> Option<i128> {
    match value.unlocated() {
        Expr::Int(text) => crate::lexer::integer(text).ok().map(i128::from),
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => integer(value)?.checked_neg(),
        _ => None,
    }
}
fn integer_expression(value: i128) -> Option<Expr> {
    let magnitude = u64::try_from(value.checked_abs()?).ok()?;
    let literal = Expr::Int(magnitude.to_string());
    Some(if value < 0 {
        Expr::Unary {
            op: UnaryOp::Neg,
            value: Box::new(literal),
        }
    } else {
        literal
    })
}
fn integer_range(ty: &Type) -> Option<(i128, i128)> {
    match ty {
        Type::Named(name, args) if args.is_empty() => match name.as_str() {
            "int" => Some((i128::from(i64::MIN), i128::from(i64::MAX))),
            "uint" => Some((0, i128::from(u64::MAX))),
            "byte" => Some((0, i128::from(u8::MAX))),
            _ => None,
        },
        _ => None,
    }
}
fn iterations(start: i128, bound: i128, step: i128, op: BinaryOp) -> Option<i128> {
    match op {
        BinaryOp::Lt | BinaryOp::Le if step > 0 => {
            let distance = bound
                .checked_sub(start)?
                .checked_add(i128::from(op == BinaryOp::Le))?;
            Some(if distance <= 0 {
                0
            } else {
                distance.checked_add(step - 1)? / step
            })
        }
        BinaryOp::Gt | BinaryOp::Ge if step < 0 => iterations(
            start.checked_neg()?,
            bound.checked_neg()?,
            step.checked_neg()?,
            if op == BinaryOp::Gt {
                BinaryOp::Lt
            } else {
                BinaryOp::Le
            },
        ),
        BinaryOp::Ne if step != 0 => {
            let distance = bound.checked_sub(start)?;
            (distance % step == 0 && distance / step >= 0).then_some(distance / step)
        }
        _ => None,
    }
}
