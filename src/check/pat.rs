//! Pattern checking and exhaustiveness.

use super::*;
use crate::ast::ExprKind as E;

impl Checker {
    /// A pattern matching values of type `ty`. Anything that is not a binding or the shape of
    /// a tuple, array, enum variant or struct is a value, compared with `==`.
    pub fn check_pattern(&mut self, p: &ast::Expr, ty: TyId, multi: bool) -> Result<Pat> {
        let span = p.span;
        match &p.kind {
            E::Wildcard => return Ok(Pat::Wild),
            // an existing name is compared; otherwise the identifier binds a new variable
            E::Ident(name)
                if self.resolve_var(name, span)?.is_none() && !self.is_value_symbol(name) =>
            {
                if multi {
                    return err(
                        span,
                        "patterns with several alternatives cannot bind variables",
                    );
                }
                let l = self.declare_local(name, ty, false, LocalKind::Normal, span);
                self.idx_type_hint(span, ty);
                return Ok(Pat::Bind(l));
            }
            E::Tuple(items) => {
                if let TyKind::Tuple(ts) = self.types.kind(ty).clone() {
                    if ts.len() != items.len() {
                        return err(
                            span,
                            format!(
                                "expected a tuple of {} elements, found {}",
                                ts.len(),
                                items.len()
                            ),
                        );
                    }
                    return Ok(Pat::Tuple(self.check_patterns(items, ts, multi)?));
                }
            }
            E::Array(items) => {
                if let TyKind::Array(e) | TyKind::Fixed(e, _) = *self.types.kind(ty) {
                    if let TyKind::Fixed(_, n) = *self.types.kind(ty)
                        && n as usize != items.len()
                    {
                        return err(
                            span,
                            format!(
                                "expected {n} elements to match `{}`, found {}",
                                self.types.show(ty),
                                items.len()
                            ),
                        );
                    }
                    let ts = vec![e; items.len()];
                    return Ok(Pat::Array(self.check_patterns(items, ts, multi)?));
                }
            }
            E::Call(callee, args) => {
                if let E::Field(base, vname) = &callee.kind
                    && let Some(ety) = self.type_path(base)?
                {
                    let idx = self.variant_of(ety, ty, vname, span)?;
                    let payload = self.types.enum_def(ety).unwrap().variants[idx].1.clone();
                    if payload.len() != args.len() {
                        return err(
                            span,
                            format!(
                                "`{vname}` holds {} value(s), found {}",
                                payload.len(),
                                args.len()
                            ),
                        );
                    }
                    let ps = self.check_patterns(args, payload, multi)?;
                    return Ok(Pat::Variant(idx, ps));
                }
            }
            E::Field(base, vname) => {
                if let Some(ety) = self.type_path(base)? {
                    let idx = self.variant_of(ety, ty, vname, span)?;
                    let payload = &self.types.enum_def(ety).unwrap().variants[idx].1;
                    if !payload.is_empty() {
                        return err(
                            span,
                            format!("`{vname}` holds a value; match it with `{vname}(...)`"),
                        );
                    }
                    return Ok(Pat::Variant(idx, vec![]));
                }
            }
            E::Struct(path, fields) => {
                let Some(sty) = self.type_path(path)? else {
                    return err(path.span, "expected a struct type");
                };
                self.same_type(ty, sty, span)?;
                let out = self.struct_fields(
                    sty,
                    fields,
                    "appears more than once",
                    (codes::MISSING_PATTERN_FIELD, |name| {
                        format!("missing field `{name}` in pattern; use `.{name} = _` to ignore it")
                    }),
                    span,
                    |c, v, t| c.check_pattern(v, t, multi),
                )?;
                return Ok(Pat::Struct(out));
            }
            _ => {}
        }
        let v = self.expect_expr(p, ty)?;
        self.require_plain(ty, span, "values cannot be compared")?;
        Ok(Pat::Value(v))
    }

    /// Patterns for `items`, matching values of the types `tys` in order.
    fn check_patterns(
        &mut self,
        items: &[ast::Expr],
        tys: Vec<TyId>,
        multi: bool,
    ) -> Result<Vec<Pat>> {
        (items.iter().zip(tys))
            .map(|(it, t)| self.check_pattern(it, t, multi))
            .collect()
    }

    fn is_value_symbol(&self, name: &str) -> bool {
        let m = self.module();
        matches!(
            self.mods[m].syms.get(name).map(|e| &e.sym),
            Some(Sym::Fn { .. })
        )
    }

    /// An error unless a pattern of type `found` matches values of type `expected`.
    fn same_type(&self, expected: TyId, found: TyId, span: Span) -> Result<()> {
        if expected == found {
            return Ok(());
        }
        let (e, f) = (self.types.show(expected), self.types.show(found));
        err(span, format!("expected `{e}`, found `{f}`"))
    }

    fn variant_of(&self, ety: TyId, ty: TyId, vname: &str, span: Span) -> Result<usize> {
        self.same_type(ty, ety, span)?;
        self.enum_variant(ety, vname, span)
    }

    /// The index of the variant `vname` of the enum `ety`.
    pub fn enum_variant(&self, ety: TyId, vname: &str, span: Span) -> Result<usize> {
        let Some(def) = self.types.enum_def(ety) else {
            return err(span, format!("`{}` is not an enum", self.types.show(ety)));
        };
        def.variants
            .iter()
            .position(|(n, _)| n == vname)
            .ok_or_else(|| Diag::new(span, format!("enum `{}` has no member `{vname}`", def.name)))
    }

    // ------------------------------------------------------------ exhaustiveness

    pub fn exhaustive(&self, arms: &[Arm], ty: TyId) -> bool {
        let rows: Vec<Vec<Pat>> = arms
            .iter()
            .flat_map(|a| a.patterns.iter().map(|p| vec![p.clone()]))
            .collect();
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
        let default: Vec<Vec<Pat>> = rows
            .iter()
            .filter(|r| Self::is_wild(&r[0]))
            .map(|r| r[1..].to_vec())
            .collect();
        if default.len() == rows.len() {
            // no constructor in this column: it is irrelevant
            return self.matrix_exhaustive(&default, rest);
        }
        let is_lit =
            |p: &Pat, lit: &Lit| matches!(p, Pat::Value(Expr { kind: EK::Lit(x), .. }) if x == lit);
        // the rows matching the literal `lit` (or anything) in the first column
        let lit_rows = |lit: Lit| -> Vec<Vec<Pat>> {
            (rows.iter())
                .filter(|r| Self::is_wild(&r[0]) || is_lit(&r[0], &lit))
                .map(|r| r[1..].to_vec())
                .collect()
        };
        match self.types.kind(t).clone() {
            TyKind::Bool => [true, false]
                .into_iter()
                .all(|b| self.matrix_exhaustive(&lit_rows(Lit::Bool(b)), rest)),
            // all the values are listed
            TyKind::Byte
                if (0..=255).all(|b| rows.iter().any(|r| is_lit(&r[0], &Lit::Byte(b)))) =>
            {
                (0..=255).all(|b| self.matrix_exhaustive(&lit_rows(Lit::Byte(b)), rest))
            }
            TyKind::Enum(i) => {
                let variants = self.types.enums[i as usize].variants.clone();
                variants.iter().enumerate().all(|(vi, (_, payload))| {
                    self.expand_product(rows, payload, rest, |p| match p {
                        Pat::Variant(x, ps) if *x == vi => Some(ps.clone()),
                        _ => None,
                    })
                })
            }
            TyKind::Tuple(ts) => self.expand_product(rows, &ts, rest, |p| match p {
                Pat::Tuple(ps) => Some(ps.clone()),
                _ => None,
            }),
            TyKind::Struct(i) => {
                let ts: Vec<TyId> = (self.types.structs[i as usize].fields.iter())
                    .map(|f| f.1)
                    .collect();
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

    /// The rows whose first column matches the product `ts` (a wildcard, or a pattern `get`
    /// splits into its parts), with that column expanded into the parts.
    fn expand_product(
        &self,
        rows: &[Vec<Pat>],
        ts: &[TyId],
        rest: &[TyId],
        get: impl Fn(&Pat) -> Option<Vec<Pat>>,
    ) -> bool {
        let sub: Vec<Vec<Pat>> = (rows.iter())
            .filter_map(|r| {
                let mut row = match &r[0] {
                    p if Self::is_wild(p) => vec![Pat::Wild; ts.len()],
                    p => get(p)?,
                };
                row.extend_from_slice(&r[1..]);
                Some(row)
            })
            .collect();
        let tys: Vec<TyId> = ts.iter().chain(rest).copied().collect();
        self.matrix_exhaustive(&sub, &tys)
    }
}
