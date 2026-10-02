//! Non-executing helper expansion for counted-loop certificates.
//! These trees never reach the evaluator: source trees retain their IDs and locations.
use super::{
    Block, Evaluator, Expr, Function, HashMap, HashSet, Pattern, Stmt, Type, UnaryOp, Value,
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
        let body = self.block(body, &scope)?;
        if !self.writes.is_disjoint(&self.callable_names) {
            return None;
        }
        let certificate = crate::termination::counted_loop(&comparison, &body)?;
        // Condition effects also occur on the final, false check. They must not
        // change ranking state, even if a body path would exit before updating it.
        let mut protected = HashSet::from([certificate.counter.to_owned()]);
        expression_names(certificate.bound, &mut protected);
        let mut prefix_writes = HashSet::new();
        block_writes(&prefix, &mut prefix_writes);
        if !prefix_writes.is_disjoint(&protected) {
            return None;
        }
        let (start, min, max) = self
            .evaluator
            .loop_integer(self.values.get(certificate.counter)?)?;
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
