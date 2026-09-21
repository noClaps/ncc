use crate::{
    ast::*,
    diagnostic::Diagnostics,
    lexer::{Keyword, Token, TokenKind},
};

pub fn parse(tokens: Vec<Token>) -> Result<Module, Diagnostics> {
    parse_at(tokens, std::path::Path::new("<source>"))
}
pub fn parse_at(tokens: Vec<Token>, path: &std::path::Path) -> Result<Module, Diagnostics> {
    Parser {
        tokens,
        pos: 0,
        path: path.to_path_buf(),
    }
    .module()
}
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    path: std::path::PathBuf,
}
impl Parser {
    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }
    fn at(&self, k: &TokenKind) -> bool {
        &self.current().kind == k
    }
    fn bump(&mut self) -> Token {
        let t = self.current().clone();
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1
        };
        t
    }
    fn error<T>(&self, s: impl Into<String>) -> Result<T, Diagnostics> {
        Err(Diagnostics::one(s, self.current().span.clone()))
    }
    fn keyword(&mut self, k: Keyword) -> bool {
        if self.at(&TokenKind::Keyword(k)) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, k: TokenKind) -> Result<(), Diagnostics> {
        if self.at(&k) {
            self.bump();
            Ok(())
        } else {
            self.error(format!("expected {:?}", k))
        }
    }
    fn ident(&mut self) -> Result<String, Diagnostics> {
        let token = self.bump();
        match token.kind {
            TokenKind::Ident(s) => Ok(s),
            x => Err(Diagnostics::one(
                format!("expected identifier, found {:?}", x),
                token.span,
            )),
        }
    }
    fn module(&mut self) -> Result<Module, Diagnostics> {
        let mut items = vec![];
        while !self.at(&TokenKind::Eof) {
            if self.keyword(Keyword::Import) {
                self.expect(TokenKind::LBrace)?;
                while !self.at(&TokenKind::RBrace) {
                    let path = match self.bump().kind {
                        TokenKind::String(path) => path,
                        _ => return self.error("expected import path"),
                    };
                    if !self.keyword(Keyword::As) {
                        return self.error("expected `as` after import path");
                    }
                    let alias = self.ident()?;
                    items.push(Item::Import { path, alias });
                }
                self.bump();
                continue;
            }
            items.push(self.item()?)
        }
        Ok(Module { items })
    }
    fn item(&mut self) -> Result<Item, Diagnostics> {
        if self.keyword(Keyword::Extern) {
            let path = match self.bump().kind {
                TokenKind::String(path) => path,
                _ => return self.error("expected external implementation path"),
            };
            if !self.keyword(Keyword::As) {
                return self.error("expected `as` in extern declaration");
            }
            let alias = self.ident()?;
            self.expect(TokenKind::LBrace)?;
            let mut functions = Vec::new();
            while !self.at(&TokenKind::RBrace) {
                if !self.keyword(Keyword::Fn) {
                    return self.error("extern blocks may only contain functions");
                }
                let name = self.ident()?;
                let params = self.params()?;
                let return_type = if self.at(&TokenKind::Assign) {
                    Type::void()
                } else {
                    self.ty()?
                };
                let throws = if self.at(&TokenKind::Bang) {
                    self.bump();
                    true
                } else {
                    false
                };
                self.expect(TokenKind::Assign)?;
                let symbol = match self.bump().kind {
                    TokenKind::String(symbol) => symbol,
                    _ => return self.error("expected external symbol string"),
                };
                functions.push(FunctionDecl {
                    name,
                    params,
                    return_type,
                    throws,
                    symbol,
                });
            }
            self.bump();
            return Ok(Item::Extern {
                path,
                alias,
                functions,
            });
        }
        let public = self.keyword(Keyword::Pub);
        if self.keyword(Keyword::Struct) {
            return self.struct_item(public).map(Item::Struct);
        }
        if self.keyword(Keyword::Enum) {
            return self.enum_item(public).map(Item::Enum);
        }
        if self.keyword(Keyword::Type) {
            let name = self.ident()?;
            self.expect(TokenKind::Assign)?;
            return Ok(Item::TypeAlias {
                public,
                name,
                ty: self.ty()?,
            });
        }
        if self.at(&TokenKind::Keyword(Keyword::Fn))
            && !matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::Assign)
            )
        {
            self.bump();
            return self.function(public).map(Item::Function);
        }
        if !public && self.keyword(Keyword::Test) {
            let name = match self.bump().kind {
                TokenKind::String(s) => s,
                _ => return self.error("expected test name"),
            };
            return Ok(Item::Test {
                name,
                body: self.block()?,
            });
        }
        match self.stmt()? {
            Stmt::Var(mut declaration) => {
                declaration.public = public;
                Ok(Item::Global(declaration))
            }
            statement => {
                if public {
                    self.error("`pub` can only precede a declaration")
                } else {
                    Ok(Item::Statement(statement))
                }
            }
        }
    }
    fn generics(&mut self) -> Result<Vec<String>, Diagnostics> {
        if !self.at(&TokenKind::Lt) {
            return Ok(vec![]);
        }
        self.bump();
        let mut result = vec![];
        while !self.at(&TokenKind::Gt) {
            self.keyword(Keyword::Type);
            result.push(self.ident()?);
            if !self.at(&TokenKind::Gt) {
                self.expect(TokenKind::Comma)?
            }
        }
        self.bump();
        Ok(result)
    }
    fn struct_item(&mut self, public: bool) -> Result<StructDecl, Diagnostics> {
        let name = self.ident()?;
        let generics = self.generics()?;
        self.expect(TokenKind::LBrace)?;
        let mut fields = vec![];
        while !self.at(&TokenKind::RBrace) {
            let ty = self.ty()?;
            let name = self.ident()?;
            fields.push(Field { name, ty });
        }
        self.bump();
        Ok(StructDecl {
            public,
            name,
            generics,
            fields,
        })
    }
    fn enum_item(&mut self, public: bool) -> Result<EnumDecl, Diagnostics> {
        let name = self.ident()?;
        let generics = self.generics()?;
        self.expect(TokenKind::LBrace)?;
        let mut variants = vec![];
        while !self.at(&TokenKind::RBrace) {
            let name = self.ident()?;
            let mut values = vec![];
            if self.at(&TokenKind::LParen) {
                self.bump();
                while !self.at(&TokenKind::RParen) {
                    values.push(self.ty()?);
                    if !self.at(&TokenKind::RParen) {
                        self.expect(TokenKind::Comma)?
                    }
                }
                let _ = self.bump();
            }
            variants.push(Variant { name, values });
        }
        self.bump();
        Ok(EnumDecl {
            public,
            name,
            generics,
            variants,
        })
    }
    fn function(&mut self, public: bool) -> Result<Function, Diagnostics> {
        let start = self.current().span.start;
        let name = self.ident()?;
        let generics = self.generics()?;
        let params = self.params()?;
        let return_type = if self.at(&TokenKind::Bang) {
            self.bump();
            Type::ErrorUnion(Box::new(Type::void()))
        } else if self.at(&TokenKind::LBrace) {
            Type::void()
        } else {
            self.ty()?
        };
        let throws = if self.at(&TokenKind::Bang) {
            self.bump();
            true
        } else {
            false
        };
        let body = self.block()?;
        Ok(Function {
            source_path: self.path.clone(),
            span: start..self.tokens[self.pos - 1].span.end,
            public,
            name,
            generics,
            params,
            return_type,
            throws,
            body,
        })
    }
    fn params(&mut self) -> Result<Vec<Param>, Diagnostics> {
        self.expect(TokenKind::LParen)?;
        let mut out = vec![];
        while !self.at(&TokenKind::RParen) {
            let ty = self.ty()?;
            let name = self.ident()?;
            out.push(Param { name, ty });
            if !self.at(&TokenKind::RParen) {
                self.expect(TokenKind::Comma)?
            }
        }
        self.bump();
        Ok(out)
    }
    fn ty(&mut self) -> Result<Type, Diagnostics> {
        if self.keyword(Keyword::Fut) {
            return Ok(Type::Future(Box::new(self.ty()?)));
        }
        let mut ty = if self.at(&TokenKind::LBracket) {
            self.bump();
            let key = self.ty()?;
            self.expect(TokenKind::RBracket)?;
            Type::Map(Box::new(key), Box::new(self.ty()?))
        } else if self.at(&TokenKind::LParen) {
            self.bump();
            if self.keyword(Keyword::Fn) {
                let args = self.params_types()?;
                let ret = self.ty()?;
                self.expect(TokenKind::RParen)?;
                Type::Function(args, Box::new(ret))
            } else {
                let mut xs = vec![self.ty()?];
                while self.at(&TokenKind::Comma) {
                    self.bump();
                    xs.push(self.ty()?)
                }
                self.expect(TokenKind::RParen)?;
                Type::Tuple(xs)
            }
        } else {
            let mut name = self.ident()?;
            while self.at(&TokenKind::Dot) {
                self.bump();
                name.push('.');
                name.push_str(&self.ident()?);
            }
            Type::Named(name, self.type_args()?)
        };
        loop {
            if self.at(&TokenKind::LBracket) {
                self.bump();
                let n = if self.at(&TokenKind::RBracket) {
                    None
                } else {
                    match self.bump().kind {
                        TokenKind::Int(x) => Some(x.parse().map_err(|_| {
                            Diagnostics::one("invalid array size", self.current().span.clone())
                        })?),
                        _ => return self.error("expected array size"),
                    }
                };
                self.expect(TokenKind::RBracket)?;
                ty = Type::Array(Box::new(ty), n)
            } else if self.at(&TokenKind::Question) {
                self.bump();
                ty = Type::Optional(Box::new(ty))
            } else if self.at(&TokenKind::Bang) {
                self.bump();
                ty = Type::ErrorUnion(Box::new(ty));
            } else {
                break;
            }
        }
        Ok(ty)
    }
    fn type_args(&mut self) -> Result<Vec<Type>, Diagnostics> {
        if !self.at(&TokenKind::Lt) {
            return Ok(vec![]);
        }
        self.bump();
        let mut out = vec![];
        while !self.at(&TokenKind::Gt) && !self.at(&TokenKind::Shr) {
            out.push(self.ty()?);
            if !self.at(&TokenKind::Gt) && !self.at(&TokenKind::Shr) {
                self.expect(TokenKind::Comma)?
            }
        }
        if self.at(&TokenKind::Shr) {
            // Leave the second angle bracket for the enclosing type application.
            self.tokens[self.pos].kind = TokenKind::Gt;
            self.tokens[self.pos].span.start += 1;
        } else {
            self.bump();
        }
        Ok(out)
    }
    fn params_types(&mut self) -> Result<Vec<Type>, Diagnostics> {
        self.expect(TokenKind::LParen)?;
        let mut x = vec![];
        while !self.at(&TokenKind::RParen) {
            x.push(self.ty()?);
            if !self.at(&TokenKind::RParen) {
                self.expect(TokenKind::Comma)?
            }
        }
        self.bump();
        Ok(x)
    }
    fn var_decl(&mut self) -> Result<VarDecl, Diagnostics> {
        let mutex = self.keyword(Keyword::Mutex);
        let mutable = self.keyword(Keyword::Mut);
        let mut ty = self.ty()?;
        let mut pattern = Pattern::Name(self.ident()?);
        if self.at(&TokenKind::Comma) {
            let mut types = vec![ty];
            let mut patterns = vec![pattern];
            while self.at(&TokenKind::Comma) {
                self.bump();
                types.push(self.ty()?);
                patterns.push(Pattern::Name(self.ident()?));
            }
            ty = Type::Tuple(types);
            pattern = Pattern::Tuple(patterns);
        }
        self.expect(TokenKind::Assign)?;
        let value = self.expr(0)?;
        Ok(VarDecl {
            public: false,
            mutable,
            mutex,
            pattern,
            ty,
            value,
        })
    }
    fn block(&mut self) -> Result<Block, Diagnostics> {
        self.expect(TokenKind::LBrace)?;
        let mut statements = vec![];
        while !self.at(&TokenKind::RBrace) {
            statements.push(self.stmt()?)
        }
        self.bump();
        Ok(Block { statements })
    }
    fn stmt(&mut self) -> Result<Stmt, Diagnostics> {
        if self.keyword(Keyword::Fn) {
            if matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::LParen)
            ) {
                let function = self.function(false)?;
                let ty = Type::Function(
                    function.params.iter().map(|p| p.ty.clone()).collect(),
                    Box::new(function.return_type.clone()),
                );
                return Ok(Stmt::Var(VarDecl {
                    public: false,
                    mutable: false,
                    mutex: false,
                    pattern: Pattern::Name(function.name.clone()),
                    ty,
                    value: Expr::Lambda(Box::new(function)),
                }));
            }
            let name = self.ident()?;
            self.expect(TokenKind::Assign)?;
            let value = self.expr(0)?;
            let Expr::Lambda(f) = &value else {
                return self.error("inferred `fn` declarations require an anonymous function");
            };
            let ty = Type::Function(
                f.params.iter().map(|p| p.ty.clone()).collect(),
                Box::new(f.return_type.clone()),
            );
            return Ok(Stmt::Var(VarDecl {
                public: false,
                mutable: false,
                mutex: false,
                pattern: Pattern::Name(name),
                ty,
                value,
            }));
        }
        if self.at(&TokenKind::LBrace) {
            return self.block().map(Stmt::Block);
        }
        let label = if let TokenKind::Ident(name) = &self.current().kind {
            if matches!(
                self.tokens.get(self.pos + 1).map(|token| &token.kind),
                Some(TokenKind::Colon)
            ) {
                let name = name.clone();
                self.bump();
                self.bump();
                Some(name)
            } else {
                None
            }
        } else {
            None
        };
        if self.keyword(Keyword::Break) {
            let label_target = if self.at(&TokenKind::Colon) {
                self.bump();
                Some(self.ident()?)
            } else {
                None
            };
            let value = if label_target.is_some()
                || self.at(&TokenKind::RBrace)
                || self.current().newline_before
            {
                None
            } else {
                Some(self.expr(0)?)
            };
            return Ok(Stmt::Break(value, label_target));
        }
        if self.keyword(Keyword::Continue) {
            let label_target = if self.at(&TokenKind::Colon) {
                self.bump();
                Some(self.ident()?)
            } else {
                None
            };
            return Ok(Stmt::Continue(label_target));
        }
        if self.keyword(Keyword::Return) {
            if self.at(&TokenKind::RBrace) || self.current().newline_before {
                return Ok(Stmt::Return(None));
            }
            let mut value = self.expr(0)?;
            if self.at(&TokenKind::Comma) {
                let mut values = vec![value];
                while self.at(&TokenKind::Comma) {
                    self.bump();
                    values.push(self.expr(0)?);
                }
                value = Expr::Tuple(values);
            }
            return Ok(Stmt::Return(Some(value)));
        }
        if self.keyword(Keyword::Throw) {
            return Ok(Stmt::Throw(self.expr(0)?));
        }
        if self.keyword(Keyword::Assert) {
            return Ok(Stmt::Assert(self.expr(0)?));
        }
        if self.keyword(Keyword::For) {
            let name = self.ident()?;
            if !self.keyword(Keyword::In) {
                return self.error("expected `in`");
            };
            let iterable = self.expr(0)?;
            return Ok(Stmt::For {
                label,
                name,
                iterable,
                body: self.block()?,
            });
        }
        if self.keyword(Keyword::While) {
            let condition = self.expr(0)?;
            return Ok(Stmt::While {
                label,
                condition,
                body: self.block()?,
            });
        }
        if self.keyword(Keyword::Lock) {
            let name = self.ident()?;
            return Ok(Stmt::Lock {
                label,
                name,
                body: self.block()?,
            });
        }
        if let Some(label) = label {
            if self.at(&TokenKind::Keyword(Keyword::If)) {
                return Ok(Stmt::LabeledIf {
                    label,
                    value: self.expr(0)?,
                });
            }
            return self.error("labels may only be applied to if, for, while, or lock blocks");
        }
        let saved = self.pos;
        let saved_tokens = self.tokens.clone();
        self.keyword(Keyword::Mut);
        self.keyword(Keyword::Mutex);
        let is_decl = self.ty().is_ok() && matches!(self.current().kind, TokenKind::Ident(_));
        self.pos = saved;
        self.tokens = saved_tokens;
        if is_decl {
            return self.var_decl().map(Stmt::Var);
        }
        let target = self.expr(0)?;
        if self.at(&TokenKind::Assign) {
            self.bump();
            return Ok(Stmt::Assign {
                target,
                value: self.expr(0)?,
            });
        }
        Ok(Stmt::Expr(target))
    }
    fn expr(&mut self, min: u8) -> Result<Expr, Diagnostics> {
        let mut left = self.prefix()?;
        loop {
            if self.at(&TokenKind::Lt) && matches!(left, Expr::Name(_) | Expr::Member { .. }) {
                let position = self.pos;
                let tokens = self.tokens.clone();
                if let Ok(generics) = self.type_args() {
                    if let Some(name) = expression_path(&left)
                        && self.at(&TokenKind::LBrace)
                    {
                        self.bump();
                        let mut fields = vec![];
                        while !self.at(&TokenKind::RBrace) {
                            self.expect(TokenKind::Dot)?;
                            let field = self.ident()?;
                            self.expect(TokenKind::Assign)?;
                            fields.push((field, self.expr(0)?));
                            if !self.at(&TokenKind::RBrace) {
                                self.expect(TokenKind::Comma)?;
                            }
                        }
                        self.bump();
                        left = Expr::Cast {
                            ty: Type::Named(name.clone(), generics),
                            value: Box::new(Expr::StructInit { name, fields }),
                        };
                        continue;
                    }
                    if self.at(&TokenKind::LParen) {
                        self.bump();
                        let mut args = vec![];
                        while !self.at(&TokenKind::RParen) {
                            args.push(self.expr(0)?);
                            if !self.at(&TokenKind::RParen) {
                                self.expect(TokenKind::Comma)?;
                            }
                        }
                        self.bump();
                        left = Expr::Call {
                            callee: Box::new(left),
                            args,
                            generics,
                        };
                        continue;
                    }
                }
                self.pos = position;
                self.tokens = tokens;
            }
            if min == 0 && self.keyword(Keyword::Else) {
                let fallback = if self.at(&TokenKind::LBrace) {
                    self.block()?
                } else {
                    Block {
                        statements: vec![Stmt::Break(Some(self.expr(0)?), None)],
                    }
                };
                left = Expr::Else {
                    value: Box::new(left),
                    fallback,
                };
                continue;
            }
            if min == 0 && self.keyword(Keyword::Catch) {
                let name = self.ident()?;
                let body = self.block()?;
                left = Expr::Catch {
                    value: Box::new(left),
                    name,
                    body,
                };
                continue;
            }
            if self.current().newline_before
                && matches!(
                    self.current().kind,
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace
                )
            {
                break;
            }
            if self.at(&TokenKind::LBrace)
                && self
                    .tokens
                    .get(self.pos + 1)
                    .is_some_and(|t| t.kind == TokenKind::Dot)
                && let Some(name) = expression_path(&left)
            {
                self.bump();
                let mut fields = vec![];
                while !self.at(&TokenKind::RBrace) {
                    self.expect(TokenKind::Dot)?;
                    let field = self.ident()?;
                    self.expect(TokenKind::Assign)?;
                    fields.push((field, self.expr(0)?));
                    if !self.at(&TokenKind::RBrace) {
                        self.expect(TokenKind::Comma)?;
                    }
                }
                self.bump();
                left = Expr::StructInit { name, fields };
                continue;
            }
            if self.at(&TokenKind::LParen) {
                self.bump();
                let mut args = vec![];
                while !self.at(&TokenKind::RParen) {
                    args.push(self.expr(0)?);
                    if !self.at(&TokenKind::RParen) {
                        self.expect(TokenKind::Comma)?
                    }
                }
                self.bump();
                left = Expr::Call {
                    callee: Box::new(left),
                    args,
                    generics: vec![],
                };
                continue;
            }
            if self.at(&TokenKind::LBracket) {
                self.bump();
                let index = self.expr(0)?;
                self.expect(TokenKind::RBracket)?;
                left = Expr::Index {
                    object: Box::new(left),
                    index: Box::new(index),
                };
                continue;
            }
            if self.at(&TokenKind::Dot) {
                self.bump();
                left = Expr::Member {
                    object: Box::new(left),
                    name: self.ident()?,
                };
                continue;
            }
            let Some((op, p)) = self.binop() else { break };
            if p < min {
                break;
            }
            self.bump();
            let right = self.expr(if op == BinaryOp::Pow { p } else { p + 1 })?;
            left = Expr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            }
        }
        Ok(left)
    }
    fn prefix(&mut self) -> Result<Expr, Diagnostics> {
        let start = self.current().span.start;
        if self.keyword(Keyword::Fn) {
            let params = self.params()?;
            let return_type = if self.at(&TokenKind::LBrace) {
                Type::void()
            } else {
                self.ty()?
            };
            let body = self.block()?;
            return Ok(Expr::Lambda(Box::new(Function {
                source_path: self.path.clone(),
                span: start..self.tokens[self.pos - 1].span.end,
                public: false,
                name: "anonymous".into(),
                generics: vec![],
                params,
                return_type,
                throws: false,
                body,
            })));
        }
        let t = self.bump();
        match t.kind {
            TokenKind::Keyword(Keyword::Try) => Ok(Expr::Try(Box::new(self.expr(12)?))),
            TokenKind::Keyword(Keyword::Await) => Ok(Expr::Await(Box::new(self.expr(12)?))),
            TokenKind::Keyword(Keyword::Async) => Ok(Expr::Async(Box::new(self.expr(12)?))),
            TokenKind::Dollar => Ok(Expr::Name("$".into())),
            TokenKind::At => {
                if self.keyword(Keyword::As) {
                    self.expect(TokenKind::LParen)?;
                    let ty = self.ty()?;
                    self.expect(TokenKind::Comma)?;
                    let value = Box::new(self.expr(0)?);
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::Cast { ty, value })
                } else {
                    Ok(Expr::Name(format!("@{}", self.ident()?)))
                }
            }
            TokenKind::Int(x) => Ok(Expr::Int(x)),
            TokenKind::Float(x) => Ok(Expr::Float(x)),
            TokenKind::String(x) => self.string_expression(&x),
            TokenKind::Char(x) => Ok(Expr::Char(x)),
            TokenKind::Ident(x) => Ok(Expr::Name(x)),
            TokenKind::Keyword(Keyword::True) => Ok(Expr::Bool(true)),
            TokenKind::Keyword(Keyword::False) => Ok(Expr::Bool(false)),
            TokenKind::Keyword(Keyword::None) => Ok(Expr::None),
            TokenKind::Keyword(Keyword::Not) => Ok(Expr::Unary {
                op: UnaryOp::Not,
                value: Box::new(self.expr(12)?),
            }),
            TokenKind::Minus => Ok(Expr::Unary {
                op: UnaryOp::Neg,
                value: Box::new(self.expr(12)?),
            }),
            TokenKind::Bang => Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                value: Box::new(self.expr(12)?),
            }),
            TokenKind::Keyword(Keyword::If) => self.if_expr(),
            TokenKind::LParen => {
                let first = self.expr(0)?;
                if self.at(&TokenKind::Comma) {
                    let mut x = vec![first];
                    while self.at(&TokenKind::Comma) {
                        self.bump();
                        x.push(self.expr(0)?)
                    }
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::Tuple(x))
                } else {
                    self.expect(TokenKind::RParen)?;
                    Ok(first)
                }
            }
            TokenKind::LBracket => {
                let mut x = vec![];
                let mut entries = vec![];
                let mut is_map = false;
                while !self.at(&TokenKind::RBracket) {
                    let value = self.expr(0)?;
                    if self.at(&TokenKind::Colon) {
                        if !x.is_empty() {
                            return self.error("cannot mix map entries and array elements");
                        }
                        self.bump();
                        is_map = true;
                        entries.push((value, self.expr(0)?));
                    } else {
                        if is_map {
                            return self.error("expected `:` after map key");
                        }
                        x.push(value);
                    }
                    if !self.at(&TokenKind::RBracket) {
                        self.expect(TokenKind::Comma)?
                    }
                }
                self.bump();
                Ok(if is_map {
                    Expr::Map(entries)
                } else {
                    Expr::Array(x)
                })
            }
            x => self.error(format!("expected expression, found {:?}", x)),
        }
    }
    fn if_expr(&mut self) -> Result<Expr, Diagnostics> {
        let subject = if self.at(&TokenKind::LBrace) {
            None
        } else {
            Some(Box::new(self.expr(0)?))
        };
        self.expect(TokenKind::LBrace)?;
        let mut arms = vec![];
        while !self.at(&TokenKind::RBrace) {
            let mut patterns = vec![self.pattern()?];
            while self.at(&TokenKind::Comma) {
                self.bump();
                patterns.push(self.pattern()?);
            }
            if !self.at(&TokenKind::Arrow) {
                return self.error("expected `->` after conditional pattern(s)");
            }
            self.bump();
            arms.push((patterns, self.block()?));
        }
        self.bump();
        Ok(Expr::If { subject, arms })
    }
    fn string_expression(&self, text: &str) -> Result<Expr, Diagnostics> {
        let mut parts = vec![];
        let mut literal = String::new();
        let mut chars = text.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if c == '\\' && chars.peek().is_some_and(|(_, c)| *c == '{') {
                chars.next();
                literal.push('{');
                continue;
            }
            if c != '{' {
                literal.push(c);
                continue;
            }
            parts.push(Expr::String(std::mem::take(&mut literal)));
            let start = chars.peek().map_or(text.len(), |(i, _)| *i);
            let mut depth = 1usize;
            let mut end = None;
            let mut quote = None;
            let mut escape = false;
            for (i, c) in chars.by_ref() {
                if escape {
                    escape = false;
                    continue;
                }
                if let Some(q) = quote {
                    if c == '\\' {
                        escape = true;
                    } else if c == q {
                        quote = None;
                    }
                    continue;
                }
                match c {
                    '\'' | '"' => quote = Some(c),
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                return self.error("unterminated format-string expression");
            };
            let mut parser = Parser {
                path: self.path.clone(),
                tokens: crate::lexer::lex(&text[start..end])?,
                pos: 0,
            };
            let value = parser.expr(0)?;
            parser.expect(TokenKind::Eof)?;
            parts.push(Expr::Cast {
                ty: Type::Named("str".into(), vec![]),
                value: Box::new(value),
            });
        }
        parts.push(Expr::String(literal));
        Ok(parts
            .into_iter()
            .reduce(|left, right| Expr::Binary {
                left: Box::new(left),
                op: BinaryOp::Concat,
                right: Box::new(right),
            })
            .unwrap())
    }
    fn pattern(&mut self) -> Result<Pattern, Diagnostics> {
        if self.at(&TokenKind::Ident("_".into())) {
            self.bump();
            Ok(Pattern::Wildcard)
        } else {
            Ok(expression_pattern(self.expr(0)?))
        }
    }
    fn binop(&self) -> Option<(BinaryOp, u8)> {
        Some(match &self.current().kind {
            TokenKind::Keyword(Keyword::Or) => (BinaryOp::Or, 1),
            TokenKind::Keyword(Keyword::And) => (BinaryOp::And, 2),
            TokenKind::Eq => (BinaryOp::Eq, 3),
            TokenKind::Ne => (BinaryOp::Ne, 3),
            TokenKind::Lt => (BinaryOp::Lt, 4),
            TokenKind::Le => (BinaryOp::Le, 4),
            TokenKind::Gt => (BinaryOp::Gt, 4),
            TokenKind::Ge => (BinaryOp::Ge, 4),
            TokenKind::Keyword(Keyword::In) => (BinaryOp::In, 4),
            TokenKind::BitOr => (BinaryOp::BitOr, 5),
            TokenKind::BitXor => (BinaryOp::BitXor, 6),
            TokenKind::BitAnd => (BinaryOp::BitAnd, 7),
            TokenKind::Shl => (BinaryOp::Shl, 8),
            TokenKind::Shr => (BinaryOp::Shr, 8),
            TokenKind::Plus => (BinaryOp::Add, 9),
            TokenKind::Minus => (BinaryOp::Sub, 9),
            TokenKind::Concat => (BinaryOp::Concat, 9),
            TokenKind::Star => (BinaryOp::Mul, 10),
            TokenKind::Slash => (BinaryOp::Div, 10),
            TokenKind::Percent => (BinaryOp::Mod, 10),
            TokenKind::Power => (BinaryOp::Pow, 11),
            _ => return None,
        })
    }
}

fn expression_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.clone()),
        Expr::Member { object, name } => Some(format!("{}.{name}", expression_path(object)?)),
        _ => None,
    }
}
fn expression_pattern(expr: Expr) -> Pattern {
    match expr {
        Expr::Name(n) if n == "_" => Pattern::Wildcard,
        Expr::Name(n) => Pattern::Name(n),
        Expr::Tuple(xs) => Pattern::Tuple(xs.into_iter().map(expression_pattern).collect()),
        Expr::Array(xs) => Pattern::Array(xs.into_iter().map(expression_pattern).collect()),
        Expr::StructInit { name, fields } => Pattern::Struct {
            name,
            fields: fields
                .into_iter()
                .map(|(n, e)| (n, expression_pattern(e)))
                .collect(),
        },
        Expr::Call { callee, args, .. } if matches!(&*callee, Expr::Member { object, .. } if expression_path(object).is_some()) =>
        {
            let Expr::Member { object, name } = *callee else {
                unreachable!()
            };
            let ty = expression_path(&object).unwrap();
            Pattern::Variant {
                name: format!("{ty}.{name}"),
                values: args.into_iter().map(expression_pattern).collect(),
            }
        }
        expr => Pattern::Literal(Box::new(expr)),
    }
}
