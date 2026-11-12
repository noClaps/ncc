//! Types as completion and signature help work with them: syntax trees (`ast::TypeExpr`), from
//! checked types (`Types::to_type_expr`), signatures and the code being written (read by the
//! parser). Generic parameters stay as names, so a signature's `T[]` can be matched against an
//! argument's `int[]`.

use std::collections::HashMap;

use crate::ast::{ConstExpr, TypeExpr as T};
use crate::diag::Span;

/// A type written as a single name (`int`, `Point`).
pub fn named(name: &str) -> T {
    T::Named {
        module: None,
        name: name.to_string(),
        args: vec![],
        span: Span::default(),
    }
}

/// Whether two types are written the same.
pub fn same(a: &T, b: &T) -> bool {
    a.show() == b.show()
}

pub fn is_fn(t: &T) -> bool {
    matches!(t, T::Fn(..))
}

/// The element type of an array type (`int[]`, `P[3]` -> `int`, `P`); not of a map.
pub fn elem(t: &T) -> Option<&T> {
    match t {
        T::Array(e, ..) => Some(e),
        _ => None,
    }
}

/// The generic parameter `t` is when it is one of `tps`.
fn param<'a>(t: &'a T, tps: &[String]) -> Option<&'a str> {
    match t {
        T::Named {
            module: None,
            name,
            args,
            ..
        } if args.is_empty() && tps.contains(name) => Some(name),
        _ => None,
    }
}

/// The generic constant `c` is when it is one of `tps`.
fn const_param<'a>(c: &'a ConstExpr, tps: &[String]) -> Option<&'a str> {
    match c {
        ConstExpr::Name(n, _) if tps.contains(n) => Some(n),
        _ => None,
    }
}

/// Apply `f` to each type directly inside `t`.
fn children(t: &T) -> Vec<&T> {
    match t {
        T::Named { args, .. } | T::Tuple(args, _) => args.iter().collect(),
        T::Array(e, ..) | T::Optional(e, _) | T::ErrorUnion(e, _) | T::Fut(e, _) => vec![e],
        T::Map(k, v, _) => vec![k, v],
        T::Fn(ps, r, _) => ps.iter().chain(r.as_deref()).collect(),
        T::Const(_) => vec![],
    }
}

/// Whether `t` names one of the generic parameters `tps`.
pub fn mentions(t: &T, tps: &[String]) -> bool {
    let in_size = |c: &ConstExpr| {
        let mut found = false;
        c.visit_names(&mut |n| found |= tps.iter().any(|t| t == n));
        found
    };
    param(t, tps).is_some()
        || matches!(t, T::Array(_, Some(n), _) if in_size(n))
        || matches!(t, T::Const(c) if in_size(c))
        || children(t).into_iter().any(|c| mentions(c, tps))
}

/// `t` with the generic parameters in `binds` replaced by what they stand for.
pub fn substitute(t: &T, binds: &HashMap<String, T>) -> T {
    let sub = |x: &T| substitute(x, binds);
    let sub_const = |c: &ConstExpr| {
        c.map_names(&mut |n| match binds.get(n) {
            Some(T::Const(v)) => Some(v.clone()),
            _ => None,
        })
    };
    let tps: Vec<String> = binds.keys().cloned().collect();
    if let Some(p) = param(t, &tps) {
        return binds[p].clone();
    }
    match t {
        T::Named {
            module,
            name,
            args,
            span,
        } => T::Named {
            module: module.clone(),
            name: name.clone(),
            args: args.iter().map(sub).collect(),
            span: *span,
        },
        T::Array(e, n, s) => T::Array(Box::new(sub(e)), n.as_ref().map(sub_const), *s),
        T::Map(k, v, s) => T::Map(Box::new(sub(k)), Box::new(sub(v)), *s),
        T::Optional(e, s) => T::Optional(Box::new(sub(e)), *s),
        T::ErrorUnion(e, s) => T::ErrorUnion(Box::new(sub(e)), *s),
        T::Fut(e, s) => T::Fut(Box::new(sub(e)), *s),
        T::Tuple(ts, s) => T::Tuple(ts.iter().map(sub).collect(), *s),
        T::Fn(ps, r, s) => T::Fn(
            ps.iter().map(sub).collect(),
            r.as_deref().map(|r| Box::new(sub(r))),
            *s,
        ),
        T::Const(c) => T::Const(sub_const(c)),
    }
}

/// The generic parameters `tps` bound so that a value of type `ty` can be passed for a
/// parameter of type `param` (directly, or as the value of an optional parameter).
pub fn accepts(param: &T, ty: &T, tps: &[String]) -> Option<HashMap<String, T>> {
    let try_match = |p: &T| {
        let mut binds = HashMap::new();
        unify(p, ty, tps, &mut binds).then_some(binds)
    };
    try_match(param).or_else(|| match param {
        T::Optional(inner, _) => try_match(inner),
        _ => None,
    })
}

/// Match a type pattern (`T[]`, `(fn(I) O)`) against a concrete type (`int[]`), binding the
/// generic parameters `tps`.
fn unify(p: &T, c: &T, tps: &[String], binds: &mut HashMap<String, T>) -> bool {
    if let Some(name) = param(p, tps) {
        return bind(name, c, binds);
    }
    let all = |ps: &[T], cs: &[T], binds: &mut HashMap<String, T>| {
        ps.len() == cs.len() && ps.iter().zip(cs).all(|(p, c)| unify(p, c, tps, binds))
    };
    match (p, c) {
        (
            T::Named {
                module: pm,
                name: pn,
                args: pa,
                ..
            },
            T::Named {
                module: cm,
                name: cn,
                args: ca,
                ..
            },
        ) => pm == cm && pn == cn && all(pa, ca, binds),
        (T::Array(pe, pn, _), T::Array(ce, cn, _)) => {
            let size = match (pn, cn) {
                (None, None) => true,
                (Some(p), Some(c)) => match const_param(p, tps) {
                    Some(n) => bind(n, &T::Const(c.clone()), binds),
                    None => p.show() == c.show(),
                },
                _ => false,
            };
            size && unify(pe, ce, tps, binds)
        }
        (T::Map(pk, pv, _), T::Map(ck, cv, _)) => {
            unify(pk, ck, tps, binds) && unify(pv, cv, tps, binds)
        }
        (T::Optional(p, _), T::Optional(c, _))
        | (T::ErrorUnion(p, _), T::ErrorUnion(c, _))
        | (T::Fut(p, _), T::Fut(c, _)) => unify(p, c, tps, binds),
        (T::Tuple(ps, _), T::Tuple(cs, _)) => all(ps, cs, binds),
        (T::Fn(pp, pr, _), T::Fn(cp, cr, _)) => {
            all(pp, cp, binds)
                && match (pr, cr) {
                    (None, None) => true,
                    (Some(p), Some(c)) => unify(p, c, tps, binds),
                    _ => false,
                }
        }
        (T::Const(p), T::Const(c)) => p.show() == c.show(),
        _ => false,
    }
}

/// Bind the generic parameter `name` to `c`, or check that it is bound to it already.
fn bind(name: &str, c: &T, binds: &mut HashMap<String, T>) -> bool {
    match binds.get(name) {
        Some(b) => same(b, c),
        None => {
            binds.insert(name.to_string(), c.clone());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_type_text as ty;

    #[test]
    fn generic_parameters_and_elements() {
        let g = ["I".to_string(), "O".to_string()];
        assert!(mentions(&ty("I[]").unwrap(), &g) && !mentions(&ty("int[]").unwrap(), &g));
        let elem_of = |t: &str| elem(&ty(t).unwrap()).map(T::show);
        assert_eq!(elem_of("P[]").as_deref(), Some("P"));
        assert_eq!(elem_of("(fn() int)[3]").as_deref(), Some("(fn() int)"));
        assert_eq!(elem_of("int[][]").as_deref(), Some("int[]"));
        assert_eq!(elem_of("[str]int"), None);
        let binds = HashMap::from([("T".to_string(), ty("int").unwrap())]);
        assert_eq!(
            substitute(&ty("(fn(T) T[])").unwrap(), &binds).show(),
            "(fn(int) int[])"
        );
    }

    #[test]
    fn first_argument_types() {
        let b = |p: &str, t: &str, tps: &[&str]| {
            let tps: Vec<String> = tps.iter().map(ToString::to_string).collect();
            accepts(&ty(p).unwrap(), &ty(t).unwrap(), &tps).map(|m| {
                let mut v: Vec<(String, String)> =
                    m.into_iter().map(|(k, t)| (k, t.show())).collect();
                v.sort();
                v
            })
        };
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(b("int", "int", &[]), Some(vec![]));
        assert_eq!(b("int", "uint", &[]), None);
        assert_eq!(b("int", "int[]", &[]), None);
        assert_eq!(b("T[]", "str[]", &["T"]), Some(vec![pair("T", "str")]));
        assert_eq!(
            b("T[]", "(int, str)[]", &["T"]),
            Some(vec![pair("T", "(int, str)")])
        );
        assert_eq!(b("T[]", "int", &["T"]), None);
        assert_eq!(
            b("T[N]", "int[3]", &["T", "N"]),
            Some(vec![pair("N", "3"), pair("T", "int")])
        );
        // a value for an optional, but not a fixed-size array for a dynamic one
        assert_eq!(b("str?", "str", &[]), Some(vec![]));
        assert_eq!(b("int[]", "int[3]", &[]), None);
        assert_eq!(
            b("[K]V", "[str]int[]", &["K", "V"]),
            Some(vec![pair("K", "str"), pair("V", "int[]")])
        );
        assert_eq!(b("Pair<T, T>", "Pair<int, str>", &["T"]), None);
        assert_eq!(b("Vec", "Vec2", &[]), None);
    }
}
