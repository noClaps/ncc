//! Conservative certificates for integer loops. Unknown is not infinite.
#[path = "termination_ranks.rs"]
mod ranks;
pub(crate) use ranks::varying_loop;
#[path = "termination_affine.rs"]
mod affine;
#[path = "termination_growth.rs"]
mod growth;
#[path = "termination_reset.rs"]
mod reset;
pub(crate) use affine::affine_loop;
pub(crate) use growth::growth_loop;
pub(crate) use reset::reset_loop;
#[path = "termination_condition.rs"]
mod condition;
pub(crate) use condition::condition_loop;
use std::collections::{BTreeMap, HashSet};

use crate::ast::{BinaryOp, Block, Expr, Stmt, UnaryOp};

pub(crate) struct CountedLoop<'a> {
    pub(crate) counter: &'a str,
    pub(crate) bound: &'a Expr,
    pub(crate) comparison: BinaryOp,
    pub(crate) step_min: i128,
    pub(crate) step_max: i128,
    // Include intermediate updates and paths which exit instead of looping.
    excursion: Range,
}

fn counted_comparison(condition: &Expr) -> Option<CountedLoop<'_>> {
    let Expr::Binary { left, op, right } = condition.unlocated() else {
        return None;
    };
    if !matches!(
        op,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Ne
    ) {
        return None;
    }
    let Expr::Name(counter) = left.unlocated() else {
        return None;
    };
    Some(CountedLoop {
        counter,
        bound: right,
        comparison: *op,
        step_min: 0,
        step_max: 0,
        excursion: Range::ZERO,
    })
}

pub(crate) fn counted_loop<'a>(condition: &'a Expr, body: &Block) -> Option<CountedLoop<'a>> {
    let mut certificate = counted_comparison(condition)?;
    let mut protected = HashSet::from([certificate.counter]);
    if !bound_names(certificate.bound, certificate.counter, &mut protected) {
        return None;
    }
    let mut analysis = Analysis {
        counter: certificate.counter,
        bound: None,
        protected,
        direction: 0,
        excursion: Range::ZERO,
    };
    let paths = analysis.block(body, Range::ZERO, true)?;
    let mut progress: Option<Range> = None;
    for (exit, range) in paths {
        // Without the enclosing loop's label, an unresolved continue may target
        // this loop. Requiring progress also for outer continues is conservative.
        if matches!(exit, Exit::Next | Exit::Continue(_)) {
            if range.min <= 0 && range.max >= 0 {
                return None;
            }
            progress = Some(progress.map_or(range, |prior| prior.union(range)));
        }
    }
    let progress = progress.unwrap_or(Range::ZERO);
    certificate.step_min = progress.min;
    certificate.step_max = progress.max;
    certificate.excursion = analysis.excursion;
    Some(certificate)
}

#[derive(Clone, Copy)]
struct Range {
    min: i128,
    max: i128,
}

impl Range {
    const ZERO: Self = Self { min: 0, max: 0 };

    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    fn add(self, delta: i128) -> Option<Self> {
        Some(Self {
            min: self.min.checked_add(delta)?,
            max: self.max.checked_add(delta)?,
        })
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Exit {
    Next,
    Break(Option<String>),
    Continue(Option<String>),
    Return,
}

trait Displacement: Copy {
    const ZERO: Self;

    fn union(self, other: Self) -> Self;
    fn shifted(self, binding: usize, delta: i128) -> Option<Self>;
}

impl Displacement for Range {
    const ZERO: Self = Self::ZERO;

    fn union(self, other: Self) -> Self {
        self.union(other)
    }

    fn shifted(self, _binding: usize, delta: i128) -> Option<Self> {
        self.add(delta)
    }
}

type Paths<S> = BTreeMap<Exit, S>;

fn merge<S: Displacement>(paths: &mut Paths<S>, exit: Exit, range: S) {
    paths
        .entry(exit)
        .and_modify(|prior| *prior = prior.union(range))
        .or_insert(range);
}

fn one<S>(exit: Exit, range: S) -> Paths<S> {
    BTreeMap::from([(exit, range)])
}

struct Analysis<'a, S> {
    counter: &'a str,
    bound: Option<&'a str>,
    protected: HashSet<&'a str>,
    direction: i128,
    excursion: S,
}

impl<S: Displacement> Analysis<'_, S> {
    fn block(&mut self, body: &Block, input: S, updates: bool) -> Option<Paths<S>> {
        let mut paths = one(Exit::Next, input);
        for statement in &body.statements {
            let Some(input) = paths.remove(&Exit::Next) else {
                break;
            };
            for (exit, range) in self.statement(statement, input, updates)? {
                merge(&mut paths, exit, range);
            }
        }
        Some(paths)
    }

    fn statement(&mut self, statement: &Stmt, input: S, updates: bool) -> Option<Paths<S>> {
        let change = update(statement, self.counter)
            .map(|delta| (0, delta))
            .or_else(|| update(statement, self.bound?).map(|delta| (1, delta)));
        if let Some((binding, delta)) = change {
            if !updates
                || (self.bound.is_none() && self.direction != 0 && self.direction != delta.signum())
            {
                return None;
            }
            self.direction = delta.signum();
            let range = input.shifted(binding, delta)?;
            self.excursion = self.excursion.union(range);
            return Some(one(Exit::Next, range));
        }
        match statement.unlocated() {
            Stmt::Assign { target, value } => {
                if root_name(target).is_none_or(|name| self.protected.contains(name))
                    || !pure_expression(target)
                {
                    return None;
                }
                self.expression(value, input, updates)
            }
            Stmt::Var(declaration) => {
                if declaration.mutex
                    || declaration
                        .binding_names()
                        .iter()
                        .any(|name| self.protected.contains(name))
                {
                    return None;
                }
                self.expression(&declaration.value, input, updates)
            }
            Stmt::Block(body) => self.block(body, input, updates),
            Stmt::Expr(value) | Stmt::Assert(value) => self.expression(value, input, updates),
            Stmt::LabeledIf { label, value } => {
                let mut paths = self.expression(value, input, updates)?;
                if let Some(range) = paths.remove(&Exit::Break(Some(label.clone()))) {
                    merge(&mut paths, Exit::Next, range);
                }
                Some(paths)
            }
            Stmt::Return(value) => {
                let mut paths = match value {
                    Some(value) => self.expression(value, input, updates)?,
                    None => one(Exit::Next, input),
                };
                if let Some(range) = paths.remove(&Exit::Next) {
                    merge(&mut paths, Exit::Return, range);
                }
                Some(paths)
            }
            Stmt::Throw(value) => pure_expression(value).then(|| one(Exit::Return, input)),
            // Value breaks depend on type metadata identifying the value scope.
            Stmt::Break(Some(_), _) | Stmt::Lock { .. } => None,
            Stmt::Break(None, label) => Some(one(Exit::Break(label.clone()), input)),
            Stmt::Continue(label) => Some(one(Exit::Continue(label.clone()), input)),
            Stmt::While {
                label,
                condition,
                body,
            } => self.nested_loop(label.as_deref(), condition, body, input),
            Stmt::For {
                label,
                name,
                iterable,
                body,
            } => {
                if self.protected.contains(name.as_str()) {
                    return None;
                }
                self.nested_loop(label.as_deref(), iterable, body, input)
            }
            Stmt::Located(_, _) => unreachable!(),
        }
    }

    fn expression(&mut self, expression: &Expr, input: S, updates: bool) -> Option<Paths<S>> {
        match expression.unlocated() {
            Expr::If { subject, arms } => {
                if !subject.as_deref().is_none_or(pure_expression) || arms.is_empty() {
                    return None;
                }
                let mut paths = Paths::<S>::new();
                // Checked NC conditionals are exhaustive. Pattern evaluation may
                // not hide writes or jumps; all possible arms are retained.
                for (patterns, body) in arms {
                    let mut safe = true;
                    for pattern in patterns {
                        crate::visit::pattern(pattern, &mut |value| {
                            safe &= leaf_expression(value);
                        });
                    }
                    if !safe {
                        return None;
                    }
                    for (exit, range) in self.block(body, input, updates)? {
                        merge(&mut paths, exit, range);
                    }
                }
                Some(paths)
            }
            Expr::Else { value, fallback } => self.fallback(value, fallback, input, updates),
            Expr::Catch { value, name, body } => {
                if self.protected.contains(name.as_str()) {
                    return None;
                }
                self.fallback(value, body, input, updates)
            }
            _ => pure_expression(expression).then(|| one(Exit::Next, input)),
        }
    }

    fn fallback(
        &mut self,
        value: &Expr,
        body: &Block,
        input: S,
        updates: bool,
    ) -> Option<Paths<S>> {
        if !pure_expression(value) {
            return None;
        }
        let mut paths = self.block(body, input, updates)?;
        merge(&mut paths, Exit::Next, input);
        Some(paths)
    }

    fn nested_loop(
        &mut self,
        label: Option<&str>,
        condition: &Expr,
        body: &Block,
        input: S,
    ) -> Option<Paths<S>> {
        // The new relational path cannot assume a nested-loop certificate will
        // be available later, after entering the enclosing iteration.
        if self.bound.is_some() || !pure_expression(condition) {
            return None;
        }
        // The caller must discharge nested termination before execution.
        // This analysis only protects ranking state, including exiting paths.
        let nested = self.block(body, input, false)?;
        let mut paths = one(Exit::Next, input);
        for (exit, range) in nested {
            match &exit {
                Exit::Next => {}
                Exit::Break(target) | Exit::Continue(target)
                    if target.is_none() || target.as_deref() == label => {}
                _ => merge(&mut paths, exit, range),
            }
        }
        Some(paths)
    }
}

fn integer(expression: &Expr) -> Option<i128> {
    match expression.unlocated() {
        Expr::Int(text) => crate::lexer::integer(text).ok().map(i128::from),
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => integer(value)?.checked_neg(),
        _ => None,
    }
}

fn update(statement: &Stmt, counter: &str) -> Option<i128> {
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
    let step = match op {
        BinaryOp::Add => integer(right)?,
        BinaryOp::Sub => integer(right)?.checked_neg()?,
        _ => return None,
    };
    (step != 0).then_some(step)
}

fn bound_names<'a>(expression: &'a Expr, counter: &str, names: &mut HashSet<&'a str>) -> bool {
    match expression.unlocated() {
        Expr::Name(name) => {
            name != counter && {
                names.insert(name);
                true
            }
        }
        Expr::Int(_) => true,
        Expr::Unary { value, .. } | Expr::Member { object: value, .. } => {
            bound_names(value, counter, names)
        }
        Expr::Binary { left, right, .. } => {
            bound_names(left, counter, names) && bound_names(right, counter, names)
        }
        _ => false,
    }
}

fn pure_expression(expression: &Expr) -> bool {
    let mut safe = true;
    let item = crate::ast::Item::Statement(Stmt::Expr(expression.clone()));
    crate::visit::item(&item, &mut |value| safe &= leaf_expression(value));
    safe
}

fn leaf_expression(expression: &Expr) -> bool {
    match expression.unlocated() {
        Expr::Call { callee, .. } => {
            matches!(callee.unlocated(), Expr::Name(name) if name.starts_with('@'))
        }
        // Embedded control flow and callable creation can hide writes or jumps.
        Expr::Lambda(_)
        | Expr::Async(_)
        | Expr::Await(_)
        | Expr::If { .. }
        | Expr::Else { .. }
        | Expr::Catch { .. } => false,
        _ => true,
    }
}

fn root_name(expression: &Expr) -> Option<&str> {
    match expression.unlocated() {
        Expr::Name(name) => Some(name),
        Expr::Index { object, .. } | Expr::Member { object, .. } => root_name(object),
        _ => None,
    }
}

impl CountedLoop<'_> {
    pub(crate) fn terminates(&self, start: i128, bound: i128, min: i128, max: i128) -> bool {
        use BinaryOp::{Ge, Gt, Le, Lt, Ne};
        let active = match self.comparison {
            Lt => start < bound,
            Le => start <= bound,
            Gt => start > bound,
            Ge => start >= bound,
            Ne => start != bound,
            _ => return false,
        };
        if !active {
            return true;
        }
        if !(min..=max).contains(&start) {
            return false;
        }
        if self.step_min == 0 && self.step_max == 0 {
            return self.in_range(start, min, max);
        }
        let increasing = self.step_min > 0;
        if !increasing && self.step_max >= 0 {
            return false;
        }
        let last = if self.comparison == Ne || self.step_min == self.step_max {
            self.fixed_last(start, bound)
        } else {
            // Before the last iteration, any active integer is at most bound-1
            // (or bound for <=). Include the largest possible intermediate update.
            match self.comparison {
                Lt if increasing => bound.checked_sub(1).map(|last| last.max(start)),
                Le if increasing => Some(bound.max(start)),
                Gt if !increasing => bound.checked_add(1).map(|last| last.min(start)),
                Ge if !increasing => Some(bound.min(start)),
                _ => None,
            }
        };
        last.is_some_and(|last| self.in_range(start, min, max) && self.in_range(last, min, max))
    }

    fn in_range(&self, value: i128, min: i128, max: i128) -> bool {
        value
            .checked_add(self.excursion.min)
            .is_some_and(|low| low >= min)
            && value
                .checked_add(self.excursion.max)
                .is_some_and(|high| high <= max)
    }

    fn fixed_last(&self, start: i128, bound: i128) -> Option<i128> {
        if self.step_min != self.step_max {
            return None;
        }
        let endpoint = match self.comparison {
            BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Ne => bound,
            BinaryOp::Le => bound.checked_add(1)?,
            BinaryOp::Ge => bound.checked_sub(1)?,
            _ => return None,
        };
        let distance = endpoint.checked_sub(start)?;
        let step = self.step_min;
        if distance.signum() != step.signum() {
            return None;
        }
        if self.comparison == BinaryOp::Ne && distance.checked_rem(step)? != 0 {
            return None;
        }
        let distance = distance.checked_abs()?;
        let stride = step.checked_abs()?;
        let iterations = (distance / stride).checked_add(i128::from(distance % stride != 0))?;
        start.checked_add(iterations.checked_sub(1)?.checked_mul(step)?)
    }
}
