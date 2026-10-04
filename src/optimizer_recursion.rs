//! Structural recursion certificates and callable-aware heap continuations.
use super::{BinaryOp, Block, Evaluator, Expr, Function, Pattern, Stmt, Type, Value};
use std::collections::{HashMap, HashSet};

struct Rank<'a> {
    index: usize,
    coverage: u64,
    bases: Vec<&'a Expr>,
    recursive: &'a Expr,
}

type Targets = HashMap<usize, HashSet<usize>>;
type Scopes = HashMap<usize, HashMap<String, HashSet<usize>>>;
type Arms = [(Vec<Pattern>, Block)];

struct Conditional<'a> {
    subject: &'a Expr,
    arms: &'a Arms,
    fallthrough: Option<&'a Expr>,
}

impl Evaluator<'_> {
    /// A direct-self certificate reads only parameters and has no transitive
    /// callable environment. Other certified graphs may read cells not present
    /// in the named entry's memo key, even when that entry itself has no captures.
    pub(super) fn recursion_heap_cacheable(
        &self,
        function: &Function,
        callable: &Value,
        arguments: &[Value],
    ) -> bool {
        matches!(callable, Value::Function(name) if *name == function.name)
            && !arguments.iter().any(Value::contains_cell)
            && self
                .recursion_expressions(function)
                .iter()
                .all(|expression| {
                    let Expr::Call { callee, .. } = expression else {
                        return true;
                    };
                    matches!(callee.unlocated(), Expr::Name(name) if *name == function.name)
                        && !self.checked.constant_sources.contains_key(&callee.id())
                })
    }

    /// The callable-flow map is keyed by spelling, not lexical binding identity.
    /// Local binders cannot safely supply targets for reads of another binding,
    /// including reads before a later declaration in the same block. Keep those
    /// reads unknown until the graph carries binding provenance.
    pub(super) fn recursion_scoped_callable_barriers(
        &self,
        function: &Function,
        scope: &mut HashMap<String, HashSet<usize>>,
    ) -> bool {
        let mut binders = HashMap::<String, HashSet<usize>>::new();
        for statement in Self::recursion_statements(function) {
            match statement {
                Stmt::Var(declaration) => {
                    for name in declaration.binding_names() {
                        binders
                            .entry(name.to_owned())
                            .or_default()
                            .insert(declaration.value.id());
                    }
                }
                Stmt::For { name, .. } => {
                    binders.entry(name.clone()).or_default().insert(usize::MAX);
                }
                _ => {}
            }
        }
        let expressions = self.recursion_expressions(function);
        for expression in &expressions {
            if let Expr::If { arms, .. } = expression {
                for (patterns, _) in arms {
                    for pattern in patterns {
                        for name in Self::recursion_pattern_names(pattern) {
                            binders
                                .entry(name.to_owned())
                                .or_default()
                                .insert(usize::MAX);
                        }
                    }
                }
            }
        }
        let mutable_locals = self.recursion_unambiguous_mutable_locals(function, &binders);
        let mut changed = false;
        for expression in expressions {
            changed |= self.recursion_shadow_barrier(expression, &binders, &mutable_locals, scope);
            if let Expr::Lambda(child) = expression {
                // Deferred bodies can read an outer binding after the inner
                // declaration's scope has ended, too. Do not inherit its target.
                crate::visit::block(&child.body, &mut |read| {
                    changed |=
                        self.recursion_shadow_barrier(read, &binders, &HashSet::new(), scope);
                });
            }
        }
        changed
    }

    /// Mutable reads have no constant-source key. A unique direct local binder
    /// is still identifiable when parameters/captures cannot supply an earlier
    /// binding and the read is not a shared global access. Deferred bodies are
    /// excluded from this exception because they have their own lexical scopes.
    fn recursion_unambiguous_mutable_locals<'a>(
        &self,
        function: &'a Function,
        binders: &HashMap<String, HashSet<usize>>,
    ) -> HashSet<&'a str> {
        let captures = self.lambdas.iter().find_map(|(key, lambda)| {
            std::ptr::eq(*lambda, function)
                .then(|| self.checked.captures.get(key))
                .flatten()
        });
        let mut names = HashSet::new();
        for statement in &function.body.statements {
            let Stmt::Var(declaration) = statement.unlocated() else {
                continue;
            };
            if !declaration.mutable {
                continue;
            }
            for name in declaration.binding_names() {
                if binders
                    .get(name)
                    .is_some_and(|keys| keys.len() == 1 && keys.contains(&declaration.value.id()))
                    && !function.params.iter().any(|param| param.name == name)
                    && !captures
                        .is_some_and(|captures| captures.iter().any(|capture| capture.name == name))
                {
                    names.insert(name);
                }
            }
        }
        names
    }

    fn recursion_shadow_barrier(
        &self,
        expression: &Expr,
        binders: &HashMap<String, HashSet<usize>>,
        mutable_locals: &HashSet<&str>,
        scope: &mut HashMap<String, HashSet<usize>>,
    ) -> bool {
        let Expr::Name(name) = expression.unlocated() else {
            return false;
        };
        let Some(initializers) = binders.get(name) else {
            return false;
        };
        if !self.recursion_callable_type(expression) {
            return false;
        }
        if matches!(self.checked.constant_sources.get(&expression.id()), Some(Some(key)) if initializers.contains(key))
        {
            return false;
        }
        if matches!(
            self.checked.constant_sources.get(&expression.id()),
            Some(None)
        ) && mutable_locals.contains(name.as_str())
            && !self.checked.shared_accesses.contains(&expression.id())
        {
            return false;
        }
        Self::recursion_merge(scope, name, &HashSet::from([usize::MAX]))
    }

    /// Every cycle must contain strict descent. Neutral forwarding is permitted
    /// only in an acyclic subgraph, so it cannot postpone descent indefinitely.
    pub(super) fn recursion_certify_graph(
        &self,
        functions: &HashMap<usize, &Function>,
        scopes: &Scopes,
        returns: &Targets,
        graph: &Targets,
        active: &HashSet<usize>,
    ) -> Option<HashMap<usize, usize>> {
        let mut pending = Vec::new();
        for id in active {
            let scope = scopes.get(id).cloned().unwrap_or_default();
            if !self.recursion_calls_known(functions[id], &scope, returns) {
                return None;
            }
            if graph
                .get(id)
                .into_iter()
                .flatten()
                .any(|target| Self::recursion_reaches(graph, *target, *id))
            {
                pending.push(*id);
            }
        }
        let mut ranks = HashMap::new();
        let mut neutral = Targets::new();
        while let Some(id) = pending.pop() {
            if ranks.contains_key(&id) {
                continue;
            }
            let function = functions[&id];
            let rank = Self::recursion_shape(function)?;
            let scope = scopes.get(&id).cloned().unwrap_or_default();
            if !rank
                .bases
                .iter()
                .all(|value| Self::recursion_pure(value, function))
                || !self.recursion_ranked_expression(
                    rank.recursive,
                    function,
                    &rank,
                    &scope,
                    functions,
                    &mut neutral,
                )
            {
                return None;
            }
            ranks.insert(id, rank.index);
            pending.extend(graph.get(&id).into_iter().flatten());
        }
        if neutral.iter().any(|(id, targets)| {
            targets
                .iter()
                .any(|target| Self::recursion_reaches(&neutral, *target, *id))
        }) {
            return None;
        }
        Some(ranks)
    }

    fn recursion_conditional(function: &Function) -> Option<Conditional<'_>> {
        let (statement, fallthrough) = match function.body.statements.as_slice() {
            [statement] => (statement, None),
            [statement, last] => {
                let Stmt::Return(Some(value)) = last.unlocated() else {
                    return None;
                };
                (statement, Some(value))
            }
            _ => return None,
        };
        let Stmt::Expr(expression) = statement.unlocated() else {
            return None;
        };
        let Expr::If {
            subject: Some(subject),
            arms,
        } = expression.unlocated()
        else {
            return None;
        };
        Some(Conditional {
            subject,
            arms,
            fallthrough,
        })
    }

    fn recursion_shape(function: &Function) -> Option<Rank<'_>> {
        let Some(conditional) = Self::recursion_conditional(function) else {
            return Self::recursion_forwarding_shape(function);
        };
        let arms = conditional.arms;
        if let Expr::Name(name) = conditional.subject.unlocated() {
            let index = Self::recursion_parameter(function, name)?;
            let (fallback, base_arms) = arms.split_last()?;
            if !matches!(fallback.0.as_slice(), [Pattern::Wildcard]) {
                return None;
            }
            let mut literals = HashSet::new();
            let mut bases = Vec::new();
            for (patterns, body) in base_arms {
                bases.push(Self::recursion_return(body)?);
                for pattern in patterns {
                    let Pattern::Literal(value) = pattern else {
                        return None;
                    };
                    literals.insert(Self::recursion_rank_literal(function, index, value)?);
                }
            }
            let coverage = u64::try_from(literals.len()).ok()?;
            if coverage == 0 || !(0..coverage).all(|value| literals.contains(&value)) {
                return None;
            }
            return Some(Rank {
                index,
                coverage,
                bases,
                recursive: Self::recursion_branch(&fallback.1, conditional.fallthrough)?,
            });
        }
        Self::recursion_comparison_shape(function, &conditional)
    }

    /// A guard-free forwarding frame has no guaranteed positive rank: only
    /// unchanged-rank edges are safe here, including entry at zero.
    fn recursion_forwarding_shape(function: &Function) -> Option<Rank<'_>> {
        let recursive = Self::recursion_return(&function.body)?;
        if !matches!(recursive.unlocated(), Expr::Call { .. }) {
            return None;
        }
        let mut candidates = function
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                Self::recursion_parameter(function, &param.name).map(|_| index)
            });
        let index = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(Rank {
            index,
            coverage: 0,
            bases: Vec::new(),
            recursive,
        })
    }

    fn recursion_parameter(function: &Function, name: &str) -> Option<usize> {
        function.params.iter().position(|param| {
            param.name == name
                && matches!(&param.ty, Type::Named(ty, args) if args.is_empty()
                && matches!(ty.as_str(), "int" | "uint" | "byte" | "float"))
        })
    }

    fn recursion_comparison_shape<'a>(
        function: &'a Function,
        conditional: &Conditional<'a>,
    ) -> Option<Rank<'a>> {
        let Expr::Binary { left, op, right } = conditional.subject.unlocated() else {
            return None;
        };
        let (name, literal, op) = match (left.unlocated(), right.unlocated()) {
            (Expr::Name(name), _) => (name, &**right, *op),
            (_, Expr::Name(name)) => (
                name,
                &**left,
                match op {
                    BinaryOp::Lt => BinaryOp::Gt,
                    BinaryOp::Le => BinaryOp::Ge,
                    BinaryOp::Gt => BinaryOp::Lt,
                    BinaryOp::Ge => BinaryOp::Le,
                    other => *other,
                },
            ),
            _ => return None,
        };
        let index = Self::recursion_parameter(function, name)?;
        let limit = Self::recursion_rank_literal(function, index, literal)?;
        let [(patterns, first), (fallback, last)] = conditional.arms else {
            return None;
        };
        let [Pattern::Literal(boolean)] = patterns.as_slice() else {
            return None;
        };
        let Expr::Bool(first_when) = boolean.unlocated() else {
            return None;
        };
        if !matches!(fallback.as_slice(), [Pattern::Wildcard])
            && !matches!(fallback.as_slice(), [Pattern::Literal(value)] if matches!(value.unlocated(), Expr::Bool(value) if value != first_when))
        {
            return None;
        }
        let (coverage, base_when) = match op {
            BinaryOp::Lt => (limit, true),
            BinaryOp::Le => (limit.checked_add(1)?, true),
            BinaryOp::Gt => (limit.checked_add(1)?, false),
            BinaryOp::Ge => (limit, false),
            BinaryOp::Eq if limit == 0 => (1, true),
            BinaryOp::Ne if limit == 0 => (1, false),
            _ => return None,
        };
        if coverage == 0 {
            return None;
        }
        let (base, recursive) = if *first_when == base_when {
            (first, last)
        } else {
            (last, first)
        };
        Some(Rank {
            index,
            coverage,
            bases: vec![Self::recursion_branch(base, conditional.fallthrough)?],
            recursive: Self::recursion_branch(recursive, conditional.fallthrough)?,
        })
    }

    fn recursion_branch<'a>(body: &'a Block, fallthrough: Option<&'a Expr>) -> Option<&'a Expr> {
        if body.statements.is_empty() {
            fallthrough
        } else {
            Self::recursion_return(body)
        }
    }

    fn recursion_pure(expression: &Expr, function: &Function) -> bool {
        match expression.unlocated() {
            Expr::Name(name) => function.params.iter().any(|param| param.name == *name),
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) | Expr::Char(_) => true,
            Expr::Unary { value, .. } => Self::recursion_pure(value, function),
            Expr::Binary { left, right, .. } => {
                Self::recursion_pure(left, function) && Self::recursion_pure(right, function)
            }
            _ => false,
        }
    }

    fn recursion_ranked_expression(
        &self,
        expression: &Expr,
        function: &Function,
        rank: &Rank<'_>,
        scope: &HashMap<String, HashSet<usize>>,
        functions: &HashMap<usize, &Function>,
        neutral: &mut Targets,
    ) -> bool {
        match expression.unlocated() {
            Expr::Unary { value, .. } => {
                self.recursion_ranked_expression(value, function, rank, scope, functions, neutral)
            }
            Expr::Binary { left, right, .. } => {
                self.recursion_ranked_expression(left, function, rank, scope, functions, neutral)
                    && self.recursion_ranked_expression(
                        right, function, rank, scope, functions, neutral,
                    )
            }
            Expr::Call { callee, args, .. } => {
                // Callee evaluation must itself be a read, never a factory or an
                // indexed expression with effects. All alternatives are checked.
                if !matches!(callee.unlocated(), Expr::Name(_)) {
                    return false;
                }
                let targets =
                    self.recursion_abstract(callee, scope, &HashMap::new(), &mut HashSet::new());
                !targets.is_empty()
                    && targets.iter().all(|target| {
                        let Some(child) = functions.get(target) else {
                            return false;
                        };
                        let Some(child_rank) = Self::recursion_shape(child) else {
                            return false;
                        };
                        if args.len() != child.params.len()
                            || child.params[child_rank.index].ty != function.params[rank.index].ty
                        {
                            return false;
                        }
                        if !args.iter().all(|arg| Self::recursion_pure(arg, function)) {
                            return false;
                        }
                        match Self::recursion_descent(&args[child_rank.index], function, rank) {
                            Some(true) => true,
                            Some(false) => {
                                neutral
                                    .entry(Self::recursion_identity(function))
                                    .or_default()
                                    .insert(*target);
                                true
                            }
                            None => false,
                        }
                    })
            }
            _ => Self::recursion_pure(expression, function),
        }
    }

    fn recursion_descent(expression: &Expr, function: &Function, rank: &Rank<'_>) -> Option<bool> {
        let name = &function.params[rank.index].name;
        if matches!(expression.unlocated(), Expr::Name(value) if value == name) {
            return Some(false);
        }
        let Expr::Binary { left, op, right } = expression.unlocated() else {
            return None;
        };
        if !matches!(left.unlocated(), Expr::Name(value) if value == name) {
            return None;
        }
        let step = Self::recursion_rank_literal(function, rank.index, right)?;
        match op {
            BinaryOp::Sub if step > 0 && step <= rank.coverage => Some(true),
            BinaryOp::Div
                if rank.coverage > 0
                    && step > 1
                    && !matches!(&function.params[rank.index].ty, Type::Named(name, _) if name == "float") =>
            {
                Some(true)
            }
            _ => None,
        }
    }

    pub(super) fn recursion_selected_body<'a>(
        &mut self,
        function: &'a Function,
        scope: &mut HashMap<String, Value>,
        rank: usize,
    ) -> Option<&'a Expr> {
        if !Self::recursion_domain(scope.get(&function.params[rank].name)?) {
            return None;
        }
        let Some(conditional) = Self::recursion_conditional(function) else {
            return Self::recursion_return(&function.body);
        };
        let subject = self.evaluate(conditional.subject, scope)?;
        for (patterns, body) in conditional.arms {
            for pattern in patterns {
                let matched = match pattern {
                    Pattern::Wildcard => true,
                    Pattern::Literal(literal) => subject.equals(&self.evaluate(literal, scope)?),
                    _ => return None,
                };
                if matched {
                    return Self::recursion_branch(body, conditional.fallthrough);
                }
            }
        }
        None
    }
}
