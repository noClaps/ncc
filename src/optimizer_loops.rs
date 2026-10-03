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
        proof.prepare(condition, body, env).is_some()
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
        env: &HashMap<String, Value>,
    ) -> Option<()> {
        let scope = self.scope(env);
        let (prefix, comparison) = if matches!(condition.unlocated(), Expr::Call { .. }) {
            self.return_call(condition, &scope)?
        } else {
            (
                Block { statements: vec![] },
                self.expression(condition, &scope)?,
            )
        };
        let mut body = self.block(body, &scope)?;
        if !self.writes.is_disjoint(&self.callable_names) {
            return None;
        }
        let Expr::Binary { left, right, .. } = comparison.unlocated() else {
            return None;
        };
        let Expr::Name(counter) = left.unlocated() else {
            return None;
        };
        let mut protected_bounds = HashSet::new();
        expression_names(right, &mut protected_bounds);
        let mut prefix_writes = HashSet::new();
        block_writes(&prefix, &mut prefix_writes);
        if !prefix_writes.is_disjoint(&protected_bounds) {
            return None;
        }
        let range = self.evaluator.loop_integer(self.values.get(counter)?)?;
        let (initial, min, max) = range;
        let start = if prefix_writes.contains(counter) {
            condition_counter_start(&mut body, &prefix, counter, range)?
        } else {
            initial
        };
        let certificate = crate::termination::counted_loop(&comparison, &body)?;
        let bound = self.bound_integer(certificate.bound)?;
        certificate.terminates(start, bound, min, max).then_some(())
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

    fn call_scope(&mut self, call: &Expr, caller: &Scope) -> Option<(&'module Function, Scope)> {
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
        let arguments = args
            .iter()
            .map(|arg| self.expression(arg, caller))
            .collect::<Option<Vec<_>>>()?;
        let (function, mut scope) = self.callable(callee, caller)?;
        if function.params.len() != arguments.len() {
            return None;
        }
        if !arguments.is_empty()
            && !matches!(function.body.statements.as_slice(), [statement]
            if matches!(statement.unlocated(), Stmt::Return(Some(_))))
        {
            return None;
        }
        for ((parameter, argument), original) in function.params.iter().zip(arguments).zip(args) {
            // Substitution models a scalar by-value argument only in a pure
            // return expression. No writable parameter is identified with a cell.
            if !matches!(&parameter.ty, Type::Named(name, types)
                if types.is_empty() && matches!(name.as_str(), "int" | "uint" | "byte" | "bool"))
                || self.evaluator.checked.expression_types.get(&original.id())
                    != Some(&parameter.ty)
            {
                return None;
            }
            scope.insert(
                parameter.name.clone(),
                Binding {
                    expression: argument,
                    value: None,
                },
            );
        }
        Some((function, scope))
    }

    fn return_call(&mut self, call: &Expr, caller: &Scope) -> Option<(Block, Expr)> {
        let (function, mut scope) = self.call_scope(call, caller)?;
        let identity = std::ptr::from_ref(function) as usize;

        if !self.active.insert(identity) {
            return None;
        }
        let (last, prefix) = function.body.statements.split_last()?;
        let Stmt::Return(Some(value)) = last.unlocated() else {
            return None;
        };
        // Substitution must not erase the evaluator's return coercion.
        if self.evaluator.expr_type(value)? != &function.return_type {
            return None;
        }
        // Declarations in the function's prefix shadow its initial environment
        // for the final return, including a returned transitive helper call.
        let prefix = self.statements_in_scope(prefix, &mut scope)?;
        let (nested, value) = if matches!(value.unlocated(), Expr::Call { .. }) {
            self.return_call(value, &scope)?
        } else {
            (
                Block { statements: vec![] },
                self.expression(value, &scope)?,
            )
        };
        let mut prefix = prefix;
        prefix.statements.extend(nested.statements);
        self.active.remove(&identity);
        Some((prefix, value))
    }

    fn void_call(&mut self, call: &Expr, caller: &Scope) -> Option<Block> {
        let (function, scope) = self.call_scope(call, caller)?;
        if function.return_type != Type::void() || !function.params.is_empty() {
            return None;
        }
        let identity = std::ptr::from_ref(function) as usize;
        if !self.active.insert(identity) {
            return None;
        }
        let statements = &function.body.statements;
        let end = statements.len()
            - usize::from(
                statements
                    .last()
                    .is_some_and(|s| matches!(s.unlocated(), Stmt::Return(None))),
            );
        let body = self.block_statements(&statements[..end], &scope)?;
        self.active.remove(&identity);
        Some(body)
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
        // Acyclic calls alone do not prove termination of loops inside helpers.
        if !self.active.is_empty()
            && matches!(statement.unlocated(), Stmt::While { .. } | Stmt::For { .. })
        {
            return None;
        }
        let statement = match statement.unlocated() {
            Stmt::Assign { target, value } => {
                let value = self.expression(value, scope)?;
                let target = self.expression(target, scope)?;
                let name = root_name(&target)?;
                if self
                    .values
                    .get(name)
                    .is_some_and(|value| !self.evaluator.recursion_targets(value).is_empty())
                {
                    return None;
                }
                self.writes.insert(name.to_owned());
                Stmt::Assign { target, value }
            }
            Stmt::Var(declaration) => {
                if declaration.mutex {
                    return None;
                }
                let value = self.expression(&declaration.value, scope)?;
                let mut declaration = declaration.clone();
                declaration.value = value;
                declaration.pattern = self.declaration_pattern(&declaration.pattern, scope)?;
                Stmt::Var(declaration)
            }
            Stmt::Block(body) => Stmt::Block(self.block(body, scope)?),
            Stmt::Expr(value) if matches!(value.unlocated(), Expr::Call { .. }) => {
                if let Expr::Call {
                    callee,
                    args,
                    generics,
                } = value.unlocated()
                    && matches!(callee.unlocated(), Expr::Name(name) if name == "@print" || name == "@println")
                {
                    Stmt::Expr(Expr::Call {
                        callee: callee.clone(),
                        generics: generics.clone(),
                        args: args
                            .iter()
                            .map(|arg| self.expression(arg, scope))
                            .collect::<Option<_>>()?,
                    })
                } else {
                    return Some(vec![Stmt::Block(self.void_call(value, scope)?)]);
                }
            }
            Stmt::Expr(value) => Stmt::Expr(self.control_expression(value, scope)?),
            Stmt::Assert(value) => Stmt::Assert(self.expression(value, scope)?),
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
            // Early helper returns cannot become exits from the caller's loop.
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

fn condition_counter_start(
    body: &mut Block,
    prefix: &Block,
    counter: &str,
    (start, min, max): (i128, i128, i128),
) -> Option<i128> {
    // In this slice, only normal fallthrough reaches the next condition. In
    // particular, a continue must not bypass the proof-only appended updates.
    if !normal_back_edges(body) {
        return None;
    }
    let mut updates = Vec::new();
    collect_condition_updates(prefix, counter, &mut updates)?;
    let mut delta: i128 = 0;
    for update in &updates {
        let step = counter_delta(update, counter)?;
        if delta != 0 && delta.signum() != step.signum() {
            return None;
        }
        delta = delta.checked_add(step)?;
    }
    if delta == 0 {
        return None;
    }
    // Sample the comparison after the first prefix. Same-direction updates
    // make its endpoints sufficient to bound all first-prefix intermediates.
    let first = start.checked_add(delta)?;
    if !(min..=max).contains(&first) {
        return None;
    }
    // From a sampled true comparison, the next sample is body then prefix.
    // Counted-loop excursions include the entire next prefix, even when its
    // comparison is false. Break paths never reach these appended statements.
    body.statements.extend(updates);
    Some(first)
}

fn collect_condition_updates(prefix: &Block, counter: &str, updates: &mut Vec<Stmt>) -> Option<()> {
    for statement in &prefix.statements {
        match statement.unlocated() {
            Stmt::Block(body) => collect_condition_updates(body, counter, updates)?,
            Stmt::Assign { target, .. } if root_name(target) == Some(counter) => {
                counter_delta(statement, counter)?;
                updates.push(statement.clone());
            }
            Stmt::Assign { .. } | Stmt::Var(_) | Stmt::Assert(_) => {}
            Stmt::Expr(value) if !matches!(value.unlocated(), Expr::If { .. }) => {}
            // Do not assume that a conditional write, jump, or nested loop
            // executes a fixed prefix on every check, including the last one.
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

fn expression_names(value: &Expr, names: &mut HashSet<String>) {
    crate::visit::item(
        &super::Item::Statement(Stmt::Expr(value.clone())),
        &mut |value| {
            if let Expr::Name(name) = value.unlocated() {
                names.insert(name.clone());
            }
        },
    );
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
