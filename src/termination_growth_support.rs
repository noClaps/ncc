//! Shared, deliberately small syntax and interval machinery for standalone certificates.
use crate::ast::{BinaryOp, Block, Expr, Pattern, Stmt, UnaryOp, VarDecl};
#[path = "termination_growth_reads.rs"]
mod reads;
pub(crate) use reads::{declaration_safe, read_safe};
use reads::{output_syntax, pure_syntax};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Update {
    Add(i128),
    Mul(i128),
    Set(i128),
}

#[derive(Clone)]
pub(crate) enum Step<'a> {
    Guard(&'a Expr, bool),
    Constant(&'a str, &'a Expr),
    Update(&'a str, Update),
    Read(&'a Expr),
    Declare(&'a VarDecl),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Exit {
    #[default]
    Next,
    Break,
    Continue,
}

#[derive(Clone, Default)]
pub(crate) struct Path<'a> {
    pub steps: Vec<Step<'a>>,
    pub exit: Exit,
}

pub(crate) fn integer(value: &Expr) -> Option<i128> {
    match value.unlocated() {
        Expr::Int(text) => crate::lexer::integer(text).ok().map(i128::from),
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => {
            // A signed-minimum literal is allowed, but further negations are
            // real arithmetic whose intermediate type limits must not vanish.
            let Expr::Int(text) = value.unlocated() else {
                return None;
            };
            let magnitude = crate::lexer::integer(text).ok()?;
            if text.ends_with('u') && magnitude != 0 {
                return None;
            }
            i128::from(magnitude).checked_neg()
        }
        _ => None,
    }
}

/// Validate the original literal, not its reduced displacement: unsigned
/// subtraction by 1 has a negative delta but a valid positive operand.
pub(crate) fn constant(value: &Expr, limits: (i128, i128, i128)) -> Option<i128> {
    let value = integer(value)?;
    (limits.1..=limits.2).contains(&value).then_some(value)
}

pub(crate) fn comparison(value: &Expr) -> Option<(&str, BinaryOp, &Expr)> {
    let Expr::Binary { left, op, right } = value.unlocated() else {
        return None;
    };
    if !matches!(
        op,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    ) {
        return None;
    }
    if let Expr::Name(name) = left.unlocated() {
        Some((name, *op, right))
    } else if let Expr::Name(name) = right.unlocated() {
        Some((name, reverse(*op), left))
    } else {
        None
    }
}

pub(crate) fn reverse(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Le => BinaryOp::Ge,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::Ge => BinaryOp::Le,
        _ => op,
    }
}

fn update<'a>(target: &'a Expr, value: &'a Expr) -> Option<(&'a str, Update, &'a Expr)> {
    let Expr::Name(name) = target.unlocated() else {
        return None;
    };
    if let Some(number) = integer(value) {
        return Some((name, Update::Set(number), value));
    }
    let Expr::Binary { left, op, right } = value.unlocated() else {
        return None;
    };
    let (constant, swapped) = if matches!(left.unlocated(), Expr::Name(n) if n == name) {
        (right.as_ref(), false)
    } else if matches!(right.unlocated(), Expr::Name(n) if n == name) {
        (left.as_ref(), true)
    } else {
        return None;
    };
    let number = integer(constant)?;
    let change = match op {
        BinaryOp::Add => Update::Add(number),
        BinaryOp::Sub if !swapped => Update::Add(number.checked_neg()?),
        BinaryOp::Mul => Update::Mul(number),
        _ => return None,
    };
    Some((name, change, constant))
}

/// Paths retain guard/update/read order. Only builtin output calls are admitted;
/// their arguments and other reads must pass the static syntax and safety checks.
/// Labels cannot be consumed inside this subset: labeled breaks leave the loop,
/// and requiring progress for all labeled continues conservatively covers either
/// this loop or an enclosing loop as their destination.
pub(crate) fn paths(body: &Block) -> Option<Vec<Path<'_>>> {
    block(body, vec![Path::default()])
}

fn block<'a>(body: &'a Block, mut paths: Vec<Path<'a>>) -> Option<Vec<Path<'a>>> {
    for statement in &body.statements {
        let mut next = Vec::new();
        for mut path in paths {
            if path.exit != Exit::Next {
                next.push(path);
                continue;
            }
            match statement.unlocated() {
                Stmt::Assign { target, value } => {
                    let (name, change, operand) = update(target, value)?;
                    path.steps.push(Step::Constant(name, operand));
                    path.steps.push(Step::Update(name, change));
                    next.push(path);
                }
                Stmt::Block(body) => next.extend(block(body, vec![path])?),
                Stmt::Expr(value) if matches!(value.unlocated(), Expr::If { .. }) => {
                    next.extend(branch(value, &path)?);
                }
                Stmt::Expr(value) if output_syntax(value) || pure_syntax(value) => {
                    path.steps.push(Step::Read(value));
                    next.push(path);
                }
                Stmt::Assert(value) if pure_syntax(value) => {
                    // Ignoring assertion failure overapproximates continuing paths.
                    path.steps.push(Step::Read(value));
                    next.push(path);
                }
                Stmt::Var(declaration) if !declaration.mutex && pure_syntax(&declaration.value) => {
                    if !matches!(declaration.pattern, Pattern::Name(_)) {
                        return None;
                    }
                    path.steps.push(Step::Declare(declaration));
                    next.push(path);
                }
                Stmt::Break(None, _) => {
                    path.exit = Exit::Break;
                    next.push(path);
                }
                Stmt::Continue(_) => {
                    path.exit = Exit::Continue;
                    next.push(path);
                }
                _ => return None,
            }
        }
        paths = next;
    }
    Some(paths)
}

fn branch<'a>(value: &'a Expr, input: &Path<'a>) -> Option<Vec<Path<'a>>> {
    let Expr::If { subject, arms } = value.unlocated() else {
        return None;
    };
    if subject.as_ref().is_some_and(|value| !guard_syntax(value)) {
        return None;
    }
    let mut remaining = input.clone();
    if let Some(subject) = subject {
        // Even a wildcard-only conditional evaluates its subject.
        remaining.steps.push(Step::Read(subject));
    }
    let mut result = Vec::new();
    let mut boolean_coverage = 0_u8;
    for (patterns, body) in arms {
        let [pattern] = patterns.as_slice() else {
            return None;
        };
        if matches!(pattern, Pattern::Wildcard) {
            result.extend(block(body, vec![remaining])?);
            return Some(result);
        }
        let Pattern::Literal(value) = pattern else {
            return None;
        };
        let (guard, truth) = if let Some(subject) = subject {
            let Expr::Bool(truth) = value.unlocated() else {
                return None;
            };
            (subject.as_ref(), *truth)
        } else {
            (value.as_ref(), true)
        };
        if !guard_syntax(guard) {
            return None;
        }
        let mut selected = remaining.clone();
        selected.steps.push(Step::Guard(guard, truth));
        result.extend(block(body, vec![selected])?);
        if subject.is_some() {
            boolean_coverage |= if truth { 1 } else { 2 };
            if boolean_coverage == 3 {
                return Some(result);
            }
        } else if matches!(guard.unlocated(), Expr::Bool(true)) {
            return Some(result);
        }
        remaining.steps.push(Step::Guard(guard, !truth));
    }
    result.push(remaining);
    Some(result)
}

fn guard_syntax(value: &Expr) -> bool {
    matches!(value.unlocated(), Expr::Bool(_))
        || comparison(value).is_some_and(|(_, _, bound)| integer(bound).is_some())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Interval {
    pub low: i128,
    pub high: i128,
}

impl Interval {
    pub fn checked(self, update: Update, limits: (i128, i128, i128)) -> Option<Self> {
        let result = match update {
            Update::Add(delta) => Self {
                low: self.low.checked_add(delta)?,
                high: self.high.checked_add(delta)?,
            },
            Update::Mul(factor) if factor > 0 => Self {
                low: self.low.checked_mul(factor)?,
                high: self.high.checked_mul(factor)?,
            },
            Update::Set(value) => Self {
                low: value,
                high: value,
            },
            Update::Mul(_) => return None,
        };
        (result.low >= limits.1 && result.high <= limits.2).then_some(result)
    }

    pub fn refine(&mut self, op: BinaryOp, bound: i128, truth: bool) -> Option<()> {
        let op = if truth {
            op
        } else {
            match op {
                BinaryOp::Lt => BinaryOp::Ge,
                BinaryOp::Le => BinaryOp::Gt,
                BinaryOp::Gt => BinaryOp::Le,
                BinaryOp::Ge => BinaryOp::Lt,
                _ => return None,
            }
        };
        match op {
            BinaryOp::Lt => self.high = self.high.min(bound.checked_sub(1)?),
            BinaryOp::Le => self.high = self.high.min(bound),
            BinaryOp::Gt => self.low = self.low.max(bound.checked_add(1)?),
            BinaryOp::Ge => self.low = self.low.max(bound),
            _ => return None,
        }
        Some(())
    }
}

pub(crate) fn valid(value: &(i128, i128, i128)) -> bool {
    (value.1..=value.2).contains(&value.0)
}

/// Guards are effect-free, single comparisons against substituted integer constants.
/// The boolean return distinguishes an impossible path from unsupported syntax.
pub(crate) fn guard(
    value: &Expr,
    truth: bool,
    names: [&str; 2],
    ranges: &mut [Interval; 2],
    limits: &[(i128, i128, i128); 2],
) -> Option<bool> {
    if let Expr::Bool(value) = value.unlocated() {
        return Some(*value == truth);
    }
    let (name, op, operand) = comparison(value)?;
    let index = names.iter().position(|candidate| *candidate == name)?;
    ranges[index].refine(op, constant(operand, limits[index])?, truth)?;
    Some(ranges[index].low <= ranges[index].high)
}
