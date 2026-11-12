//! Untyped syntax tree produced by the parser.

use crate::diag::Span;

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// `int`, `Foo`, `Matrix<int>`, `mod.Foo<T>`
    Named {
        module: Option<String>,
        name: String,
        args: Vec<Self>,
        span: Span,
    },
    /// `T[]` (None) or `T[N]`, `T[N - 1]`
    Array(Box<Self>, Option<ConstExpr>, Span),
    /// `[K]V`
    Map(Box<Self>, Box<Self>, Span),
    Optional(Box<Self>, Span),
    ErrorUnion(Box<Self>, Span),
    Tuple(Vec<Self>, Span),
    /// `(fn(int, int) int)`
    Fn(Vec<Self>, Option<Box<Self>>, Span),
    Fut(Box<Self>, Span),
    /// A compile-time constant given as a generic argument: `3`, `N + 1`
    Const(ConstExpr),
}

/// A compile-time integer: an array size or a generic constant argument.
#[derive(Clone, Debug)]
pub enum ConstExpr {
    Int(u128, Span),
    /// A generic constant parameter
    Name(String, Span),
    Neg(Box<Self>, Span),
    /// `+ - * / %`
    Bin(BinOp, Box<Self>, Box<Self>),
    Paren(Box<Self>, Span),
}

impl ConstExpr {
    /// The constant as NC source (`N - 1`, `(N + M) * 2`).
    pub fn show(&self) -> String {
        match self {
            Self::Int(n, _) => n.to_string(),
            Self::Name(n, _) => n.clone(),
            Self::Neg(e, _) => format!("-{}", e.show()),
            Self::Bin(op, a, b) => format!("{} {} {}", a.show(), op.symbol(), b.show()),
            Self::Paren(e, _) => format!("({})", e.show()),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Int(_, s) | Self::Name(_, s) | Self::Neg(_, s) | Self::Paren(_, s) => *s,
            Self::Bin(_, a, b) => a.span().to(b.span()),
        }
    }

    /// Call `f` with each name in the constant.
    pub fn visit_names(&self, f: &mut dyn FnMut(&str)) {
        match self {
            Self::Int(..) => {}
            Self::Name(n, _) => f(n),
            Self::Neg(e, _) | Self::Paren(e, _) => e.visit_names(f),
            Self::Bin(_, a, b) => {
                a.visit_names(f);
                b.visit_names(f);
            }
        }
    }

    /// The constant with each name for which `f` gives a constant replaced by it.
    pub fn map_names(&self, f: &mut dyn FnMut(&str) -> Option<Self>) -> Self {
        match self {
            Self::Int(..) => self.clone(),
            Self::Name(n, _) => f(n).unwrap_or_else(|| self.clone()),
            Self::Neg(e, s) => Self::Neg(Box::new(e.map_names(f)), *s),
            Self::Paren(e, s) => Self::Paren(Box::new(e.map_names(f)), *s),
            Self::Bin(op, a, b) => {
                Self::Bin(*op, Box::new(a.map_names(f)), Box::new(b.map_names(f)))
            }
        }
    }
}

/// A generic parameter: `type T`, or a constant such as `uint N`.
#[derive(Clone, Debug)]
pub struct GenericParam {
    pub name: String,
    /// The constant's integer type (`int`, `uint` or `byte`); `None` for a type parameter
    pub konst: Option<String>,
    /// The name's span
    pub span: Span,
}

impl GenericParam {
    /// `type T` or `uint N`
    pub fn show(&self) -> String {
        match &self.konst {
            Some(t) => format!("{t} {}", self.name),
            None => format!("type {}", self.name),
        }
    }

    /// `<type T, uint N>` (empty for no parameters)
    pub fn list(ps: &[Self]) -> String {
        if ps.is_empty() {
            return String::new();
        }
        format!(
            "<{}>",
            ps.iter().map(Self::show).collect::<Vec<_>>().join(", ")
        )
    }
}

impl TypeExpr {
    /// The type as NC source (`int[]`, `[str]int`, `(fn(int) str)`, `Matrix<T>`).
    pub fn show(&self) -> String {
        let list = Self::show_list;
        match self {
            Self::Named {
                module, name, args, ..
            } => {
                let m = module.as_ref().map(|m| format!("{m}.")).unwrap_or_default();
                if args.is_empty() {
                    format!("{m}{name}")
                } else {
                    format!("{m}{name}<{}>", list(args))
                }
            }
            Self::Array(e, Some(n), _) => format!("{}[{}]", e.show(), n.show()),
            Self::Array(e, None, _) => format!("{}[]", e.show()),
            Self::Map(k, v, _) => format!("[{}]{}", k.show(), v.show()),
            Self::Optional(i, _) => format!("{}?", i.show()),
            // a bare `!` return type (parsed with the span of the `!` for its `void`)
            Self::ErrorUnion(i, sp) if i.span() == *sp => "!".into(),
            Self::ErrorUnion(i, _) => format!("{}!", i.show()),
            Self::Tuple(ts, _) => format!("({})", list(ts)),
            Self::Fn(ps, Some(r), _) => format!("(fn({}) {})", list(ps), r.show()),
            Self::Fn(ps, None, _) => format!("(fn({}))", list(ps)),
            Self::Fut(i, _) => format!("fut {}", i.show()),
            Self::Const(c) => c.show(),
        }
    }

    /// Types as NC source, separated by commas.
    pub fn show_list(ts: &[Self]) -> String {
        ts.iter().map(Self::show).collect::<Vec<_>>().join(", ")
    }

    /// The types directly inside this one.
    pub fn children(&self) -> Vec<&Self> {
        match self {
            Self::Named { args, .. } | Self::Tuple(args, _) => args.iter().collect(),
            Self::Array(e, ..)
            | Self::Optional(e, _)
            | Self::ErrorUnion(e, _)
            | Self::Fut(e, _) => {
                vec![e]
            }
            Self::Map(k, v, _) => vec![k, v],
            Self::Fn(ps, r, _) => ps.iter().chain(r.as_deref()).collect(),
            Self::Const(_) => vec![],
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } => *span,
            Self::Array(_, _, s)
            | Self::Map(_, _, s)
            | Self::Optional(_, s)
            | Self::ErrorUnion(_, s)
            | Self::Tuple(_, s)
            | Self::Fn(_, _, s)
            | Self::Fut(_, s) => *s,
            Self::Const(c) => c.span(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    In,
    And,
    Or,
}

impl BinOp {
    /// Precedence of a binary operator: higher binds tighter.
    pub fn prec(self) -> u8 {
        use BinOp::*;
        match self {
            Or => 1,
            And => 2,
            Eq | Ne | Lt | Gt | Le | Ge | In => 3,
            Concat => 4,
            BitOr => 5,
            BitXor => 6,
            BitAnd => 7,
            Shl | Shr => 8,
            Add | Sub => 9,
            Mul | Div | Mod => 10,
            Pow => 11,
        }
    }

    pub fn symbol(self) -> &'static str {
        use BinOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Mod => "%",
            Pow => "**",
            Shl => "<<",
            Shr => ">>",
            BitAnd => "&",
            BitOr => "|",
            BitXor => "^",
            Concat => "<>",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Gt => ">",
            Le => "<=",
            Ge => ">=",
            In => "in",
            And => "and",
            Or => "or",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    BitNot,
    Not,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::BitNot => "!",
            Self::Not => "not",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ElseFallback {
    Expr(Box<Expr>),
    Block(Block),
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(u128, bool),
    Float(f64),
    Char(u32),
    Str(Vec<StrSeg>),
    Bool(bool),
    None,
    Ident(String),
    /// `_` (only valid in patterns)
    Wildcard,
    Dollar,
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// callee, explicit generic args, args
    Call(Box<Expr>, Vec<Expr>),
    /// `name<T, U>` reference to a generic function or type
    Generic(Box<Expr>, Vec<TypeExpr>),
    Builtin(String, Option<TypeExpr>, Vec<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// `x[a:b]`, both bounds inclusive
    Slice(Box<Expr>, Box<Expr>, Box<Expr>),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    /// `Type{.a = x, .b = y}` -- the type is given as an expression path (`Foo`, `Foo<int>`, `m.Foo`)
    Struct(Box<Expr>, Vec<(String, Expr, Span)>),
    FnLit(Box<FnSig>, Block),
    If(Box<IfExpr>),
    Else(Box<Expr>, ElseFallback),
    /// value, error variable name and its span, handler
    Catch(Box<Expr>, String, Span, Block),
    Try(Box<Expr>),
    Async(Box<Expr>),
    Await(Box<Expr>),
    /// A value that failed to parse (the error has already been reported)
    Error,
}

#[derive(Clone, Debug)]
pub enum StrSeg {
    Lit(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct IfExpr {
    pub label: Option<String>,
    pub scrutinee: Option<Expr>,
    pub arms: Vec<Arm>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Arm {
    /// Comma-separated alternatives; `_` is `ExprKind::Wildcard`
    pub patterns: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// A function parameter or struct field: `T name`, spanning the name.
#[derive(Clone, Debug)]
pub struct Param {
    pub ty: TypeExpr,
    pub name: String,
    pub span: Span,
}

impl Param {
    /// `T name`
    pub fn show(&self) -> String {
        format!("{} {}", self.ty.show(), self.name)
    }
}

pub type Field = Param;

/// An enum member: `Name` or `Name(T, U)`, spanning the name.
#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    pub payload: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnSig {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub sig: FnSig,
    pub body: Block,
    pub public: bool,
}

#[derive(Clone, Debug)]
pub struct DeclTarget {
    pub ty: TypeExpr,
    /// `_` allowed for discarding parts
    pub name: String,
    /// the whole target, type and name
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    /// `[pub] [mut|mutex] T a [, U b] = value`
    VarDecl {
        public: bool,
        mutable: bool,
        mutex: bool,
        targets: Vec<DeclTarget>,
        value: Expr,
    },
    /// `fn f = fn(...) R { ... }`
    FnVar {
        name: String,
        name_span: Span,
        value: Expr,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    Discard(Expr),
    Expr(Expr),
    Return(Option<Expr>),
    Throw(Expr),
    Break {
        label: Option<String>,
        value: Option<Expr>,
    },
    Continue(Option<String>),
    While {
        label: Option<String>,
        cond: Expr,
        body: Block,
    },
    For {
        label: Option<String>,
        var: String,
        var_span: Span,
        iter: Expr,
        body: Block,
    },
    Lock {
        label: Option<String>,
        name: String,
        name_span: Span,
        body: Block,
    },
    Block(Block),
    Fn(FnDecl),
    Assert(Expr),
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub fields: Vec<Field>,
    pub public: bool,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub variants: Vec<Variant>,
    pub public: bool,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub public: bool,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct ExternFn {
    pub sig: FnSig,
    pub symbol: String,
}

#[derive(Clone, Debug)]
pub struct ExternDecl {
    pub path: String,
    pub alias: String,
    pub fns: Vec<ExternFn>,
    pub span: Span,
    pub alias_span: Span,
}

/// `test "name" { ... }`
#[derive(Clone, Debug)]
pub struct Test {
    pub name: String,
    pub body: Block,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    pub alias: String,
    pub span: Span,
    pub alias_span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Import(Import),
    Extern(ExternDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Type(TypeDecl),
    Fn(FnDecl),
    Test(Test),
    Stmt(Stmt),
}

impl Item {
    /// The names this item declares in its module, and whether they are public.
    pub fn declared(&self) -> (Vec<&String>, bool) {
        match self {
            Self::Fn(f) => (vec![&f.sig.name], f.public),
            Self::Struct(s) => (vec![&s.name], s.public),
            Self::Enum(e) => (vec![&e.name], e.public),
            Self::Type(t) => (vec![&t.name], t.public),
            Self::Stmt(Stmt {
                kind: StmtKind::VarDecl {
                    public, targets, ..
                },
                ..
            }) => (targets.iter().map(|t| &t.name).collect(), *public),
            _ => (vec![], false),
        }
    }

    /// The names this item exports (`pub`).
    pub fn public_names(&self) -> Vec<&String> {
        match self.declared() {
            (names, true) => names,
            _ => vec![],
        }
    }

    /// The source range of the whole item.
    pub fn range(&self) -> (u32, u32) {
        match self {
            Self::Import(i) => (i.span.start, i.span.end),
            Self::Extern(e) => (e.span.start, e.span.end),
            Self::Struct(s) => (s.span.start, s.span.end),
            Self::Enum(e) => (e.span.start, e.span.end),
            Self::Type(t) => (t.span.start, t.span.end),
            Self::Fn(f) => (f.sig.span.start, f.body.span.end),
            Self::Test(t) => (t.span.start, t.body.span.end.max(t.span.end)),
            Self::Stmt(s) => (s.span.start, s.span.end),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Module {
    pub items: Vec<Item>,
}

impl Module {
    /// The module's imports, in source order.
    pub fn imports(&self) -> impl Iterator<Item = &Import> {
        (self.items.iter()).filter_map(|it| match it {
            Item::Import(im) => Some(im),
            _ => None,
        })
    }
}

/// Walks over the syntax tree.
pub mod visit {
    use super::{ExprKind as E, StmtKind as S, *};

    /// A node of the syntax tree, visited by `walk`. `Span` is reported for every node.
    #[derive(Clone, Copy)]
    pub enum Node<'a> {
        Span(Span),
        Stmt(&'a Stmt),
        Expr(&'a Expr),
        Block(&'a Block),
        /// a function (declared or literal): its signature and body
        Fn(&'a FnSig, &'a Block),
    }

    /// Visit every node of a module.
    pub fn walk<'a>(m: &'a Module, out: &mut dyn FnMut(Node<'a>)) {
        for it in &m.items {
            item_spans(it, out);
        }
    }

    pub fn item_spans<'a>(it: &'a Item, out: &mut dyn FnMut(Node<'a>)) {
        match it {
            Item::Import(im) => out(Node::Span(im.span)),
            Item::Extern(e) => {
                out(Node::Span(e.span));
                for f in &e.fns {
                    sig_spans(&f.sig, out);
                }
            }
            Item::Struct(s) => {
                out(Node::Span(s.span));
                params_spans(&s.fields, out);
            }
            Item::Enum(e) => {
                out(Node::Span(e.span));
                for v in &e.variants {
                    out(Node::Span(v.span));
                    v.payload.iter().for_each(|t| type_spans(t, out));
                }
            }
            Item::Type(t) => {
                out(Node::Span(t.span));
                type_spans(&t.ty, out);
            }
            Item::Fn(f) => fn_spans(f, out),
            Item::Test(t) => {
                out(Node::Span(t.span));
                block_spans(&t.body, out);
            }
            Item::Stmt(s) => stmt_spans(s, out),
        }
    }

    fn fn_spans<'a>(f: &'a FnDecl, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Fn(&f.sig, &f.body));
        out(Node::Span(f.sig.span.to(f.body.span)));
        sig_spans(&f.sig, out);
        block_spans(&f.body, out);
    }

    fn sig_spans<'a>(s: &'a FnSig, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Span(s.span));
        params_spans(&s.params, out);
        if let Some(r) = &s.ret {
            type_spans(r, out);
        }
    }

    fn params_spans<'a>(ps: &'a [Param], out: &mut dyn FnMut(Node<'a>)) {
        for p in ps {
            out(Node::Span(p.span));
            type_spans(&p.ty, out);
        }
    }

    fn type_spans<'a>(t: &'a TypeExpr, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Span(t.span()));
        for c in t.children() {
            type_spans(c, out);
        }
    }

    pub fn block_spans<'a>(b: &'a Block, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Span(b.span));
        out(Node::Block(b));
        for s in &b.stmts {
            stmt_spans(s, out);
        }
    }

    pub fn stmt_spans<'a>(s: &'a Stmt, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Span(s.span));
        out(Node::Stmt(s));
        match &s.kind {
            S::VarDecl { targets, value, .. } => {
                for t in targets {
                    out(Node::Span(t.span));
                    type_spans(&t.ty, out);
                }
                expr_spans(value, out);
            }
            S::FnVar { value, .. } | S::Discard(value) | S::Expr(value) | S::Throw(value) => {
                expr_spans(value, out);
            }
            S::Assert(e) => expr_spans(e, out),
            S::Assign { target, value } => {
                expr_spans(target, out);
                expr_spans(value, out);
            }
            S::Return(v) | S::Break { value: v, .. } => {
                if let Some(v) = v {
                    expr_spans(v, out);
                }
            }
            S::Continue(_) => {}
            S::While { cond, body, .. } => {
                expr_spans(cond, out);
                block_spans(body, out);
            }
            S::For {
                var_span,
                iter,
                body,
                ..
            } => {
                out(Node::Span(*var_span));
                expr_spans(iter, out);
                block_spans(body, out);
            }
            S::Lock {
                name_span, body, ..
            } => {
                out(Node::Span(*name_span));
                block_spans(body, out);
            }
            S::Block(b) => block_spans(b, out),
            S::Fn(f) => fn_spans(f, out),
        }
    }

    pub fn expr_spans<'a>(e: &'a Expr, out: &mut dyn FnMut(Node<'a>)) {
        out(Node::Span(e.span));
        out(Node::Expr(e));
        match &e.kind {
            E::Unary(_, x) | E::Try(x) | E::Async(x) | E::Await(x) | E::Field(x, _) => {
                expr_spans(x, out)
            }
            E::Binary(_, a, b) | E::Index(a, b) => {
                expr_spans(a, out);
                expr_spans(b, out);
            }
            E::Call(c, args) => {
                expr_spans(c, out);
                for a in args {
                    expr_spans(a, out);
                }
            }
            E::Generic(x, ts) => {
                expr_spans(x, out);
                for t in ts {
                    type_spans(t, out);
                }
            }
            E::Builtin(_, t, args) => {
                if let Some(t) = t {
                    type_spans(t, out);
                }
                for a in args {
                    expr_spans(a, out);
                }
            }
            E::Tuple(xs) | E::Array(xs) => {
                for x in xs {
                    expr_spans(x, out);
                }
            }
            E::Map(kvs) => {
                for (k, v) in kvs {
                    expr_spans(k, out);
                    expr_spans(v, out);
                }
            }
            E::Struct(t, fields) => {
                expr_spans(t, out);
                for (_, v, sp) in fields {
                    out(Node::Span(*sp));
                    expr_spans(v, out);
                }
            }
            E::FnLit(sig, b) => {
                out(Node::Fn(sig, b));
                sig_spans(sig, out);
                block_spans(b, out);
            }
            E::If(i) => {
                out(Node::Span(i.span));
                if let Some(s) = &i.scrutinee {
                    expr_spans(s, out);
                }
                for arm in &i.arms {
                    out(Node::Span(arm.span));
                    for p in &arm.patterns {
                        expr_spans(p, out);
                    }
                    block_spans(&arm.body, out);
                }
            }
            E::Else(x, fb) => {
                expr_spans(x, out);
                match fb {
                    ElseFallback::Expr(y) => expr_spans(y, out),
                    ElseFallback::Block(b) => block_spans(b, out),
                }
            }
            E::Catch(x, _, sp, b) => {
                expr_spans(x, out);
                out(Node::Span(*sp));
                block_spans(b, out);
            }
            E::Str(segs) => {
                for s in segs {
                    if let StrSeg::Expr(x) = s {
                        expr_spans(x, out);
                    }
                }
            }
            _ => {}
        }
    }
}
