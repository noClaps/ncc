//! Discharge repeated-entry obligations without interpreting enclosing iterations.
use super::{Block, Evaluator, Expr, HashMap, HashSet, Proof, Scope, Stmt, Value, helpers};

impl Evaluator<'_> {
    pub(in crate::optimizer) fn nested_loops_proven(
        &self,
        body: &Block,
        iterator: Option<(&str, &Expr)>,
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
        proof.discharge(body, iterator, env).is_some()
    }
}

impl Proof<'_, '_> {
    fn discharge(
        &mut self,
        body: &Block,
        iterator: Option<(&str, &Expr)>,
        env: &HashMap<String, Value>,
    ) -> Option<()> {
        let mut scope = self.scope(env);
        // Finite traversals and loop-free calls have no termination obligation.
        // Do not restrict their ordinary value/jump grammar to the proof grammar.
        let mut inner = scope.clone();
        if let Some((name, _)) = iterator {
            self.local(name, &mut inner);
        }
        let body_obligations = self.obligations(body, &inner);
        let iterable_obligations = iterator.is_some_and(|(_, value)| {
            let block = Block {
                statements: vec![Stmt::Expr(value.clone())],
            };
            self.obligations(&block, &scope)
        });
        if !body_obligations && !iterable_obligations {
            return Some(());
        }
        let prefix = if let Some((name, iterable)) = iterator {
            let (prefix, _) = self.effect_expression(iterable, &scope)?;
            // Every key/index is a new iteration binding, never an outer value
            // with the same spelling or the first iteration's concrete value.
            self.local(name, &mut scope);
            prefix
        } else {
            Block { statements: vec![] }
        };
        let body = helpers::consume_results(&self.block(body, &scope)?);
        if !self.writes.is_disjoint(&self.callable_names) {
            return None;
        }
        self.certify_helper_loops(&prefix)?;
        self.certify_helper_loops(&body)?;
        Some(())
    }

    fn obligations(&mut self, body: &Block, scope: &Scope) -> bool {
        if loop_syntax(body) {
            return true;
        }
        let mut calls = Vec::new();
        crate::visit::block(body, &mut |value| {
            if let Expr::Call { callee, .. } = value.unlocated()
                && !matches!(callee.unlocated(), Expr::Name(name) if name.starts_with('@'))
            {
                calls.push(callee);
            }
        });
        calls.into_iter().any(|callee| {
            matches!(callee.unlocated(), Expr::Name(name) if shadowed(body, name))
                || self.call_obligations(callee, scope)
        })
    }

    fn call_obligations(&mut self, callee: &Expr, scope: &Scope) -> bool {
        // Writable callables may be replaced before a later call. Scanning the
        // current value alone cannot establish absence of hidden loop execution.
        if let Expr::Name(name) = callee.unlocated()
            && scope
                .get(name)
                .is_some_and(|binding| matches!(binding.value, Some(Value::Cell(_))))
        {
            return true;
        }
        let Some((function, scope)) = self.callable(callee, scope) else {
            return true;
        };
        let identity = std::ptr::from_ref(function) as usize;
        if !self.active.insert(identity) {
            return true;
        }
        let mut scope = scope;
        for parameter in &function.params {
            self.local(&parameter.name, &mut scope);
        }
        let obligations = self.obligations(&function.body, &scope);
        self.active.remove(&identity);
        obligations
    }
}

fn loop_syntax(body: &Block) -> bool {
    if body
        .statements
        .iter()
        .any(|statement| match statement.unlocated() {
            Stmt::While { .. } => true,
            Stmt::For { body, .. } | Stmt::Block(body) | Stmt::Lock { body, .. } => {
                loop_syntax(body)
            }
            _ => false,
        })
    {
        return true;
    }
    let mut found = false;
    crate::visit::block(body, &mut |value| {
        found |= match value.unlocated() {
            Expr::If { arms, .. } => arms.iter().any(|(_, body)| loop_syntax(body)),
            Expr::Else { fallback, .. } => loop_syntax(fallback),
            Expr::Catch { body, .. } => loop_syntax(body),
            _ => false,
        };
    });
    found
}

fn shadowed(body: &Block, name: &str) -> bool {
    if body
        .statements
        .iter()
        .any(|statement| match statement.unlocated() {
            Stmt::Var(declaration) => declaration.binding_names().contains(&name),
            Stmt::For {
                name: iterator,
                body,
                ..
            } => iterator == name || shadowed(body, name),
            Stmt::Block(body) | Stmt::Lock { body, .. } => shadowed(body, name),
            _ => false,
        })
    {
        return true;
    }
    let mut found = false;
    crate::visit::block(body, &mut |value| {
        found |= match value.unlocated() {
            Expr::If { arms, .. } => arms.iter().any(|(patterns, body)| {
                patterns.iter().any(|pattern| pattern_name(pattern, name)) || shadowed(body, name)
            }),
            Expr::Else { fallback, .. } => shadowed(fallback, name),
            Expr::Catch {
                name: binding,
                body,
                ..
            } => binding == name || shadowed(body, name),
            Expr::Lambda(function) => {
                function
                    .params
                    .iter()
                    .any(|parameter| parameter.name == name)
                    || shadowed(&function.body, name)
            }
            _ => false,
        };
    });
    found
}

fn pattern_name(pattern: &super::Pattern, name: &str) -> bool {
    use super::Pattern;
    match pattern {
        Pattern::Name(binding) => binding == name,
        Pattern::Array(patterns)
        | Pattern::Tuple(patterns)
        | Pattern::Variant {
            values: patterns, ..
        } => patterns.iter().any(|pattern| pattern_name(pattern, name)),
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|(_, pattern)| pattern_name(pattern, name)),
        _ => false,
    }
}
