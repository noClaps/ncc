//! Non-executing helper expansion for counted-loop certificates.
//! These trees never reach the evaluator: source trees retain their IDs and locations.
use super::{
    BinaryOp, Block, Evaluator, Expr, Function, HashMap, HashSet, Pattern, Stmt, Type, UnaryOp,
    Value,
};

#[derive(Clone)]
struct Binding {
    expression: Expr,
    value: Option<Value>,
}
type Scope = HashMap<String, Binding>;

#[cfg(test)]
#[path = "optimizer_loops_tests.rs"]
mod tests;

#[path = "optimizer_loops_helpers.rs"]
mod helpers;

#[path = "optimizer_loops_effects.rs"]
mod effects;

#[path = "optimizer_loops_summaries.rs"]
mod summaries;

#[path = "optimizer_loops_arguments.rs"]
mod arguments;

#[path = "optimizer_loops_nested.rs"]
mod nested;

struct Proof<'e, 'module> {
    evaluator: &'e Evaluator<'module>,
    values: HashMap<String, Value>,
    copies: usize,
    active: HashSet<usize>,
    callable_names: HashSet<String>,
    writes: HashSet<String>,
}

impl Evaluator<'_> {
    pub(super) fn helper_loop_proven(
        &self,
        condition: &Expr,
        body: &Block,
        label: Option<&str>,
        env: &HashMap<String, Value>,
    ) -> bool {
        let mut proof = Proof {
            evaluator: self,
            values: HashMap::new(),
            copies: 0,
            active: HashSet::new(),
            callable_names: HashSet::new(),
            writes: HashSet::new(),
        };
        proof.prepare(condition, body, label, env).is_some()
    }
}

impl<'module> Proof<'_, 'module> {
    fn fresh(&mut self) -> String {
        let name = format!("copy{}", self.copies);
        self.copies += 1;
        name
    }

    fn scope(&mut self, env: &HashMap<String, Value>) -> Scope {
        env.iter()
            .map(|(name, value)| {
                let canonical = match value {
                    Value::Cell(cell) => format!("cell{cell}"),
                    _ => self.fresh(),
                };
                self.values.insert(canonical.clone(), value.clone());
                (
                    name.clone(),
                    Binding {
                        expression: Expr::Name(canonical),
                        value: Some(value.clone()),
                    },
                )
            })
            .collect()
    }

    fn prepare(
        &mut self,
        condition: &Expr,
        body: &Block,
        label: Option<&str>,
        env: &HashMap<String, Value>,
    ) -> Option<()> {
        let scope = self.scope(env);
        let (mut prefix, mut comparison) = if matches!(condition.unlocated(), Expr::Call { .. }) {
            self.condition_call(condition, &scope)?
        } else {
            (
                Block { statements: vec![] },
                self.expression(condition, &scope)?,
            )
        };
        let mut body = helpers::consume_results(&self.block(body, &scope)?);
        if !self.writes.is_disjoint(&self.callable_names) {
            return None;
        }
        self.substitute_constants(&mut comparison, &mut prefix, &mut body);
        let prefix = self.certify_helper_loops(&prefix)?;
        let body = self.certify_helper_loops(&body)?;
        if self.counted_proven(&comparison, &body, &prefix, label)
            || self.generalized_proven(&comparison, &body, &prefix)
        {
            return Some(());
        }
        let mut prefix_writes = HashSet::new();
        block_writes(&prefix, &mut prefix_writes);
        self.varying_proven(&comparison, &body, &prefix, &prefix_writes)
            .then_some(())
    }

    fn counted_proven(
        &self,
        comparison: &Expr,
        body: &Block,
        prefix: &Block,
        label: Option<&str>,
    ) -> bool {
        let Expr::Binary { left, right, .. } = comparison.unlocated() else {
            return false;
        };
        let Expr::Name(counter) = left.unlocated() else {
            return false;
        };
        let Some(range) = self
            .values
            .get(counter)
            .and_then(|value| self.evaluator.loop_integer(value))
        else {
            return false;
        };
        let Some(bound) = self.bound_integer(right) else {
            return false;
        };
        crate::termination::condition_loop(comparison, prefix, body, label, range, bound)
    }

    fn substitute_constants(&self, comparison: &mut Expr, prefix: &mut Block, body: &mut Block) {
        let constants: HashMap<_, _> = self
            .values
            .iter()
            .filter(|(name, value)| {
                !self.writes.contains(*name) && !matches!(value, Value::Cell(_))
            })
            .filter_map(|(name, value)| {
                let (value, min, max) = self.evaluator.loop_integer(value)?;
                let suffix = if min == 0 && max == i128::from(u64::MAX) {
                    "u"
                } else {
                    ""
                };
                let expression = if value < 0 {
                    Expr::Unary {
                        op: UnaryOp::Neg,
                        value: Box::new(Expr::Int(value.checked_neg()?.to_string())),
                    }
                } else {
                    Expr::Int(format!("{value}{suffix}"))
                };
                Some((name.clone(), expression))
            })
            .collect();
        if let Stmt::Expr(value) =
            substitute_proof_statement(Stmt::Expr(comparison.clone()), &constants)
        {
            *comparison = value;
        }
        for block in [prefix, body] {
            if let Stmt::Block(rewritten) =
                substitute_proof_statement(Stmt::Block(block.clone()), &constants)
            {
                *block = rewritten;
            }
        }
    }

    fn generalized_proven(&self, comparison: &Expr, body: &Block, prefix: &Block) -> bool {
        let mut values: HashMap<_, _> = self
            .values
            .iter()
            .filter_map(|(name, value)| {
                self.evaluator
                    .loop_integer(value)
                    .map(|range| (name.clone(), range))
            })
            .collect();
        let mut body = body.clone();
        if general_prefix(&mut body, prefix, &mut values).is_none() {
            return false;
        }
        crate::termination::affine_loop(comparison, &body, &values)
            || crate::termination::growth_loop(comparison, &body, &values)
            || crate::termination::reset_loop(comparison, &body, &values)
    }

    fn varying_proven(
        &self,
        comparison: &Expr,
        body: &Block,
        prefix: &Block,
        prefix_writes: &HashSet<String>,
    ) -> bool {
        let Expr::Binary { left, right, .. } = comparison.unlocated() else {
            return false;
        };
        let (Expr::Name(counter), Expr::Name(bound)) = (left.unlocated(), right.unlocated()) else {
            return false;
        };
        let ranges = [counter, bound].map(|name| {
            self.values
                .get(name)
                .and_then(|value| self.evaluator.loop_integer(value))
        });
        let [Some(counter_range), Some(bound_range)] = ranges else {
            return false;
        };
        let mut body = body.clone();
        let mut ranges = [counter_range, bound_range];
        if (prefix_writes.contains(counter) || prefix_writes.contains(bound))
            && ranking_prefix(&mut body, prefix, [counter, bound], &mut ranges).is_none()
        {
            return false;
        }
        crate::termination::varying_loop(comparison, &body, ranges[0], ranges[1])
    }

    fn bound_integer(&self, expression: &Expr) -> Option<i128> {
        // No evaluation of cloned expressions: their checked type metadata is
        // intentionally absent. More elaborate bounds stay on the original path.
        match expression.unlocated() {
            Expr::Name(name) => self
                .evaluator
                .loop_integer(self.values.get(name)?)
                .map(|v| v.0),
            Expr::Int(text) => crate::lexer::integer(text).ok().map(i128::from),
            Expr::Unary {
                op: UnaryOp::Neg,
                value,
            } if matches!(value.unlocated(), Expr::Int(_)) => {
                self.bound_integer(value)?.checked_neg()
            }
            _ => None,
        }
    }

    fn callable(&mut self, callee: &Expr, scope: &Scope) -> Option<(&'module Function, Scope)> {
        let Expr::Name(name) = callee.unlocated() else {
            return None;
        };
        let callable = if let Some(binding) = scope.get(name) {
            let Expr::Name(canonical) = &binding.expression else {
                return None;
            };
            self.callable_names.insert(canonical.clone());
            let value = binding.value.as_ref()?;
            match value {
                Value::Cell(cell) => self.evaluator.cells.get(*cell)?.clone(),
                value => value.clone(),
            }
        } else {
            // A checked local/constant which is unavailable in this scope must
            // not accidentally resolve to a same-spelled module function.
            if self
                .evaluator
                .checked
                .constant_sources
                .contains_key(&callee.id())
            {
                return None;
            }
            Value::Function(name.clone())
        };
        let (function, env) = match callable {
            Value::Function(name) => (
                *self.evaluator.functions.get(&name)?,
                if self.evaluator.analyse_output {
                    self.evaluator.analysis_globals.clone()
                } else {
                    HashMap::new()
                },
            ),
            Value::Closure(key, captures) => (
                *self.evaluator.lambdas.get(&key)?,
                captures.into_iter().collect(),
            ),
            _ => return None,
        };
        let scope = self.scope(&env);
        Some((function, scope))
    }

    fn return_call(&mut self, call: &Expr, caller: &Scope) -> Option<(Block, Expr)> {
        self.expanded_call(call, caller, false)
    }

    fn void_call(&mut self, call: &Expr, caller: &Scope) -> Option<Block> {
        self.expanded_call(call, caller, true).map(|(body, _)| body)
    }

    fn expression(&mut self, expression: &Expr, scope: &Scope) -> Option<Expr> {
        Some(match expression.unlocated() {
            Expr::Name(name) => scope.get(name)?.expression.clone(),
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::None
            | Expr::Bytes(_)
            | Expr::Discard => expression.unlocated().clone(),
            Expr::Array(values) => Expr::Array(self.expressions(values, scope)?),
            Expr::Tuple(values) => Expr::Tuple(self.expressions(values, scope)?),
            Expr::Map(entries) => Expr::Map(
                entries
                    .iter()
                    .map(|(key, value)| {
                        Some((self.expression(key, scope)?, self.expression(value, scope)?))
                    })
                    .collect::<Option<_>>()?,
            ),
            Expr::Unary { op, value } => Expr::Unary {
                op: *op,
                value: Box::new(self.expression(value, scope)?),
            },
            Expr::Binary { left, op, right } => Expr::Binary {
                left: Box::new(self.expression(left, scope)?),
                op: *op,
                right: Box::new(self.expression(right, scope)?),
            },
            Expr::Cast {
                ty,
                value,
                implicit,
            } => Expr::Cast {
                ty: ty.clone(),
                value: Box::new(self.expression(value, scope)?),
                implicit: *implicit,
            },
            Expr::Index { object, index } => Expr::Index {
                object: Box::new(self.expression(object, scope)?),
                index: Box::new(self.expression(index, scope)?),
            },
            Expr::Member { object, name } => Expr::Member {
                object: Box::new(self.expression(object, scope)?),
                name: name.clone(),
            },
            Expr::Call { .. } => {
                let (prefix, result) = self.return_call(expression, scope)?;
                if !prefix.statements.is_empty() {
                    return None;
                }
                result
            }
            // Callable creation, asynchronous work, and embedded control flow
            // cannot be hidden inside arguments or value expressions.
            _ => return None,
        })
    }

    fn expressions(&mut self, values: &[Expr], scope: &Scope) -> Option<Vec<Expr>> {
        values
            .iter()
            .map(|value| self.expression(value, scope))
            .collect()
    }

    fn block(&mut self, block: &Block, scope: &Scope) -> Option<Block> {
        self.block_statements(&block.statements, scope)
    }

    fn block_statements(&mut self, source: &[Stmt], scope: &Scope) -> Option<Block> {
        self.statements_in_scope(source, &mut scope.clone())
    }

    fn statements_in_scope(&mut self, source: &[Stmt], scope: &mut Scope) -> Option<Block> {
        let mut statements = Vec::new();
        for statement in source {
            statements.extend(self.statement(statement, scope)?);
        }
        Some(Block { statements })
    }

    fn statement(&mut self, statement: &Stmt, scope: &mut Scope) -> Option<Vec<Stmt>> {
        // Helpers use separately certified loop summaries, never acyclicity alone.
        if !self.active.is_empty()
            && matches!(statement.unlocated(), Stmt::While { .. } | Stmt::For { .. })
        {
            return None;
        }
        let statement = match statement.unlocated() {
            Stmt::Assign { target, value } => return self.effect_assignment(target, value, scope),
            Stmt::Var(declaration) => return self.effect_declaration(declaration, scope),
            Stmt::Block(body) => Stmt::Block(self.block(body, scope)?),
            Stmt::Expr(value) if matches!(value.unlocated(), Expr::Call { .. }) => {
                if let Expr::Call {
                    callee,
                    args,
                    generics,
                } = value.unlocated()
                    && matches!(callee.unlocated(), Expr::Name(name) if name == "@print" || name == "@println")
                {
                    let (mut prefix, args) =
                        self.effect_operands(&args.iter().collect::<Vec<_>>(), scope)?;
                    prefix.statements.push(Stmt::Expr(Expr::Call {
                        callee: callee.clone(),
                        generics: generics.clone(),
                        args,
                    }));
                    return Some(prefix.statements);
                }
                return Some(vec![Stmt::Block(self.void_call(value, scope)?)]);
            }
            Stmt::Expr(value) => Stmt::Expr(self.control_expression(value, scope)?),
            Stmt::Assert(value) => {
                let (mut prefix, value) = self.effect_expression(value, scope)?;
                prefix.statements.push(Stmt::Assert(value));
                return Some(prefix.statements);
            }
            Stmt::LabeledIf { label, value } => Stmt::LabeledIf {
                label: label.clone(),
                value: self.control_expression(value, scope)?,
            },
            Stmt::While {
                label,
                condition,
                body,
            } => Stmt::While {
                label: label.clone(),
                condition: self.expression(condition, scope)?,
                body: self.block(body, scope)?,
            },
            Stmt::For {
                label,
                name,
                iterable,
                body,
            } => {
                let iterable = self.expression(iterable, scope)?;
                let mut inner = scope.clone();
                let name = self.local(name, &mut inner);
                Stmt::For {
                    label: label.clone(),
                    name,
                    iterable,
                    body: self.block(body, &inner)?,
                }
            }
            Stmt::Break(None, label) => Stmt::Break(None, label.clone()),
            Stmt::Continue(label) => Stmt::Continue(label.clone()),
            // Source returns exit the containing function. Expanded helper
            // returns are handled separately and must not exit the caller.
            Stmt::Return(value) if self.active.is_empty() => Stmt::Return(match value {
                Some(value) => Some(self.expression(value, scope)?),
                None => None,
            }),
            Stmt::Return(_) | Stmt::Throw(_) | Stmt::Break(Some(_), _) | Stmt::Lock { .. } => {
                return None;
            }
            Stmt::Located(_, _) => unreachable!(),
        };
        Some(vec![statement])
    }

    fn local(&mut self, name: &str, scope: &mut Scope) -> String {
        // Local initializers are not evaluated or entered in `values`. A local
        // ranking counter or bound therefore cannot borrow an outer value.
        let canonical = self.fresh();
        scope.insert(
            name.to_owned(),
            Binding {
                expression: Expr::Name(canonical.clone()),
                value: None,
            },
        );
        canonical
    }

    fn declaration_pattern(&mut self, pattern: &Pattern, scope: &mut Scope) -> Option<Pattern> {
        Some(match pattern {
            Pattern::Name(name) if name == "_" => pattern.clone(),
            Pattern::Name(name) => Pattern::Name(self.local(name, scope)),
            Pattern::Tuple(patterns) => Pattern::Tuple(
                patterns
                    .iter()
                    .map(|pattern| self.declaration_pattern(pattern, scope))
                    .collect::<Option<_>>()?,
            ),
            _ => return None,
        })
    }

    fn control_expression(&mut self, value: &Expr, scope: &Scope) -> Option<Expr> {
        let Expr::If { subject, arms } = value.unlocated() else {
            return self.expression(value, scope);
        };
        let subject = match subject.as_deref() {
            Some(value) => Some(Box::new(self.expression(value, scope)?)),
            None => None,
        };
        let arms = arms
            .iter()
            .map(|(patterns, body)| {
                let mut inner = scope.clone();
                let patterns = patterns
                    .iter()
                    .map(|pattern| self.comparison_pattern(pattern, &mut inner))
                    .collect::<Option<_>>()?;
                Some((patterns, self.block(body, &inner)?))
            })
            .collect::<Option<_>>()?;
        Some(Expr::If { subject, arms })
    }

    fn comparison_pattern(&mut self, pattern: &Pattern, scope: &mut Scope) -> Option<Pattern> {
        Some(match pattern {
            Pattern::Wildcard => Pattern::Wildcard,
            Pattern::Literal(value) => Pattern::Literal(Box::new(self.expression(value, scope)?)),
            Pattern::Name(name) => {
                if let Some(binding) = scope.get(name) {
                    Pattern::Literal(Box::new(binding.expression.clone()))
                } else {
                    Pattern::Name(self.local(name, scope))
                }
            }
            _ => return None,
        })
    }
}

fn substitute_proof_statement(statement: Stmt, constants: &HashMap<String, Expr>) -> Stmt {
    let mut item = super::Item::Statement(statement);
    crate::visit::rewrite(&mut item, &mut |expression| {
        if let Expr::Name(name) = expression.unlocated()
            && let Some(value) = constants.get(name)
        {
            *expression = value.clone();
        }
    });
    let super::Item::Statement(statement) = item else {
        unreachable!();
    };
    statement
}

fn general_prefix(
    body: &mut Block,
    prefix: &Block,
    values: &mut HashMap<String, (i128, i128, i128)>,
) -> Option<()> {
    let mut writes = HashSet::new();
    block_writes(prefix, &mut writes);
    if writes.iter().all(|name| !values.contains_key(name)) {
        return Some(());
    }
    if !normal_back_edges(body) {
        return None;
    }
    let mut updates = Vec::new();
    collect_general_updates(prefix, values, &mut updates)?;
    let mut directions = HashMap::new();
    for statement in &updates {
        let Stmt::Assign { target, .. } = statement.unlocated() else {
            return None;
        };
        let name = root_name(target)?;
        let delta = counter_delta(statement, name)?;
        if directions
            .insert(name, delta.signum())
            .is_some_and(|prior| prior != delta.signum())
        {
            return None;
        }
        let (start, min, max) = values.get_mut(name)?;
        *start = start.checked_add(delta)?;
        if !(*min..=*max).contains(start) {
            return None;
        }
    }
    body.statements.extend(updates);
    Some(())
}

fn collect_general_updates(
    prefix: &Block,
    values: &HashMap<String, (i128, i128, i128)>,
    updates: &mut Vec<Stmt>,
) -> Option<()> {
    for statement in &prefix.statements {
        match statement.unlocated() {
            Stmt::Block(body) => collect_general_updates(body, values, updates)?,
            Stmt::Assign { target, .. }
                if root_name(target).is_some_and(|name| values.contains_key(name)) =>
            {
                counter_delta(statement, root_name(target)?)?;
                updates.push(statement.clone());
            }
            Stmt::Assign { .. } | Stmt::Var(_) | Stmt::Assert(_) => {}
            Stmt::Expr(value) if !matches!(value.unlocated(), Expr::If { .. }) => {}
            _ => return None,
        }
    }
    Some(())
}

fn ranking_prefix(
    body: &mut Block,
    prefix: &Block,
    names: [&str; 2],
    ranges: &mut [(i128, i128, i128); 2],
) -> Option<()> {
    if !normal_back_edges(body) {
        return None;
    }
    let mut updates = Vec::new();
    collect_ranking_updates(prefix, names, &mut updates)?;
    // Check the first prefix in source order; the certificate includes the
    // appended next prefix, including the final false check but excluding breaks.
    for statement in &updates {
        for (name, (start, min, max)) in names.into_iter().zip(ranges.iter_mut()) {
            if let Some(delta) = counter_delta(statement, name) {
                *start = start.checked_add(delta)?;
                if !(*min..=*max).contains(start) {
                    return None;
                }
            }
        }
    }
    body.statements.extend(updates);
    Some(())
}

fn collect_ranking_updates(
    prefix: &Block,
    names: [&str; 2],
    updates: &mut Vec<Stmt>,
) -> Option<()> {
    for statement in &prefix.statements {
        match statement.unlocated() {
            Stmt::Block(body) => collect_ranking_updates(body, names, updates)?,
            Stmt::Assign { target, .. }
                if root_name(target).is_some_and(|name| names.contains(&name)) =>
            {
                let name = root_name(target)?;
                counter_delta(statement, name)?;
                updates.push(statement.clone());
            }
            Stmt::Assign { .. } | Stmt::Var(_) | Stmt::Assert(_) => {}
            Stmt::Expr(value) if !matches!(value.unlocated(), Expr::If { .. }) => {}
            _ => return None,
        }
    }
    Some(())
}

fn counter_delta(statement: &Stmt, counter: &str) -> Option<i128> {
    let Stmt::Assign { target, value } = statement.unlocated() else {
        return None;
    };
    if !matches!(target.unlocated(), Expr::Name(name) if name == counter) {
        return None;
    }
    let Expr::Binary { left, op, right } = value.unlocated() else {
        return None;
    };
    if !matches!(left.unlocated(), Expr::Name(name) if name == counter) {
        return None;
    }
    let literal = match right.unlocated() {
        Expr::Int(text) => crate::lexer::integer(text).ok().map(i128::from)?,
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => {
            let Expr::Int(text) = value.unlocated() else {
                return None;
            };
            i128::from(crate::lexer::integer(text).ok()?).checked_neg()?
        }
        _ => return None,
    };
    let delta = match op {
        BinaryOp::Add => literal,
        BinaryOp::Sub => literal.checked_neg()?,
        _ => return None,
    };
    (delta != 0).then_some(delta)
}

fn normal_back_edges(body: &Block) -> bool {
    body.statements
        .iter()
        .all(|statement| match statement.unlocated() {
            Stmt::Continue(_) | Stmt::While { .. } | Stmt::For { .. } => false,
            Stmt::Block(body) => normal_back_edges(body),
            Stmt::Expr(value) | Stmt::LabeledIf { value, .. } => {
                if let Expr::If { arms, .. } = value.unlocated() {
                    arms.iter().all(|(_, body)| normal_back_edges(body))
                } else {
                    true
                }
            }
            _ => true,
        })
}

fn root_name(value: &Expr) -> Option<&str> {
    match value.unlocated() {
        Expr::Name(name) => Some(name),
        Expr::Index { object, .. } | Expr::Member { object, .. } => root_name(object),
        _ => None,
    }
}

fn block_writes(block: &Block, names: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement.unlocated() {
            Stmt::Assign { target, .. } => {
                if let Some(name) = root_name(target) {
                    names.insert(name.to_owned());
                }
            }
            Stmt::Block(body) | Stmt::While { body, .. } | Stmt::For { body, .. } => {
                block_writes(body, names);
            }
            Stmt::Expr(Expr::If { arms, .. })
            | Stmt::LabeledIf {
                value: Expr::If { arms, .. },
                ..
            } => {
                for (_, body) in arms {
                    block_writes(body, names);
                }
            }
            _ => {}
        }
    }
}
