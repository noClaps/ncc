//! Pattern checking and exhaustiveness.

use super::*;
use crate::ast::ExprKind as E;

impl<'a> Checker<'a> {
    pub fn check_pattern(&mut self, p: &ast::Expr, ty: TyId, multi: bool) -> Result<Pat> {
        let span = p.span;
        match &p.kind {
            E::Wildcard => Ok(Pat::Wild),
            E::Ident(name) => {
                // an existing name is compared; otherwise the identifier binds a new variable
                if self.resolve_var(name, span)?.is_some() || self.is_value_symbol(name) {
                    let v = self.expect_expr(p, ty)?;
                    return self.value_pat(v, ty, span);
                }
                if multi {
                    return err(span, "patterns with several alternatives cannot bind variables");
                }
                let l = self.declare_local(name, ty, false, LocalKind::Normal, span)?;
                Ok(Pat::Bind(l))
            }
            E::Tuple(items) => {
                if let TyKind::Tuple(ts) = self.types.kind(ty).clone() {
                    if ts.len() != items.len() {
                        return err(span, format!("expected a tuple of {} elements, found {}", ts.len(), items.len()));
                    }
                    let mut out = Vec::new();
                    for (it, t) in items.iter().zip(ts) {
                        out.push(self.check_pattern(it, t, multi)?);
                    }
                    return Ok(Pat::Tuple(out));
                }
                let v = self.expect_expr(p, ty)?;
                self.value_pat(v, ty, span)
            }
            E::Array(items) => match self.types.kind(ty).clone() {
                TyKind::Array(e) => {
                    let mut out = Vec::new();
                    for it in items {
                        out.push(self.check_pattern(it, e, multi)?);
                    }
                    Ok(Pat::Array(out))
                }
                TyKind::Fixed(e, n) => {
                    if n as usize != items.len() {
                        return err(span, format!("expected {n} elements to match `{}`, found {}", self.types.show(ty), items.len()));
                    }
                    let mut out = Vec::new();
                    for it in items {
                        out.push(self.check_pattern(it, e, multi)?);
                    }
                    Ok(Pat::Array(out))
                }
                _ => {
                    let v = self.expect_expr(p, ty)?;
                    self.value_pat(v, ty, span)
                }
            },
            E::Call(callee, args) => {
                if let E::Field(base, vname) = &callee.kind
                    && let Some(ety) = self.type_path(base)? {
                        let idx = self.variant_of(ety, ty, vname, span)?;
                        let payload = self.types.enum_def(ety).unwrap().variants[idx].1.clone();
                        if payload.len() != args.len() {
                            return err(span, format!("`{vname}` holds {} value(s), found {}", payload.len(), args.len()));
                        }
                        let mut out = Vec::new();
                        for (a, t) in args.iter().zip(payload) {
                            out.push(self.check_pattern(a, t, multi)?);
                        }
                        return Ok(Pat::Variant(idx, out));
                    }
                let v = self.expect_expr(p, ty)?;
                self.value_pat(v, ty, span)
            }
            E::Field(base, vname) => {
                if let Some(ety) = self.type_path(base)? {
                    let idx = self.variant_of(ety, ty, vname, span)?;
                    let payload = &self.types.enum_def(ety).unwrap().variants[idx].1;
                    if !payload.is_empty() {
                        return err(span, format!("`{vname}` holds a value; match it with `{vname}(...)`"));
                    }
                    return Ok(Pat::Variant(idx, vec![]));
                }
                let v = self.expect_expr(p, ty)?;
                self.value_pat(v, ty, span)
            }
            E::Struct(path, fields) => {
                let Some(sty) = self.type_path(path)? else {
                    return err(path.span, "expected a struct type");
                };
                if sty != ty {
                    return err(span, format!("expected `{}`, found `{}`", self.types.show(ty), self.types.show(sty)));
                }
                let def = self.types.struct_def(sty).cloned().unwrap();
                let mut pats: Vec<Option<Pat>> = vec![None; def.fields.len()];
                for (name, v, fsp) in fields {
                    let Some(i) = def.fields.iter().position(|(n, _)| n == name) else {
                        return err(*fsp, format!("`{}` has no field `{name}`", def.name));
                    };
                    if pats[i].is_some() {
                        return err(*fsp, format!("field `{name}` appears more than once"));
                    }
                    pats[i] = Some(self.check_pattern(v, def.fields[i].1, multi)?);
                }
                let mut out = Vec::new();
                for (i, p) in pats.into_iter().enumerate() {
                    match p {
                        Some(p) => out.push(p),
                        None => {
                            return err(
                                span,
                                format!("missing field `{}` in pattern; use `.{} = _` to ignore it", def.fields[i].0, def.fields[i].0),
                            )
                        }
                    }
                }
                Ok(Pat::Struct(out))
            }
            _ => {
                let v = self.expect_expr(p, ty)?;
                self.value_pat(v, ty, span)
            }
        }
    }

    fn is_value_symbol(&self, name: &str) -> bool {
        let m = self.module();
        matches!(self.mods[m].syms.get(name).map(|e| &e.sym), Some(Sym::Fn { .. }))
    }

    fn variant_of(&self, ety: TyId, ty: TyId, vname: &str, span: Span) -> Result<usize> {
        if ety != ty {
            return err(span, format!("expected `{}`, found `{}`", self.types.show(ty), self.types.show(ety)));
        }
        let Some(def) = self.types.enum_def(ety) else {
            return err(span, format!("`{}` is not an enum", self.types.show(ety)));
        };
        def.variants
            .iter()
            .position(|(n, _)| n == vname)
            .ok_or_else(|| Diag::new(span, format!("enum `{}` has no member `{vname}`", def.name)))
    }

    fn value_pat(&mut self, v: Expr, ty: TyId, span: Span) -> Result<Pat> {
        if !self.comparable(ty) {
            return err(span, format!("`{}` values cannot be compared", self.types.show(ty)));
        }
        Ok(Pat::Value(v))
    }

    // ------------------------------------------------------------ exhaustiveness

    pub fn exhaustive(&self, arms: &[Arm], ty: TyId) -> bool {
        let rows: Vec<Vec<Pat>> = arms.iter().flat_map(|a| a.patterns.iter().map(|p| vec![p.clone()])).collect();
        self.matrix_exhaustive(&rows, &[ty])
    }

    fn is_wild(p: &Pat) -> bool {
        matches!(p, Pat::Wild | Pat::Bind(_))
    }

    fn matrix_exhaustive(&self, rows: &[Vec<Pat>], tys: &[TyId]) -> bool {
        if tys.is_empty() {
            return !rows.is_empty();
        }
        if rows.is_empty() {
            return false;
        }
        let t = tys[0];
        let rest = &tys[1..];
        // rows with a wildcard first column
        let default: Vec<Vec<Pat>> = rows.iter().filter(|r| Self::is_wild(&r[0])).map(|r| r[1..].to_vec()).collect();
        if default.len() == rows.len() {
            // no constructor in this column: it is irrelevant
            return self.matrix_exhaustive(&default, rest);
        }
        match self.types.kind(t).clone() {
            TyKind::Bool => {
                [true, false].iter().all(|b| {
                    let sub: Vec<Vec<Pat>> = rows
                        .iter()
                        .filter(|r| {
                            Self::is_wild(&r[0]) || matches!(&r[0], Pat::Value(Expr { kind: EK::Lit(Lit::Bool(x)), .. }) if x == b)
                        })
                        .map(|r| r[1..].to_vec())
                        .collect();
                    self.matrix_exhaustive(&sub, rest)
                })
            }
            TyKind::Byte => {
                let mut covered = [false; 256];
                for r in rows {
                    if let Pat::Value(Expr { kind: EK::Lit(Lit::Byte(b)), .. }) = &r[0] {
                        covered[*b as usize] = true;
                    }
                }
                if covered.iter().all(|c| *c) {
                    (0..=255u8).all(|b| {
                        let sub: Vec<Vec<Pat>> = rows
                            .iter()
                            .filter(|r| {
                                Self::is_wild(&r[0])
                                    || matches!(&r[0], Pat::Value(Expr { kind: EK::Lit(Lit::Byte(x)), .. }) if *x == b)
                            })
                            .map(|r| r[1..].to_vec())
                            .collect();
                        self.matrix_exhaustive(&sub, rest)
                    })
                } else {
                    self.matrix_exhaustive(&default, rest)
                }
            }
            TyKind::Enum(i) => {
                let variants = self.types.enums[i as usize].variants.clone();
                variants.iter().enumerate().all(|(vi, (_, payload))| {
                    let sub: Vec<Vec<Pat>> = rows
                        .iter()
                        .filter_map(|r| match &r[0] {
                            p if Self::is_wild(p) => {
                                let mut row = vec![Pat::Wild; payload.len()];
                                row.extend(r[1..].iter().cloned());
                                Some(row)
                            }
                            Pat::Variant(x, ps) if *x == vi => {
                                let mut row = ps.clone();
                                row.extend(r[1..].iter().cloned());
                                Some(row)
                            }
                            _ => None,
                        })
                        .collect();
                    let mut tys2 = payload.clone();
                    tys2.extend_from_slice(rest);
                    self.matrix_exhaustive(&sub, &tys2)
                })
            }
            TyKind::Tuple(ts) => self.expand_product(rows, &ts, rest, |p| match p {
                Pat::Tuple(ps) => Some(ps.clone()),
                _ => None,
            }),
            TyKind::Struct(i) => {
                let ts: Vec<TyId> = self.types.structs[i as usize].fields.iter().map(|f| f.1).collect();
                self.expand_product(rows, &ts, rest, |p| match p {
                    Pat::Struct(ps) => Some(ps.clone()),
                    _ => None,
                })
            }
            TyKind::Fixed(e, n) => {
                let ts = vec![e; n as usize];
                self.expand_product(rows, &ts, rest, |p| match p {
                    Pat::Array(ps) => Some(ps.clone()),
                    _ => None,
                })
            }
            _ => self.matrix_exhaustive(&default, rest),
        }
    }

    fn expand_product(&self, rows: &[Vec<Pat>], ts: &[TyId], rest: &[TyId], get: impl Fn(&Pat) -> Option<Vec<Pat>>) -> bool {
        let sub: Vec<Vec<Pat>> = rows
            .iter()
            .filter_map(|r| {
                if Self::is_wild(&r[0]) {
                    let mut row = vec![Pat::Wild; ts.len()];
                    row.extend(r[1..].iter().cloned());
                    Some(row)
                } else {
                    get(&r[0]).map(|ps| {
                        let mut row = ps;
                        row.extend(r[1..].iter().cloned());
                        row
                    })
                }
            })
            .collect();
        let mut tys2 = ts.to_vec();
        tys2.extend_from_slice(rest);
        self.matrix_exhaustive(&sub, &tys2)
    }
}
