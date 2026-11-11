//! Recursive-descent parser producing [`ast::Module`].

use crate::ast::*;
use crate::diag::{Diag, Result, Span, err};
use crate::lexer::{StrPart, Tok, Token, lex};

pub const KEYWORDS: &[&str] = &[
    "fn", "return", "if", "else", "while", "for", "in", "break", "continue", "mut", "mutex",
    "lock", "struct", "enum", "type", "import", "as", "extern", "pub", "test", "assert", "true",
    "false", "none", "and", "or", "not", "try", "catch", "throw", "async", "await", "fut",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    file: u32,
    /// When set, `Name {` is not parsed as a struct literal unless followed by `.`
    no_struct: bool,
    /// errors recovered from so far
    errors: Vec<Diag>,
    /// source text (for indentation-aware error recovery); empty for sub-parsers
    src: String,
    /// token index where the current statement started
    stmt_start: usize,
    /// set when a statement recovered from an error itself
    recovered: bool,
}

/// Parse a module, recovering from errors at statement boundaries. Returns the items that
/// parsed and every error found.
pub fn parse_module_recover(src: &str, file: u32) -> (Module, Vec<Diag>) {
    let (toks, lex_errors) = crate::lexer::lex_all(src, file, 0);
    let mut p = Parser::new(toks, file, src.to_string());
    p.errors = lex_errors;
    let m = p.module();
    let mut errors = p.errors;
    errors.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
    (m, errors)
}

impl Parser {
    fn new(toks: Vec<Token>, file: u32, src: String) -> Self {
        Self {
            toks,
            pos: 0,
            file,
            no_struct: false,
            errors: vec![],
            src,
            stmt_start: 0,
            recovered: false,
        }
    }

    /// Run `f` with struct literals allowed (`no_struct` false) or not, restoring the setting.
    fn with_no_struct<T>(&mut self, no_struct: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::replace(&mut self.no_struct, no_struct);
        let r = f(self);
        self.no_struct = saved;
        r
    }

    /// Comma-separated items up to `close` (consumed), with newlines allowed around them and
    /// an optional trailing comma.
    fn list<T>(
        &mut self,
        close: &Tok,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        self.skip_nl();
        while !self.at(close) {
            out.push(item(self)?);
            self.skip_nl();
            if !self.eat(&Tok::Comma) {
                break;
            }
            self.skip_nl();
        }
        self.expect(close)?;
        Ok(out)
    }

    /// `<A, 3>` after a type or function name: generic arguments.
    fn generic_args(&mut self) -> Result<Vec<TypeExpr>> {
        self.expect(&Tok::Lt)?;
        let mut args = vec![self.generic_arg()?];
        while self.eat(&Tok::Comma) {
            args.push(self.generic_arg()?);
        }
        self.expect_gt()?;
        Ok(args)
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_n(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }
    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok) -> Result<Span> {
        if self.at(t) {
            Ok(self.bump().span)
        } else {
            err(
                self.span(),
                format!(
                    "expected {}, found {}",
                    t.describe(),
                    self.peek().describe()
                ),
            )
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<()> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            err(
                self.span(),
                format!("expected `{kw}`, found {}", self.peek().describe()),
            )
        }
    }
    /// Expect `>`, splitting `>>` / `>=` tokens when needed.
    fn expect_gt(&mut self) -> Result<()> {
        match self.peek().clone() {
            Tok::Gt => {
                self.bump();
                Ok(())
            }
            // keep what follows the `>`: `>>` -> `>`, `>=` -> `=`
            t @ (Tok::Shr | Tok::Ge) => {
                let tok = &mut self.toks[self.pos];
                tok.tok = if t == Tok::Shr { Tok::Gt } else { Tok::Assign };
                tok.span.start += 1;
                Ok(())
            }
            t => err(self.span(), format!("expected `>`, found {}", t.describe())),
        }
    }
    fn ident(&mut self) -> Result<(String, Span)> {
        match self.peek().clone() {
            Tok::Ident(s) if !is_keyword(&s) => {
                let sp = self.bump().span;
                Ok((s, sp))
            }
            t => err(
                self.span(),
                format!("expected identifier, found {}", t.describe()),
            ),
        }
    }
    fn skip_nl(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }
    /// Parse a declaration's value. On error the error is recorded and the statement is
    /// skipped, but the declaration is kept (with an `Error` value) so that its variables
    /// still exist and later uses don't report follow-up errors.
    fn decl_value(&mut self) -> Expr {
        let at = self.span();
        match self.expr() {
            Ok(e) => e,
            Err(d) => {
                let start = self.stmt_start;
                self.recover(d, start);
                self.recovered = true;
                Expr {
                    kind: ExprKind::Error,
                    span: at,
                }
            }
        }
    }

    fn end_stmt(&mut self) -> Result<()> {
        if std::mem::take(&mut self.recovered) {
            // the statement already skipped to the start of the next one
            return Ok(());
        }
        match self.peek() {
            Tok::Newline => {
                self.skip_nl();
                Ok(())
            }
            Tok::RBrace | Tok::Eof => Ok(()),
            t => err(
                self.span(),
                format!("expected end of statement, found {}", t.describe()),
            ),
        }
    }

    // ---------------------------------------------------------------- items

    fn module(&mut self) -> Module {
        let mut items = Vec::new();
        self.skip_nl();
        while !self.at(&Tok::Eof) {
            let start = self.pos;
            let r = self.item(&mut items).and_then(|_| self.end_stmt());
            if let Err(d) = r {
                self.recover(d, start);
            }
        }
        Module { items }
    }

    /// Column (in bytes) of a token, or None when the source is unavailable.
    fn column(&self, tok: usize) -> Option<usize> {
        if self.src.is_empty() {
            return None;
        }
        let off = (self.toks[tok].span.start as usize).min(self.src.len());
        Some(off - self.src[..off].rfind('\n').map_or(0, |i| i + 1))
    }

    /// Brackets opened between `from` and `to` (token indices) that are still unclosed.
    fn open_brackets(&self, from: usize, to: usize) -> Vec<usize> {
        let mut open = Vec::new();
        for i in from..to {
            match self.toks[i].tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => open.push(i),
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    open.pop();
                }
                _ => {}
            }
        }
        open
    }

    /// Skip to the end of the line, past brackets opened on it (`depth` were already open).
    fn skip_line(&mut self, mut depth: i32) {
        loop {
            match self.peek() {
                Tok::Eof => break,
                Tok::Newline if depth <= 0 => {
                    self.skip_nl();
                    break;
                }
                Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RBrace if depth <= 0 => break,
                Tok::RBrace | Tok::RParen | Tok::RBracket => depth -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    /// Record an error and skip to the start of the next statement.
    fn recover(&mut self, d: Diag, start: usize) {
        let err_tok = self.pos.min(self.toks.len() - 1);
        let open = self.open_brackets(start, err_tok);
        // If the statement broke on a later line (e.g. an unclosed bracket swallowed the
        // newline) and that line is indented no deeper than the statement itself, the line is
        // most likely a new statement: resume there instead of skipping it.
        let line_start = (start + 1..=err_tok)
            .rev()
            .find(|&i| self.toks[i - 1].tok == Tok::Newline && self.toks[i].tok != Tok::Newline);
        if let (Some(i), Some(sc)) = (line_start, self.column(start))
            && self.column(i).is_some_and(|c| c <= sc)
        {
            // report the bracket that was left open rather than the token after it
            let d = match open.iter().rev().find(|&&b| b < i) {
                Some(&b) => {
                    let what = match self.toks[b].tok {
                        Tok::LParen => "(",
                        Tok::LBracket => "[",
                        _ => "{",
                    };
                    Diag::new(self.toks[b].span, format!("this `{what}` is never closed"))
                }
                None => d,
            };
            self.errors.push(d);
            self.pos = i;
            return;
        }
        self.errors.push(d);
        // skip to the end of the statement, including brackets it already opened
        self.skip_line(open.len() as i32);
        if self.pos == start && !self.at(&Tok::Eof) {
            self.bump();
            self.skip_nl();
        }
        // lines indented deeper than the broken statement belong to it
        if let Some(sc) = self.column(start) {
            while !matches!(self.peek(), Tok::Eof | Tok::RBrace)
                && self.column(self.pos).is_some_and(|c| c > sc)
            {
                self.skip_line(0);
            }
        }
    }

    fn item(&mut self, items: &mut Vec<Item>) -> Result<()> {
        self.stmt_start = self.pos;
        let start = self.span();
        let public = self.eat_kw("pub");
        if self.at_kw("import") {
            if public {
                return err(start, "imports cannot be `pub`");
            }
            self.bump();
            self.expect(&Tok::LBrace)?;
            self.skip_nl();
            while !self.at(&Tok::RBrace) {
                let sp = self.span();
                let path = self.plain_string("import path")?;
                self.expect_kw("as")?;
                let (alias, asp) = self.ident()?;
                items.push(Item::Import(Import {
                    path,
                    alias,
                    span: sp.to(asp),
                }));
                self.eat(&Tok::Comma);
                self.skip_nl();
            }
            self.bump();
            return Ok(());
        }
        if self.at_kw("extern") {
            if public {
                return err(start, "extern blocks cannot be `pub`");
            }
            self.bump();
            let path = self.plain_string("extern path")?;
            self.expect_kw("as")?;
            let (alias, _) = self.ident()?;
            self.expect(&Tok::LBrace)?;
            self.skip_nl();
            let mut fns = Vec::new();
            while !self.at(&Tok::RBrace) {
                if self.at_kw("pub") {
                    return err(self.span(), "functions in extern blocks cannot be `pub`");
                }
                self.expect_kw("fn")?;
                let sig = self.fn_sig(true)?;
                self.expect(&Tok::Assign)?;
                let symbol = self.plain_string("extern symbol")?;
                fns.push(ExternFn { sig, symbol });
                self.end_stmt()?;
            }
            let end = self.bump().span;
            items.push(Item::Extern(ExternDecl {
                path,
                alias,
                fns,
                span: start.to(end),
            }));
            return Ok(());
        }
        if self.at_kw("struct") {
            self.bump();
            let (name, _) = self.ident()?;
            let type_params = self.type_params()?;
            self.expect(&Tok::LBrace)?;
            self.skip_nl();
            let mut fields = Vec::new();
            while !self.at(&Tok::RBrace) {
                let ty = self.parse_type()?;
                let (fname, fsp) = self.ident()?;
                fields.push((ty, fname, fsp));
                self.eat(&Tok::Comma);
                self.end_stmt()?;
            }
            let end = self.bump().span;
            items.push(Item::Struct(StructDecl {
                name,
                type_params,
                fields,
                public,
                span: start.to(end),
            }));
            return Ok(());
        }
        if self.at_kw("enum") {
            self.bump();
            let (name, _) = self.ident()?;
            let type_params = self.type_params()?;
            self.expect(&Tok::LBrace)?;
            self.skip_nl();
            let mut variants = Vec::new();
            while !self.at(&Tok::RBrace) {
                let (vname, vsp) = self.ident()?;
                let tys = if self.eat(&Tok::LParen) {
                    self.list(&Tok::RParen, Self::parse_type)?
                } else {
                    vec![]
                };
                variants.push((vname, tys, vsp));
                self.eat(&Tok::Comma);
                self.end_stmt()?;
            }
            let end = self.bump().span;
            items.push(Item::Enum(EnumDecl {
                name,
                type_params,
                variants,
                public,
                span: start.to(end),
            }));
            return Ok(());
        }
        if self.at_kw("type") {
            self.bump();
            let (name, _) = self.ident()?;
            self.expect(&Tok::Assign)?;
            let ty = self.parse_type()?;
            items.push(Item::Type(TypeDecl {
                name,
                ty,
                public,
                span: start.to(self.prev_span()),
            }));
            return Ok(());
        }
        if self.at_kw("fn")
            && matches!(self.peek_n(1), Tok::Ident(_))
            && !matches!(self.peek_n(2), Tok::Assign)
        {
            self.bump();
            let sig = self.fn_sig(false)?;
            let body = self.block()?;
            items.push(Item::Fn(FnDecl { sig, body, public }));
            return Ok(());
        }
        if self.at_kw("test") {
            if public {
                return err(start, "tests cannot be `pub`");
            }
            self.bump();
            let name = self.plain_string("test name")?;
            let body = self.block()?;
            items.push(Item::Test(name, body, start.to(self.prev_span())));
            return Ok(());
        }
        let stmt = self.stmt(public)?;
        if public && !matches!(stmt.kind, StmtKind::VarDecl { .. }) {
            return err(start, "only declarations can be `pub`");
        }
        items.push(Item::Stmt(stmt));
        Ok(())
    }

    fn plain_string(&mut self, what: &str) -> Result<String> {
        let sp = self.span();
        match self.bump().tok {
            Tok::Str(parts) => match parts.as_slice() {
                [StrPart::Lit(s)] => Ok(s.clone()),
                _ => err(sp, format!("{what} must be a plain string literal")),
            },
            t => err(
                sp,
                format!("expected {what} string, found {}", t.describe()),
            ),
        }
    }

    /// `<type T, uint N>`
    fn type_params(&mut self) -> Result<Vec<GenericParam>> {
        let mut out = Vec::new();
        if self.eat(&Tok::Lt) {
            loop {
                let start = self.span();
                let konst = match self.peek().clone() {
                    Tok::Ident(k) if k == "type" => {
                        self.bump();
                        None
                    }
                    Tok::Ident(k) if matches!(k.as_str(), "int" | "uint" | "byte") => {
                        self.bump();
                        Some(k)
                    }
                    t => {
                        return err(
                            start,
                            format!(
                                "expected `type` or a constant's type (`int`, `uint` or `byte`) \
                                 before a generic parameter, found {}",
                                t.describe()
                            ),
                        );
                    }
                };
                let (name, span) = self.ident()?;
                out.push(GenericParam { name, konst, span });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect_gt()?;
        }
        Ok(out)
    }

    /// One argument in a generic argument list: a type, or a constant such as `3` or `N - 1`.
    /// A bare name (`N`) parses as a type; the checker tells constants and types apart.
    fn generic_arg(&mut self) -> Result<TypeExpr> {
        let save = self.pos;
        if let Ok(c) = self.const_expr()
            && !matches!(c, ConstExpr::Name(..))
            && matches!(self.peek(), Tok::Comma | Tok::Gt | Tok::Shr | Tok::Ge)
        {
            return Ok(TypeExpr::Const(c));
        }
        self.pos = save;
        self.parse_type()
    }

    /// A compile-time integer: literals, generic constants, `+ - * / %` and parentheses.
    fn const_expr(&mut self) -> Result<ConstExpr> {
        self.const_binary(Self::const_term, |t| match t {
            Tok::Plus => Some(BinOp::Add),
            Tok::Minus => Some(BinOp::Sub),
            _ => None,
        })
    }

    fn const_term(&mut self) -> Result<ConstExpr> {
        self.const_binary(Self::const_atom, |t| match t {
            Tok::Star => Some(BinOp::Mul),
            Tok::Slash => Some(BinOp::Div),
            Tok::Percent => Some(BinOp::Mod),
            _ => None,
        })
    }

    /// Left-associative operators `op` between operands parsed by `operand`.
    fn const_binary(
        &mut self,
        operand: fn(&mut Self) -> Result<ConstExpr>,
        op: fn(&Tok) -> Option<BinOp>,
    ) -> Result<ConstExpr> {
        let mut l = operand(self)?;
        while let Some(o) = op(self.peek()) {
            self.bump();
            let r = operand(self)?;
            l = ConstExpr::Bin(o, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn const_atom(&mut self) -> Result<ConstExpr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Int(n, _) => {
                self.bump();
                Ok(ConstExpr::Int(n, start))
            }
            Tok::Ident(n) if !is_keyword(&n) => {
                self.bump();
                Ok(ConstExpr::Name(n, start))
            }
            Tok::Minus => {
                self.bump();
                let e = self.const_atom()?;
                Ok(ConstExpr::Neg(Box::new(e), start.to(self.prev_span())))
            }
            Tok::LParen => {
                self.bump();
                let e = self.const_expr()?;
                self.expect(&Tok::RParen)?;
                Ok(ConstExpr::Paren(Box::new(e), start.to(self.prev_span())))
            }
            t => err(
                start,
                format!("expected a constant, found {}", t.describe()),
            ),
        }
    }

    /// After `fn`: `name<type T>(params) ret`
    fn fn_sig(&mut self, is_extern: bool) -> Result<FnSig> {
        let (name, sp) = self.ident()?;
        let type_params = if is_extern {
            vec![]
        } else {
            self.type_params()?
        };
        let (params, ret) = self.params_and_ret()?;
        Ok(FnSig {
            name,
            type_params,
            params,
            ret,
            span: sp,
        })
    }

    fn params_and_ret(&mut self) -> Result<(Vec<Param>, Option<TypeExpr>)> {
        self.expect(&Tok::LParen)?;
        let params = self.list(&Tok::RParen, |p| {
            let ty = p.parse_type()?;
            let (name, span) = p.ident()?;
            Ok(Param { ty, name, span })
        })?;
        let ret = self.ret_type()?;
        Ok((params, ret))
    }

    fn ret_type(&mut self) -> Result<Option<TypeExpr>> {
        match self.peek() {
            Tok::LBrace | Tok::Assign | Tok::Newline | Tok::RParen | Tok::Eof => Ok(None),
            Tok::Bang => {
                let sp = self.bump().span;
                let void = TypeExpr::Named {
                    module: None,
                    name: "void".into(),
                    args: vec![],
                    span: sp,
                };
                Ok(Some(TypeExpr::ErrorUnion(Box::new(void), sp)))
            }
            _ => Ok(Some(self.parse_type()?)),
        }
    }

    // ---------------------------------------------------------------- types

    pub fn parse_type(&mut self) -> Result<TypeExpr> {
        let start = self.span();
        let ty = match self.peek().clone() {
            Tok::Ident(s) if s == "fut" => {
                self.bump();
                let inner = self.parse_type()?;
                return Ok(TypeExpr::Fut(Box::new(inner), start.to(self.prev_span())));
            }
            Tok::LBracket => {
                self.bump();
                let k = self.parse_type()?;
                self.expect(&Tok::RBracket)?;
                // the value type binds all postfix sigils: `[str]int[]` is a map of int arrays
                let v = self.parse_type()?;
                TypeExpr::Map(Box::new(k), Box::new(v), start.to(self.prev_span()))
            }
            _ => self.parse_type_atom()?,
        };
        Ok(self.type_postfix(ty, start))
    }

    fn type_postfix(&mut self, mut ty: TypeExpr, start: Span) -> TypeExpr {
        loop {
            match self.peek() {
                Tok::LBracket => {
                    if self.peek_n(1).clone() == Tok::RBracket {
                        self.bump();
                        self.bump();
                        ty = TypeExpr::Array(Box::new(ty), None, start.to(self.prev_span()));
                    } else {
                        // `T[3]`, `T[N]`, `T[N - 1]`
                        let save = self.pos;
                        self.bump();
                        match self.const_expr() {
                            Ok(n) if self.eat(&Tok::RBracket) => {
                                ty = TypeExpr::Array(
                                    Box::new(ty),
                                    Some(n),
                                    start.to(self.prev_span()),
                                );
                            }
                            _ => {
                                self.pos = save;
                                break;
                            }
                        }
                    }
                }
                Tok::Question => {
                    self.bump();
                    ty = TypeExpr::Optional(Box::new(ty), start.to(self.prev_span()));
                }
                Tok::Bang => {
                    self.bump();
                    ty = TypeExpr::ErrorUnion(Box::new(ty), start.to(self.prev_span()));
                }
                _ => break,
            }
        }
        ty
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::LParen => {
                self.bump();
                if self.at_kw("fn") {
                    self.bump();
                    self.expect(&Tok::LParen)?;
                    let params = self.list(&Tok::RParen, |p| {
                        let ty = p.parse_type()?;
                        // optional parameter name
                        if let Tok::Ident(s) = p.peek()
                            && !is_keyword(s)
                        {
                            p.bump();
                        }
                        Ok(ty)
                    })?;
                    let ret = self.ret_type()?;
                    self.expect(&Tok::RParen)?;
                    return Ok(TypeExpr::Fn(
                        params,
                        ret.map(Box::new),
                        start.to(self.prev_span()),
                    ));
                }
                let tys = self.list(&Tok::RParen, Self::parse_type)?;
                if tys.len() < 2 {
                    return err(
                        start.to(self.prev_span()),
                        "tuple types need at least two members",
                    );
                }
                Ok(TypeExpr::Tuple(tys, start.to(self.prev_span())))
            }
            Tok::Ident(s) if !is_keyword(&s) => {
                self.bump();
                let (module, name) =
                    if self.at(&Tok::Dot) && matches!(self.peek_n(1), Tok::Ident(_)) {
                        self.bump();
                        let (n, _) = self.ident()?;
                        (Some(s), n)
                    } else {
                        (None, s)
                    };
                let args = if self.at(&Tok::Lt) {
                    self.generic_args()?
                } else {
                    vec![]
                };
                Ok(TypeExpr::Named {
                    module,
                    name,
                    args,
                    span: start.to(self.prev_span()),
                })
            }
            t => err(start, format!("expected type, found {}", t.describe())),
        }
    }

    // ---------------------------------------------------------------- statements

    fn block(&mut self) -> Result<Block> {
        let start = self.expect(&Tok::LBrace)?;
        self.with_no_struct(false, |p| {
            p.skip_nl();
            let mut stmts = Vec::new();
            while !p.at(&Tok::RBrace) {
                if p.at(&Tok::Eof) {
                    return err(start, "unclosed block");
                }
                let at = p.pos;
                match p.stmt(false).and_then(|s| p.end_stmt().map(|_| s)) {
                    Ok(s) => stmts.push(s),
                    Err(d) => p.recover(d, at),
                }
            }
            let end = p.bump().span;
            Ok(Block {
                stmts,
                span: start.to(end),
            })
        })
    }

    fn label(&mut self) -> Option<String> {
        if let (Tok::Ident(name), Tok::Colon) = (self.peek().clone(), self.peek_n(1).clone())
            && !is_keyword(&name)
            && let Tok::Ident(k) = self.peek_n(2)
            && matches!(k.as_str(), "while" | "for" | "if" | "lock")
        {
            self.bump();
            self.bump();
            return Some(name);
        }
        None
    }

    fn break_label(&mut self) -> Result<Option<String>> {
        if self.at(&Tok::Colon) {
            self.bump();
            Ok(Some(self.ident()?.0))
        } else {
            Ok(None)
        }
    }

    fn at_stmt_end(&self) -> bool {
        matches!(self.peek(), Tok::Newline | Tok::RBrace | Tok::Eof)
    }

    fn stmt(&mut self, public: bool) -> Result<Stmt> {
        let saved_start = self.stmt_start;
        self.stmt_start = self.pos;
        let r = self.stmt_inner(public);
        self.stmt_start = saved_start;
        r
    }

    fn stmt_inner(&mut self, public: bool) -> Result<Stmt> {
        let start = self.span();
        let label = self.label();
        if let Some(label) = label {
            let mut s = self.stmt(false)?;
            match &mut s.kind {
                StmtKind::While { label: l, .. }
                | StmtKind::For { label: l, .. }
                | StmtKind::Lock { label: l, .. } => *l = Some(label),
                StmtKind::Expr(Expr {
                    kind: ExprKind::If(ife),
                    ..
                }) => ife.label = Some(label),
                _ => {
                    return err(
                        start,
                        "labels can only be applied to loops, `if` and `lock`",
                    );
                }
            }
            return Ok(s);
        }
        let kind = match self.peek().clone() {
            Tok::Ident(k) if k == "return" => {
                self.bump();
                if self.at_stmt_end() {
                    StmtKind::Return(None)
                } else {
                    let e = self.expr()?;
                    if self.at(&Tok::Comma) {
                        let mut items = vec![e];
                        while self.eat(&Tok::Comma) {
                            items.push(self.expr()?);
                        }
                        let sp = items[0].span.to(self.prev_span());
                        StmtKind::Return(Some(Expr {
                            kind: ExprKind::Tuple(items),
                            span: sp,
                        }))
                    } else {
                        StmtKind::Return(Some(e))
                    }
                }
            }
            Tok::Ident(k) if k == "throw" => {
                self.bump();
                StmtKind::Throw(self.expr()?)
            }
            Tok::Ident(k) if k == "break" => {
                self.bump();
                let label = self.break_label()?;
                let value = if label.is_none() && !self.at_stmt_end() {
                    Some(self.expr()?)
                } else {
                    None
                };
                StmtKind::Break { label, value }
            }
            Tok::Ident(k) if k == "continue" => {
                self.bump();
                StmtKind::Continue(self.break_label()?)
            }
            Tok::Ident(k) if k == "assert" => {
                self.bump();
                StmtKind::Assert(self.expr()?)
            }
            Tok::Ident(k) if k == "while" => {
                self.bump();
                let cond = self.no_struct_expr()?;
                let body = self.block()?;
                StmtKind::While {
                    label: None,
                    cond,
                    body,
                }
            }
            Tok::Ident(k) if k == "for" => {
                self.bump();
                let (var, var_span) = self.ident()?;
                self.expect_kw("in")?;
                let iter = self.no_struct_expr()?;
                let body = self.block()?;
                StmtKind::For {
                    label: None,
                    var,
                    var_span,
                    iter,
                    body,
                }
            }
            Tok::Ident(k) if k == "lock" => {
                self.bump();
                let (name, name_span) = self.ident()?;
                let body = self.block()?;
                StmtKind::Lock {
                    label: None,
                    name,
                    name_span,
                    body,
                }
            }
            Tok::LBrace => StmtKind::Block(self.block()?),
            Tok::Ident(k) if k == "fn" && matches!(self.peek_n(1), Tok::Ident(_)) => {
                if matches!(self.peek_n(2), Tok::Assign) {
                    self.bump();
                    let (name, _) = self.ident()?;
                    self.bump();
                    let value = self.expr()?;
                    StmtKind::FnVar { name, value }
                } else {
                    self.bump();
                    let sig = self.fn_sig(false)?;
                    let body = self.block()?;
                    StmtKind::Fn(FnDecl {
                        sig,
                        body,
                        public: false,
                    })
                }
            }
            Tok::Ident(k) if k == "_" && matches!(self.peek_n(1), Tok::Assign) => {
                self.bump();
                self.bump();
                StmtKind::Discard(self.expr()?)
            }
            Tok::Ident(k) if k == "mut" || k == "mutex" => {
                self.bump();
                let targets = self.decl_targets()?;
                self.expect(&Tok::Assign)?;
                let value = self.decl_value();
                StmtKind::VarDecl {
                    public,
                    mutable: k == "mut",
                    mutex: k == "mutex",
                    targets,
                    value,
                }
            }
            _ => {
                // try a declaration first
                let save = self.pos;
                if let Ok(targets) = self.decl_targets()
                    && self.at(&Tok::Assign)
                {
                    self.bump();
                    let value = self.decl_value();
                    return Ok(Stmt {
                        kind: StmtKind::VarDecl {
                            public,
                            mutable: false,
                            mutex: false,
                            targets,
                            value,
                        },
                        span: start.to(self.prev_span()),
                    });
                }
                self.pos = save;
                let e = self.expr()?;
                if self.eat(&Tok::Assign) {
                    let value = self.expr()?;
                    StmtKind::Assign { target: e, value }
                } else {
                    StmtKind::Expr(e)
                }
            }
        };
        Ok(Stmt {
            kind,
            span: start.to(self.prev_span()),
        })
    }

    fn decl_targets(&mut self) -> Result<Vec<DeclTarget>> {
        let mut out = Vec::new();
        loop {
            let start = self.span();
            let ty = self.parse_type()?;
            let (name, nsp) = match self.peek().clone() {
                Tok::Ident(s) if s == "_" => (s, self.bump().span),
                _ => self.ident()?,
            };
            out.push(DeclTarget {
                ty,
                name,
                span: start.to(nsp),
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- expressions

    fn no_struct_expr(&mut self) -> Result<Expr> {
        self.with_no_struct(true, Self::expr)
    }

    pub fn expr(&mut self) -> Result<Expr> {
        let mut e = self.binary(0)?;
        loop {
            if self.at_kw("else") {
                self.bump();
                let fb = if self.at(&Tok::LBrace) {
                    ElseFallback::Block(self.block()?)
                } else {
                    ElseFallback::Expr(Box::new(self.binary(0)?))
                };
                let sp = e.span.to(self.prev_span());
                e = Expr {
                    kind: ExprKind::Else(Box::new(e), fb),
                    span: sp,
                };
            } else if self.at_kw("catch") {
                self.bump();
                let (name, name_span) = match self.peek().clone() {
                    Tok::Ident(s) if s == "_" => (s, self.bump().span),
                    _ => self.ident()?,
                };
                let b = self.block()?;
                let sp = e.span.to(self.prev_span());
                e = Expr {
                    kind: ExprKind::Catch(Box::new(e), name, name_span, b),
                    span: sp,
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    /// The binary operator at the current token and its precedence (`**` is parsed apart).
    fn binop_info(&self) -> Option<(BinOp, u8)> {
        let op = match self.peek() {
            Tok::Ident(s) if s == "or" => BinOp::Or,
            Tok::Ident(s) if s == "and" => BinOp::And,
            Tok::Ident(s) if s == "in" => BinOp::In,
            Tok::Eq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Gt => BinOp::Gt,
            Tok::Le => BinOp::Le,
            Tok::Ge => BinOp::Ge,
            Tok::Concat => BinOp::Concat,
            Tok::Pipe => BinOp::BitOr,
            Tok::Caret => BinOp::BitXor,
            Tok::Amp => BinOp::BitAnd,
            Tok::Shl => BinOp::Shl,
            Tok::Shr => BinOp::Shr,
            Tok::Plus => BinOp::Add,
            Tok::Minus => BinOp::Sub,
            Tok::Star => BinOp::Mul,
            Tok::Slash => BinOp::Div,
            Tok::Percent => BinOp::Mod,
            _ => return None,
        };
        Some((op, op.prec()))
    }

    fn binary(&mut self, min: u8) -> Result<Expr> {
        let mut lhs = self.pow()?;
        let mut last_cmp = false;
        while let Some((op, prec)) = self.binop_info() {
            if prec < min {
                break;
            }
            if prec == 3 && last_cmp {
                return err(
                    self.span(),
                    "comparison operators cannot be chained; use parentheses",
                );
            }
            let opsp = self.bump().span;
            self.check_line_break(opsp, op.symbol())?;
            self.skip_nl();
            let rhs = self.binary(prec + 1)?;
            last_cmp = prec == 3;
            let sp = lhs.span.to(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span: sp,
            };
        }
        Ok(lhs)
    }

    /// An expression may only continue onto the next line after an operator when it is inside
    /// brackets (`( )`, `[ ]` or a struct literal's `{ }`).
    fn check_line_break(&self, opsp: Span, sym: &str) -> Result<()> {
        if !self.at(&Tok::Newline) || !self.open_brackets(self.stmt_start, self.pos).is_empty() {
            return Ok(());
        }
        let mut j = self.pos;
        while matches!(self.toks[j].tok, Tok::Newline) && j + 1 < self.toks.len() {
            j += 1;
        }
        let continued = !matches!(self.toks[j].tok, Tok::Eof | Tok::RBrace)
            && matches!((self.column(j), self.column(self.stmt_start)), (Some(n), Some(s)) if n > s);
        if continued {
            err(
                opsp,
                format!(
                    "an expression continued on the next line after `{sym}` must be wrapped in parentheses"
                ),
            )
        } else {
            err(
                opsp,
                format!(
                    "expected an expression after `{sym}` (to continue an expression on the next line, wrap it in parentheses)"
                ),
            )
        }
    }

    fn pow(&mut self) -> Result<Expr> {
        let base = self.unary()?;
        if self.at(&Tok::StarStar) {
            let opsp = self.bump().span;
            self.check_line_break(opsp, "**")?;
            self.skip_nl();
            let exp = self.pow()?; // right-assoc
            let sp = base.span.to(exp.span);
            return Ok(Expr {
                kind: ExprKind::Binary(BinOp::Pow, Box::new(base), Box::new(exp)),
                span: sp,
            });
        }
        Ok(base)
    }

    fn unary(&mut self) -> Result<Expr> {
        let start = self.span();
        let wrap: fn(Box<Expr>) -> ExprKind = match self.peek() {
            Tok::Minus => |e| ExprKind::Unary(UnOp::Neg, e),
            Tok::Bang => |e| ExprKind::Unary(UnOp::BitNot, e),
            Tok::Ident(k) if k == "not" => |e| ExprKind::Unary(UnOp::Not, e),
            Tok::Ident(k) if k == "try" => ExprKind::Try,
            Tok::Ident(k) if k == "async" => ExprKind::Async,
            Tok::Ident(k) if k == "await" => ExprKind::Await,
            _ => return self.postfix(),
        };
        self.bump();
        let e = self.unary()?;
        Ok(Expr {
            kind: wrap(Box::new(e)),
            span: start.to(self.prev_span()),
        })
    }

    fn is_struct_lit_start(&self) -> bool {
        if !self.at(&Tok::LBrace) {
            return false;
        }
        let mut i = 1;
        while matches!(self.peek_n(i), Tok::Newline) {
            i += 1;
        }
        match self.peek_n(i) {
            Tok::Dot => true,
            Tok::RBrace => !self.no_struct,
            _ => false,
        }
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            let start = e.span;
            match self.peek().clone() {
                Tok::LParen => {
                    self.bump();
                    let args = self.with_no_struct(false, |p| p.list(&Tok::RParen, Self::expr))?;
                    e = Expr {
                        kind: ExprKind::Call(Box::new(e), args),
                        span: start.to(self.prev_span()),
                    };
                }
                Tok::LBracket => {
                    self.bump();
                    // `x[i]`, or a slice `x[a:b]` (both bounds are required)
                    let (lo, hi) = self.with_no_struct(false, |p| {
                        let missing = "a slice needs both bounds: `x[start:end]`";
                        p.skip_nl();
                        if p.at(&Tok::Colon) {
                            return err(p.span(), missing);
                        }
                        let lo = Box::new(p.expr()?);
                        p.skip_nl();
                        let mut hi = None;
                        if p.eat(&Tok::Colon) {
                            p.skip_nl();
                            if p.at(&Tok::RBracket) {
                                return err(p.span(), missing);
                            }
                            hi = Some(Box::new(p.expr()?));
                            p.skip_nl();
                        }
                        Ok((lo, hi))
                    })?;
                    self.expect(&Tok::RBracket)?;
                    let kind = match hi {
                        Some(hi) => ExprKind::Slice(Box::new(e), lo, hi),
                        None => ExprKind::Index(Box::new(e), lo),
                    };
                    e = Expr {
                        kind,
                        span: start.to(self.prev_span()),
                    };
                }
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.ident()?;
                    e = Expr {
                        kind: ExprKind::Field(Box::new(e), name),
                        span: start.to(self.prev_span()),
                    };
                }
                Tok::Lt if matches!(e.kind, ExprKind::Ident(_) | ExprKind::Field(..)) => {
                    // speculative generic arguments
                    let save = self.pos;
                    let saved_toks = self.toks.clone();
                    match self.generic_args() {
                        Ok(args) if matches!(self.peek(), Tok::LParen | Tok::LBrace | Tok::Dot) => {
                            e = Expr {
                                kind: ExprKind::Generic(Box::new(e), args),
                                span: start.to(self.prev_span()),
                            };
                        }
                        _ => {
                            self.pos = save;
                            self.toks = saved_toks;
                            break;
                        }
                    }
                }
                Tok::LBrace
                    if matches!(
                        e.kind,
                        ExprKind::Ident(_) | ExprKind::Field(..) | ExprKind::Generic(..)
                    ) && self.is_struct_lit_start() =>
                {
                    self.bump();
                    let fields = self.list(&Tok::RBrace, |p| {
                        let fsp = p.expect(&Tok::Dot)?;
                        let (name, _) = p.ident()?;
                        p.expect(&Tok::Assign)?;
                        p.skip_nl();
                        Ok((name, p.expr()?, fsp))
                    })?;
                    e = Expr {
                        kind: ExprKind::Struct(Box::new(e), fields),
                        span: start.to(self.prev_span()),
                    };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// The expressions of a list after its first one (`, b, c)`), up to `close` (consumed).
    fn rest_of_list(&mut self, first: Expr, close: &Tok) -> Result<Vec<Expr>> {
        let mut items = vec![first];
        while self.eat(&Tok::Comma) {
            self.skip_nl();
            if self.at(close) {
                break;
            }
            items.push(self.expr()?);
            self.skip_nl();
        }
        self.expect(close)?;
        Ok(items)
    }

    fn primary(&mut self) -> Result<Expr> {
        let start = self.span();
        let t = self.peek().clone();
        let kind = match t {
            Tok::Int(v, u) => {
                self.bump();
                ExprKind::Int(v, u)
            }
            Tok::Float(f) => {
                self.bump();
                ExprKind::Float(f)
            }
            Tok::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            Tok::Str(parts) => {
                self.bump();
                let mut segs = Vec::new();
                for p in parts {
                    match p {
                        StrPart::Lit(s) => segs.push(StrSeg::Lit(s)),
                        StrPart::Expr(text, off) => {
                            let toks = lex(&text, self.file, off)?;
                            let mut sub = Self::new(toks, self.file, String::new());
                            sub.skip_nl();
                            let e = sub.expr()?;
                            sub.skip_nl();
                            if !sub.at(&Tok::Eof) {
                                return err(
                                    sub.span(),
                                    "unexpected tokens in string interpolation",
                                );
                            }
                            segs.push(StrSeg::Expr(e));
                        }
                    }
                }
                ExprKind::Str(segs)
            }
            Tok::Dollar => {
                self.bump();
                ExprKind::Dollar
            }
            Tok::Builtin(name) => {
                self.bump();
                self.expect(&Tok::LParen)?;
                let mut ty = None;
                if name == "as" {
                    self.skip_nl();
                    ty = Some(self.parse_type()?);
                    self.skip_nl();
                    if !self.at(&Tok::RParen) {
                        self.expect(&Tok::Comma)?;
                    }
                }
                let args = self.list(&Tok::RParen, Self::expr)?;
                ExprKind::Builtin(name, ty, args)
            }
            Tok::LParen => {
                self.bump();
                let saved = self.no_struct;
                self.no_struct = false;
                self.skip_nl();
                let first = self.expr()?;
                self.skip_nl();
                let kind = if self.at(&Tok::Comma) {
                    ExprKind::Tuple(self.rest_of_list(first, &Tok::RParen)?)
                } else {
                    self.expect(&Tok::RParen)?;
                    self.no_struct = saved;
                    // keep the inner node but widen the span
                    return Ok(Expr {
                        kind: first.kind,
                        span: start.to(self.prev_span()),
                    });
                };
                self.no_struct = saved;
                kind
            }
            Tok::LBracket => {
                self.bump();
                let saved = self.no_struct;
                self.no_struct = false;
                self.skip_nl();
                let kind = if self.at(&Tok::RBracket) {
                    self.bump();
                    ExprKind::Array(vec![])
                } else {
                    let first = self.expr()?;
                    self.skip_nl();
                    if self.eat(&Tok::Colon) {
                        self.skip_nl();
                        let v = self.expr()?;
                        let mut entries = vec![(first, v)];
                        self.skip_nl();
                        while self.eat(&Tok::Comma) {
                            self.skip_nl();
                            if self.at(&Tok::RBracket) {
                                break;
                            }
                            let k = self.expr()?;
                            self.skip_nl();
                            self.expect(&Tok::Colon)?;
                            self.skip_nl();
                            let v = self.expr()?;
                            entries.push((k, v));
                            self.skip_nl();
                        }
                        self.expect(&Tok::RBracket)?;
                        ExprKind::Map(entries)
                    } else {
                        ExprKind::Array(self.rest_of_list(first, &Tok::RBracket)?)
                    }
                };
                self.no_struct = saved;
                kind
            }
            Tok::Ident(k) => match k.as_str() {
                "true" => {
                    self.bump();
                    ExprKind::Bool(true)
                }
                "false" => {
                    self.bump();
                    ExprKind::Bool(false)
                }
                "none" => {
                    self.bump();
                    ExprKind::None
                }
                "_" => {
                    self.bump();
                    ExprKind::Wildcard
                }
                "fn" => {
                    self.bump();
                    let (params, ret) = self.params_and_ret()?;
                    let sig = FnSig {
                        name: String::new(),
                        type_params: vec![],
                        params,
                        ret,
                        span: start,
                    };
                    let body = self.block()?;
                    ExprKind::FnLit(Box::new(sig), body)
                }
                "if" => {
                    self.bump();
                    let scrutinee = if self.at(&Tok::LBrace) {
                        None
                    } else {
                        Some(self.no_struct_expr()?)
                    };
                    self.expect(&Tok::LBrace)?;
                    let saved = self.no_struct;
                    self.no_struct = false;
                    self.skip_nl();
                    let mut arms = Vec::new();
                    while !self.at(&Tok::RBrace) {
                        let asp = self.span();
                        let mut patterns = vec![self.expr()?];
                        while self.eat(&Tok::Comma) {
                            self.skip_nl();
                            patterns.push(self.expr()?);
                        }
                        self.expect(&Tok::Arrow)?;
                        let body = self.block()?;
                        arms.push(Arm {
                            patterns,
                            body,
                            span: asp.to(self.prev_span()),
                        });
                        self.eat(&Tok::Comma);
                        self.skip_nl();
                    }
                    self.bump();
                    self.no_struct = saved;
                    ExprKind::If(Box::new(IfExpr {
                        label: None,
                        scrutinee,
                        arms,
                        span: start.to(self.prev_span()),
                    }))
                }
                _ if is_keyword(&k) => {
                    return err(start, format!("unexpected keyword `{k}`"));
                }
                _ => {
                    self.bump();
                    ExprKind::Ident(k)
                }
            },
            t => {
                return err(
                    start,
                    format!("expected expression, found {}", t.describe()),
                );
            }
        };
        Ok(Expr {
            kind,
            span: start.to(self.prev_span()),
        })
    }
}
