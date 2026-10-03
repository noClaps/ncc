//! Symbolic affine integer loop certificates, without speculative execution.
//!
//! The caller supplies checked, canonical binding names and integer domains. No
//! condition-prefix effects are supported. Nested-loop obligations are rejected.
//! Each arithmetic expression must have a single inferable integer domain;
//! independent literal-only arithmetic defaults to `int` (or `uint` for
//! suffixed literals), rather than inheriting an enclosing operation's domain.
//! Local declarations cannot shadow a supplied canonical identity. Untracked
//! names may be opaque reads. Ranking arithmetic and assignments remain strict;
//! pure non-affine reads are left to certified execution to evaluate and diagnose.
//! Only `@print` and `@println` calls are accepted, without
//! executing them, and their arguments must themselves be supported pure reads.
use std::collections::HashMap;

use crate::ast::{BinaryOp, Block, Expr, Pattern, Stmt, UnaryOp};

#[derive(Clone, Copy)]
enum Domain {
    Literal,
    Known(i128, i128),
}

impl Domain {
    fn bounds(self) -> (i128, i128) {
        match self {
            Self::Literal => (i128::from(i64::MIN), i128::from(i64::MAX)),
            Self::Known(min, max) => (min, max),
        }
    }
}

#[derive(Clone)]
struct Form {
    constant: i128,
    coefficients: Vec<i128>,
}

impl Form {
    fn constant(value: i128, count: usize) -> Self {
        Self {
            constant: value,
            coefficients: vec![0; count],
        }
    }

    fn scaled(mut self, factor: i128) -> Option<Self> {
        self.constant = self.constant.checked_mul(factor)?;
        for coefficient in &mut self.coefficients {
            *coefficient = coefficient.checked_mul(factor)?;
        }
        Some(self)
    }

    fn added(mut self, other: &Self) -> Option<Self> {
        self.constant = self.constant.checked_add(other.constant)?;
        for (coefficient, other) in self.coefficients.iter_mut().zip(&other.coefficients) {
            *coefficient = coefficient.checked_add(*other)?;
        }
        Some(self)
    }

    fn displacement(&self, state: &[i128]) -> Option<i128> {
        self.coefficients
            .iter()
            .zip(state)
            .try_fold(0_i128, |sum, (coefficient, value)| {
                sum.checked_add(coefficient.checked_mul(*value)?)
            })
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Exit {
    Next,
    Continue,
    Break(Option<String>),
    Return,
}

#[derive(Clone)]
struct Path {
    exit: Exit,
    state: Vec<i128>,
}

struct Probe {
    form: Form,
    state: Vec<i128>,
    domain: (i128, i128),
}

struct Analysis<'a> {
    names: Vec<&'a str>,
    values: Vec<(i128, i128, i128)>,
    written: Vec<bool>,
    probes: Vec<Probe>,
}

/// Tuples contain (initial, minimum representable, maximum representable).
/// The input must be semantically checked; all names denote canonical storage
/// identities. Nonwritten scalar names may also be normalized to literals by the
/// caller. Unknown is not infinite: `false` means no certificate was established.
pub(crate) fn affine_loop(
    condition: &Expr,
    body: &Block,
    values: &HashMap<String, (i128, i128, i128)>,
) -> bool {
    certificate(condition, body, values).is_some()
}

fn certificate(
    condition: &Expr,
    body: &Block,
    values: &HashMap<String, (i128, i128, i128)>,
) -> Option<()> {
    let Expr::Binary { left, op, right } = condition.unlocated() else {
        return None;
    };
    if !matches!(
        op,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Ne
    ) {
        return None;
    }
    let mut entries: Vec<_> = values.iter().collect();
    entries.sort_by_key(|(name, _)| *name);
    let mut analysis = Analysis {
        names: entries.iter().map(|(name, _)| name.as_str()).collect(),
        values: entries.iter().map(|(_, value)| **value).collect(),
        written: vec![false; entries.len()],
        probes: Vec::new(),
    };
    if analysis
        .values
        .iter()
        .any(|&(initial, min, max)| !(min..=max).contains(&initial))
    {
        return None;
    }
    analysis.find_writes(body)?;
    let zero = vec![0; entries.len()];
    let domain = analysis.domain_pair(left, right)?;
    let left = analysis.form(left, domain, &zero)?;
    let right = analysis.form(right, domain, &zero)?;
    let rank = right.added(&left.scaled(-1)?)?;
    let gap = analysis.initial(&rank)?;
    let active = match op {
        BinaryOp::Lt => gap > 0,
        BinaryOp::Le => gap >= 0,
        BinaryOp::Gt => gap < 0,
        BinaryOp::Ge => gap <= 0,
        BinaryOp::Ne => gap != 0,
        _ => unreachable!(),
    };
    let condition_probes = std::mem::take(&mut analysis.probes);
    if !active {
        return analysis.safe(&condition_probes, &[], 0);
    }
    let paths = analysis.block(
        body,
        vec![Path {
            exit: Exit::Next,
            state: zero,
        }],
    )?;
    let continuing: Vec<_> = paths
        .iter()
        .filter(|path| matches!(path.exit, Exit::Next | Exit::Continue))
        .map(|path| path.state.clone())
        .collect();
    let iterations = horizon(*op, gap, &rank, &continuing)?;
    analysis.safe(&condition_probes, &continuing, iterations)?;
    analysis.safe(&analysis.probes, &continuing, iterations.checked_sub(1)?)
}

fn horizon(op: BinaryOp, gap: i128, rank: &Form, paths: &[Vec<i128>]) -> Option<i128> {
    if paths.is_empty() {
        return Some(1);
    }
    let (min, max) = displacement_range(rank, paths)?;
    if op == BinaryOp::Ne {
        if min != max || min == 0 || min.signum() == gap.signum() {
            return None;
        }
        let distance = gap.checked_abs()?;
        let stride = min.checked_abs()?;
        return (distance.checked_rem(stride)? == 0).then_some(distance / stride);
    }
    let (distance, stride) = match op {
        BinaryOp::Lt | BinaryOp::Le if max < 0 => (gap, max.checked_neg()?),
        BinaryOp::Gt | BinaryOp::Ge if min > 0 => (gap.checked_neg()?, min),
        _ => return None,
    };
    let distance = distance.checked_add(i128::from(matches!(op, BinaryOp::Le | BinaryOp::Ge)))?;
    (distance / stride).checked_add(i128::from(distance % stride != 0))
}

fn displacement_range(form: &Form, paths: &[Vec<i128>]) -> Option<(i128, i128)> {
    let mut min = i128::MAX;
    let mut max = i128::MIN;
    for path in paths {
        let delta = form.displacement(path)?;
        min = min.min(delta);
        max = max.max(delta);
    }
    Some(if paths.is_empty() { (0, 0) } else { (min, max) })
}

impl Analysis<'_> {
    fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|candidate| *candidate == name)
    }

    fn initial(&self, form: &Form) -> Option<i128> {
        form.coefficients
            .iter()
            .zip(&self.values)
            .try_fold(form.constant, |sum, (coefficient, value)| {
                sum.checked_add(coefficient.checked_mul(value.0)?)
            })
    }

    fn safe(&self, probes: &[Probe], paths: &[Vec<i128>], preceding: i128) -> Option<()> {
        for probe in probes {
            let base = self
                .initial(&probe.form)?
                .checked_add(probe.form.displacement(&probe.state)?)?;
            let (min, max) = displacement_range(&probe.form, paths)?;
            let low = base.checked_add(preceding.checked_mul(min.min(0))?)?;
            let high = base.checked_add(preceding.checked_mul(max.max(0))?)?;
            if low < probe.domain.0 || high > probe.domain.1 {
                return None;
            }
        }
        Some(())
    }

    fn domain(&self, expression: &Expr) -> Option<Domain> {
        match expression.unlocated() {
            Expr::Name(name) => {
                let value = self.values[self.index(name)?];
                Some(Domain::Known(value.1, value.2))
            }
            Expr::Int(text) if text.ends_with('u') => Some(Domain::Known(0, i128::from(u64::MAX))),
            Expr::Int(_) => Some(Domain::Literal),
            Expr::Unary {
                op: UnaryOp::Neg,
                value,
            } => self.domain(value),
            Expr::Binary {
                left,
                op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul,
                right,
            } => unify_domain(self.domain(left)?, self.domain(right)?),
            _ => None,
        }
    }

    fn domain_pair(&self, left: &Expr, right: &Expr) -> Option<(i128, i128)> {
        Some(unify_domain(self.domain(left)?, self.domain(right)?)?.bounds())
    }

    fn form(&mut self, expression: &Expr, domain: (i128, i128), state: &[i128]) -> Option<Form> {
        // Only bare literals inherit operand context in the checked NC AST.
        // Compound expressions and unary operations retain their own domains.
        let own_domain = match expression.unlocated() {
            Expr::Binary { .. } | Expr::Unary { .. } => self.domain(expression)?.bounds(),
            _ => domain,
        };
        if (own_domain.0 < 0) != (domain.0 < 0) {
            // Compound signed results cannot be implicitly retyped as unsigned
            // (or vice versa); only bare literals receive contextual typing.
            return None;
        }
        let form = match expression.unlocated() {
            Expr::Int(text) => Form::constant(integer(text)?, self.names.len()),
            Expr::Name(name) => {
                let index = self.index(name)?;
                let value = self.values[index];
                if (value.1, value.2) != domain {
                    return None;
                }
                let mut form = Form::constant(0, self.names.len());
                form.coefficients[index] = 1;
                form
            }
            Expr::Unary {
                op: UnaryOp::Neg,
                value,
            } => {
                // NC admits the signed minimum as a negated literal even though
                // its positive magnitude is not representable in the same type.
                if let Expr::Int(text) = value.unlocated() {
                    Form::constant(integer(text)?.checked_neg()?, self.names.len())
                } else {
                    self.form(value, own_domain, state)?.scaled(-1)?
                }
            }
            Expr::Binary { left, op, right } => {
                let left = self.form(left, own_domain, state)?;
                let right = self.form(right, own_domain, state)?;
                match op {
                    BinaryOp::Add => left.added(&right)?,
                    BinaryOp::Sub => left.added(&right.scaled(-1)?)?,
                    BinaryOp::Mul => {
                        if let Some(factor) = self.invariant(&left) {
                            right.scaled(factor)?
                        } else {
                            left.scaled(self.invariant(&right)?)?
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        self.probes.push(Probe {
            form: form.clone(),
            state: state.to_vec(),
            domain: own_domain,
        });
        if domain != own_domain {
            self.probes.push(Probe {
                form: form.clone(),
                state: state.to_vec(),
                domain,
            });
        }
        Some(form)
    }

    fn invariant(&self, form: &Form) -> Option<i128> {
        if form
            .coefficients
            .iter()
            .zip(&self.written)
            .any(|(coefficient, written)| *written && *coefficient != 0)
        {
            return None;
        }
        self.initial(form)
    }

    fn block(&mut self, block: &Block, mut paths: Vec<Path>) -> Option<Vec<Path>> {
        for statement in &block.statements {
            let mut next = Vec::new();
            for path in paths {
                if path.exit == Exit::Next {
                    next.extend(self.statement(statement, path)?);
                } else {
                    next.push(path);
                }
            }
            paths = next;
        }
        Some(paths)
    }

    fn statement(&mut self, statement: &Stmt, mut path: Path) -> Option<Vec<Path>> {
        match statement.unlocated() {
            Stmt::Assign { target, value } => {
                let Expr::Name(name) = target.unlocated() else {
                    return None;
                };
                let index = self.index(name)?;
                let initial = self.values[index];
                let domain = (initial.1, initial.2);
                let mut change = self.form(value, domain, &path.state)?;
                change.coefficients[index] = change.coefficients[index].checked_sub(1)?;
                let delta = self.invariant(&change)?;
                path.state[index] = path.state[index].checked_add(delta)?;
                self.form(target, domain, &path.state)?;
            }
            Stmt::Block(block) => return self.block(block, vec![path]),
            Stmt::Expr(value) | Stmt::Assert(value) => return self.expression(value, path),
            Stmt::Var(declaration) => {
                if declaration.mutex
                    || declaration
                        .binding_names()
                        .iter()
                        .any(|name| self.index(name).is_some())
                {
                    return None;
                }
                return self.expression(&declaration.value, path);
            }
            Stmt::LabeledIf { label, value } => {
                let mut paths = self.expression(value, path)?;
                for path in &mut paths {
                    if path.exit == Exit::Break(Some(label.clone())) {
                        path.exit = Exit::Next;
                    }
                }
                return Some(paths);
            }
            Stmt::Return(value) => {
                if let Some(value) = value {
                    self.read(value, &path.state)?;
                }
                path.exit = Exit::Return;
            }
            Stmt::Throw(value) => {
                self.read(value, &path.state)?;
                path.exit = Exit::Return;
            }
            Stmt::Break(None, label) => path.exit = Exit::Break(label.clone()),
            Stmt::Continue(_) => path.exit = Exit::Continue,
            _ => return None,
        }
        Some(vec![path])
    }

    fn expression(&mut self, expression: &Expr, path: Path) -> Option<Vec<Path>> {
        match expression.unlocated() {
            Expr::If { subject, arms } => {
                if let Some(subject) = subject {
                    self.read(subject, &path.state)?;
                }
                let mut paths = Vec::new();
                for (patterns, body) in arms {
                    for pattern in patterns {
                        self.pattern(pattern, &path.state)?;
                    }
                    paths.extend(self.block(body, vec![path.clone()])?);
                }
                if !exhaustive(subject.as_deref(), arms) {
                    paths.push(path);
                }
                Some(paths)
            }
            Expr::Call {
                callee,
                args,
                generics,
            } => {
                let Expr::Name(name) = callee.unlocated() else {
                    return None;
                };
                if !matches!(name.as_str(), "@print" | "@println") || !generics.is_empty() {
                    return None;
                }
                for argument in args {
                    self.read(argument, &path.state)?;
                }
                Some(vec![path])
            }
            _ => {
                self.read(expression, &path.state)?;
                Some(vec![path])
            }
        }
    }

    fn read(&mut self, expression: &Expr, state: &[i128]) -> Option<()> {
        match expression.unlocated() {
            Expr::Name(_) | Expr::Bool(_) | Expr::String(_) | Expr::Char(_) | Expr::None => {
                Some(())
            }
            Expr::Binary {
                left,
                op:
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge,
                right,
            } => {
                if let Some(domain) = self.domain_pair(left, right) {
                    self.form(left, domain, state)?;
                    self.form(right, domain, state)?;
                    Some(())
                } else {
                    self.read_pair(left, right, state)
                }
            }
            Expr::Unary {
                op: UnaryOp::Not | UnaryOp::BitNot,
                value,
            }
            | Expr::Cast { value, .. } => self.read(value, state),
            Expr::Unary {
                op: UnaryOp::Neg,
                value,
            } if self.domain(expression).is_none() => self.read(value, state),
            Expr::Binary {
                left,
                op:
                    BinaryOp::And
                    | BinaryOp::Or
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow
                    | BinaryOp::BitAnd
                    | BinaryOp::BitOr
                    | BinaryOp::BitXor
                    | BinaryOp::Shl
                    | BinaryOp::Shr,
                right,
            } => self.read_pair(left, right, state),
            Expr::Binary {
                left,
                op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul,
                right,
            } if self.domain(expression).is_none() => self.read_pair(left, right, state),
            _ => {
                let domain = self.domain(expression)?.bounds();
                self.form(expression, domain, state)?;
                Some(())
            }
        }
    }

    fn read_pair(&mut self, left: &Expr, right: &Expr, state: &[i128]) -> Option<()> {
        // Purity, not arithmetic success, is the obligation for these reads.
        // Failures are diagnosed by execution only after the rank is certified.
        self.read(left, state)?;
        self.read(right, state)
    }

    fn pattern(&mut self, pattern: &Pattern, state: &[i128]) -> Option<()> {
        match pattern {
            Pattern::Literal(value) => self.read(value, state),
            Pattern::Wildcard | Pattern::Name(_) => Some(()),
            _ => None,
        }
    }

    fn find_writes(&mut self, block: &Block) -> Option<()> {
        for statement in &block.statements {
            match statement.unlocated() {
                Stmt::Assign { target, .. } => {
                    if let Expr::Name(name) = target.unlocated()
                        && let Some(index) = self.index(name)
                    {
                        self.written[index] = true;
                    }
                }
                Stmt::Block(block) => self.find_writes(block)?,
                Stmt::Expr(value) | Stmt::Assert(value) | Stmt::LabeledIf { value, .. } => {
                    self.expression_writes(value)?;
                }
                Stmt::Var(declaration) => {
                    if declaration
                        .binding_names()
                        .iter()
                        .any(|name| self.index(name).is_some())
                    {
                        return None;
                    }
                    self.expression_writes(&declaration.value)?;
                }
                _ => {}
            }
        }
        Some(())
    }

    fn expression_writes(&mut self, expression: &Expr) -> Option<()> {
        if let Expr::If { arms, .. } = expression.unlocated() {
            for (_, block) in arms {
                self.find_writes(block)?;
            }
        }
        Some(())
    }
}

fn exhaustive(subject: Option<&Expr>, arms: &[(Vec<Pattern>, Block)]) -> bool {
    let mut booleans = 0_u8;
    for pattern in arms.iter().flat_map(|(patterns, _)| patterns) {
        match pattern {
            Pattern::Wildcard | Pattern::Name(_) => return true,
            Pattern::Literal(value) => {
                if let Expr::Bool(value) = value.unlocated() {
                    booleans |= if *value { 1 } else { 2 };
                }
            }
            _ => {}
        }
    }
    // A subjectless conditional matches guards against true, not an unknown
    // boolean subject. Otherwise both boolean literals must be represented.
    if subject.is_none() {
        booleans & 1 != 0
    } else {
        booleans == 3
    }
}

fn unify_domain(left: Domain, right: Domain) -> Option<Domain> {
    match (left, right) {
        (Domain::Known(lmin, lmax), Domain::Known(rmin, rmax)) if (lmin, lmax) != (rmin, rmax) => {
            None
        }
        (domain @ Domain::Known(_, _), _) | (_, domain @ Domain::Known(_, _)) => Some(domain),
        (Domain::Literal, Domain::Literal) => Some(Domain::Literal),
    }
}

fn integer(text: &str) -> Option<i128> {
    let text = text.strip_suffix('u').unwrap_or(text).replace('_', "");
    for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return i128::from_str_radix(digits, radix).ok();
        }
    }
    text.parse().ok()
}

#[cfg(test)]
#[path = "termination_affine_tests.rs"]
mod tests;
