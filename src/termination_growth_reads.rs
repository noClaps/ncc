//! Effect-free read validation, separate from ranking updates.
use super::{Interval, constant, integer};
use crate::ast::{BinaryOp, Expr, Type, UnaryOp, VarDecl};

pub(super) fn output_syntax(value: &Expr) -> bool {
    matches!(value.unlocated(), Expr::Call { callee, args, generics }
        if generics.is_empty()
            && matches!(callee.unlocated(), Expr::Name(name) if name == "@print" || name == "@println")
            && args.iter().all(pure_syntax))
}

pub(super) fn pure_syntax(value: &Expr) -> bool {
    match value.unlocated() {
        Expr::Name(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::String(_)
        | Expr::Char(_)
        | Expr::Bool(_)
        | Expr::None => true,
        Expr::Unary {
            op: UnaryOp::Neg, ..
        } => integer(value).is_some(),
        Expr::Unary {
            op: UnaryOp::Not,
            value,
        } => pure_syntax(value),
        Expr::Binary { left, op, right } => {
            matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
            ) && pure_syntax(left)
                && pure_syntax(right)
        }
        _ => false,
    }
}

/// Output remains in the original AST. We check original literal operands in
/// their inferred domains before checking arithmetic results. Without complete
/// expression types, unknown integer domains and intermediate results are
/// restricted to the common native integer range (0..=255).
pub(crate) fn read_safe(
    value: &Expr,
    names: [&str; 2],
    ranges: &[Interval; 2],
    limits: &[(i128, i128, i128); 2],
) -> Option<()> {
    read_in(value, names, ranges, limits, None)
}

pub(crate) fn declaration_safe(
    declaration: &VarDecl,
    names: [&str; 2],
    ranges: &[Interval; 2],
    limits: &[(i128, i128, i128); 2],
) -> Option<()> {
    let Type::Named(name, generics) = &declaration.ty else {
        return None;
    };
    if !generics.is_empty() {
        return None;
    }
    let domain = match name.as_str() {
        "int" => Some(signed_domain()),
        "uint" => Some(unsigned_domain()),
        "byte" => Some(common_domain()),
        "float" | "bool" | "str" | "char" => None,
        _ => return None,
    };
    read_in(&declaration.value, names, ranges, limits, domain)
}

fn read_in(
    value: &Expr,
    names: [&str; 2],
    ranges: &[Interval; 2],
    limits: &[(i128, i128, i128); 2],
    expected: Option<Interval>,
) -> Option<()> {
    let domain = merge_domains(expected, operand_domain(value, names, limits));
    match value.unlocated() {
        Expr::Int(_)
        | Expr::Unary {
            op: UnaryOp::Neg, ..
        } => {
            literal_in(value, domain.unwrap_or_else(signed_domain))?;
        }
        Expr::Call { args, .. } if output_syntax(value) => {
            for argument in args {
                read_in(argument, names, ranges, limits, None)?;
            }
        }
        Expr::Binary {
            op: BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul,
            ..
        } => {
            numeric(
                value,
                names,
                ranges,
                limits,
                domain.unwrap_or_else(signed_domain),
            )?;
        }
        Expr::Binary {
            left,
            op: BinaryOp::And | BinaryOp::Or,
            right,
        } => {
            read_in(left, names, ranges, limits, None)?;
            read_in(right, names, ranges, limits, None)?;
        }
        Expr::Binary { left, right, .. } if pure_syntax(value) => {
            read_in(left, names, ranges, limits, domain)?;
            read_in(right, names, ranges, limits, domain)?;
        }
        Expr::Unary {
            op: UnaryOp::Not,
            value,
        } => read_in(value, names, ranges, limits, None)?,
        _ if pure_syntax(value) => {}
        _ => return None,
    }
    Some(())
}

fn signed_domain() -> Interval {
    Interval {
        low: i128::from(i64::MIN),
        high: i128::from(i64::MAX),
    }
}

fn unsigned_domain() -> Interval {
    Interval {
        low: 0,
        high: i128::from(u64::MAX),
    }
}

fn common_domain() -> Interval {
    Interval { low: 0, high: 255 }
}

fn merge_domains(left: Option<Interval>, right: Option<Interval>) -> Option<Interval> {
    match (left, right) {
        (Some(left), Some(right)) => Some(Interval {
            low: left.low.max(right.low),
            high: left.high.min(right.high),
        }),
        (left, right) => left.or(right),
    }
}

fn operand_domain(
    value: &Expr,
    names: [&str; 2],
    limits: &[(i128, i128, i128); 2],
) -> Option<Interval> {
    match value.unlocated() {
        Expr::Name(name) => Some(
            names
                .iter()
                .position(|candidate| *candidate == name)
                .map_or_else(common_domain, |index| Interval {
                    low: limits[index].1,
                    high: limits[index].2,
                }),
        ),
        Expr::Int(text) if text.ends_with('u') => Some(unsigned_domain()),
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => operand_domain(value, names, limits),
        Expr::Binary {
            op:
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge,
            left,
            right,
        } => merge_domains(
            operand_domain(left, names, limits),
            operand_domain(right, names, limits),
        ),
        _ => None,
    }
}

fn literal_in(value: &Expr, domain: Interval) -> Option<i128> {
    constant(value, (0, domain.low, domain.high))
}

struct Scalar {
    range: Interval,
    limits: Interval,
}

fn numeric(
    value: &Expr,
    names: [&str; 2],
    ranges: &[Interval; 2],
    limits: &[(i128, i128, i128); 2],
    domain: Interval,
) -> Option<Scalar> {
    if matches!(
        value.unlocated(),
        Expr::Int(_)
            | Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
    ) {
        let value = literal_in(value, domain)?;
        return Some(Scalar {
            range: Interval {
                low: value,
                high: value,
            },
            limits: common_domain(),
        });
    }
    if let Expr::Name(name) = value.unlocated() {
        let index = names.iter().position(|candidate| *candidate == name)?;
        return Some(Scalar {
            range: ranges[index],
            limits: Interval {
                low: limits[index].1,
                high: limits[index].2,
            },
        });
    }
    let Expr::Binary { left, op, right } = value.unlocated() else {
        return None;
    };
    let left = numeric(left, names, ranges, limits, domain)?;
    let right = numeric(right, names, ranges, limits, domain)?;
    let result = Scalar {
        range: arithmetic(left.range, *op, right.range)?,
        limits: Interval {
            low: 0.max(domain.low).max(left.limits.low).max(right.limits.low),
            high: 255
                .min(domain.high)
                .min(left.limits.high)
                .min(right.limits.high),
        },
    };
    (result.range.low >= result.limits.low && result.range.high <= result.limits.high)
        .then_some(result)
}

fn arithmetic(left: Interval, op: BinaryOp, right: Interval) -> Option<Interval> {
    match op {
        BinaryOp::Add => Some(Interval {
            low: left.low.checked_add(right.low)?,
            high: left.high.checked_add(right.high)?,
        }),
        BinaryOp::Sub => Some(Interval {
            low: left.low.checked_sub(right.high)?,
            high: left.high.checked_sub(right.low)?,
        }),
        BinaryOp::Mul => {
            let products = [
                left.low.checked_mul(right.low)?,
                left.low.checked_mul(right.high)?,
                left.high.checked_mul(right.low)?,
                left.high.checked_mul(right.high)?,
            ];
            Some(Interval {
                low: *products.iter().min()?,
                high: *products.iter().max()?,
            })
        }
        _ => None,
    }
}
