//! Conservative certificates for counted integer loops. Unknown is not infinite.
use std::collections::HashSet;

use crate::ast::{BinaryOp, Block, Expr, Stmt, UnaryOp};

pub(crate) struct CountedLoop<'a> {
    pub(crate) counter: &'a str,
    pub(crate) bound: &'a Expr,
    pub(crate) comparison: BinaryOp,
    pub(crate) step: i128,
}

pub(crate) fn counted_loop<'a>(condition: &'a Expr, body: &Block) -> Option<CountedLoop<'a>> {
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
    let mut protected = HashSet::from([counter.as_str()]);
    if !bound_names(right, counter, &mut protected) {
        return None;
    }
    let mut step = None;
    for statement in &body.statements {
        if let Some(delta) = update(statement, counter) {
            if step.replace(delta).is_some() {
                return None;
            }
        } else if !safe_statement(statement, &protected, step.is_some()) {
            return None;
        }
    }
    Some(CountedLoop {
        counter,
        bound: right,
        comparison: *op,
        step: step?,
    })
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

fn safe_expression(expression: &Expr, protected: &HashSet<&str>, progressed: bool) -> bool {
    match expression.unlocated() {
        Expr::If { subject, arms } => {
            subject
                .as_deref()
                .is_none_or(|value| safe_expression(value, protected, progressed))
                && arms.iter().all(|(patterns, body)| {
                    let mut safe = true;
                    for pattern in patterns {
                        crate::visit::pattern(pattern, &mut |value| {
                            safe &= leaf_expression(value);
                        });
                    }
                    safe && safe_block(body, protected, progressed)
                })
        }
        Expr::Else { value, fallback } => {
            safe_expression(value, protected, progressed)
                && safe_block(fallback, protected, progressed)
        }
        Expr::Catch { value, name, body } => {
            !protected.contains(name.as_str())
                && safe_expression(value, protected, progressed)
                && safe_block(body, protected, progressed)
        }
        _ => {
            let mut safe = true;
            let item = crate::ast::Item::Statement(Stmt::Expr(expression.clone()));
            crate::visit::item(&item, &mut |value| safe &= leaf_expression(value));
            safe
        }
    }
}

fn leaf_expression(expression: &Expr) -> bool {
    match expression {
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

fn safe_block(body: &Block, protected: &HashSet<&str>, progressed: bool) -> bool {
    body.statements
        .iter()
        .all(|statement| safe_statement(statement, protected, progressed))
}

fn safe_statement(statement: &Stmt, protected: &HashSet<&str>, progressed: bool) -> bool {
    match statement.unlocated() {
        Stmt::Assign { target, value } => {
            root_name(target).is_some_and(|name| !protected.contains(name))
                && safe_expression(target, protected, progressed)
                && safe_expression(value, protected, progressed)
        }
        Stmt::Var(declaration) => {
            !declaration.mutex
                && declaration
                    .binding_names()
                    .iter()
                    .all(|name| !protected.contains(name))
                && safe_expression(&declaration.value, protected, progressed)
        }
        Stmt::Block(body) => safe_block(body, protected, progressed),
        Stmt::Expr(value)
        | Stmt::Assert(value)
        | Stmt::Throw(value)
        | Stmt::LabeledIf { value, .. } => safe_expression(value, protected, progressed),
        Stmt::Return(value) | Stmt::Break(value, _) => value
            .as_ref()
            .is_none_or(|value| safe_expression(value, protected, progressed)),
        Stmt::Continue(_) => progressed,
        Stmt::While {
            condition, body, ..
        } => {
            safe_expression(condition, protected, progressed)
                && safe_block(body, protected, progressed)
        }
        Stmt::For {
            name,
            iterable,
            body,
            ..
        } => {
            !protected.contains(name.as_str())
                && safe_expression(iterable, protected, progressed)
                && safe_block(body, protected, progressed)
        }
        Stmt::Lock { .. } => false,
        Stmt::Located(_, _) => unreachable!(),
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
        let stop = match self.comparison {
            Lt | Gt | Ne => bound,
            Le => bound + 1,
            Ge => bound - 1,
            _ => return false,
        };
        let distance = stop - start;
        if distance.signum() != self.step.signum() {
            return false;
        }
        if self.comparison == Ne && distance % self.step != 0 {
            return false;
        }
        let distance = distance.abs();
        let stride = self.step.abs();
        let iterations = distance / stride + i128::from(distance % stride != 0);
        let final_value = start + iterations * self.step;
        (min..=max).contains(&final_value)
    }
}
