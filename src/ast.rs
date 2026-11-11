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
        let list = |ts: &[Self]| ts.iter().map(Self::show).collect::<Vec<_>>().join(", ");
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

#[derive(Clone, Debug)]
pub struct Param {
    pub ty: TypeExpr,
    pub name: String,
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
    pub span: Span,
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
    pub fields: Vec<(TypeExpr, String, Span)>,
    pub public: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub variants: Vec<(String, Vec<TypeExpr>, Span)>,
    pub public: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: String,
    pub ty: TypeExpr,
    pub public: bool,
    pub span: Span,
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
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    pub alias: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Import(Import),
    Extern(ExternDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Type(TypeDecl),
    Fn(FnDecl),
    Test(String, Block, Span),
    Stmt(Stmt),
}

#[derive(Clone, Debug)]
pub struct Module {
    pub items: Vec<Item>,
}
