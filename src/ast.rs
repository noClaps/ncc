//! Untyped syntax tree produced by the parser.

use crate::diag::Span;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum TypeExpr {
    /// `int`, `Foo`, `Matrix<int>`, `mod.Foo<T>`
    Named {
        module: Option<String>,
        name: String,
        args: Vec<TypeExpr>,
        span: Span,
    },
    /// `T[]` (None) or `T[N]`
    Array(Box<TypeExpr>, Option<u64>, Span),
    /// `[K]V`
    Map(Box<TypeExpr>, Box<TypeExpr>, Span),
    Optional(Box<TypeExpr>, Span),
    ErrorUnion(Box<TypeExpr>, Span),
    Tuple(Vec<TypeExpr>, Span),
    /// `(fn(int, int) int)`
    Fn(Vec<TypeExpr>, Option<Box<TypeExpr>>, Span),
    Fut(Box<TypeExpr>, Span),
}

#[allow(dead_code)]
impl TypeExpr {
    /// The type as NC source (`int[]`, `[str]int`, `(fn(int) str)`, `Matrix<T>`).
    pub fn show(&self) -> String {
        let list = |ts: &[TypeExpr]| ts.iter().map(|t| t.show()).collect::<Vec<_>>().join(", ");
        match self {
            TypeExpr::Named {
                module, name, args, ..
            } => {
                let m = module.as_ref().map(|m| format!("{m}.")).unwrap_or_default();
                if args.is_empty() {
                    format!("{m}{name}")
                } else {
                    format!("{m}{name}<{}>", list(args))
                }
            }
            TypeExpr::Array(e, Some(n), _) => format!("{}[{n}]", e.show()),
            TypeExpr::Array(e, None, _) => format!("{}[]", e.show()),
            TypeExpr::Map(k, v, _) => format!("[{}]{}", k.show(), v.show()),
            TypeExpr::Optional(i, _) => format!("{}?", i.show()),
            TypeExpr::ErrorUnion(i, _) => format!("{}!", i.show()),
            TypeExpr::Tuple(ts, _) => format!("({})", list(ts)),
            TypeExpr::Fn(ps, Some(r), _) => format!("(fn({}) {})", list(ps), r.show()),
            TypeExpr::Fn(ps, None, _) => format!("(fn({}))", list(ps)),
            TypeExpr::Fut(i, _) => format!("fut {}", i.show()),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. } => *span,
            TypeExpr::Array(_, _, s)
            | TypeExpr::Map(_, _, s)
            | TypeExpr::Optional(_, s)
            | TypeExpr::ErrorUnion(_, s)
            | TypeExpr::Tuple(_, s)
            | TypeExpr::Fn(_, _, s)
            | TypeExpr::Fut(_, s) => *s,
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
#[allow(dead_code)]
pub struct IfExpr {
    pub label: Option<String>,
    pub scrutinee: Option<Expr>,
    pub arms: Vec<Arm>,
    pub span: Span,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
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
    pub type_params: Vec<String>,
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
    pub type_params: Vec<String>,
    pub fields: Vec<(TypeExpr, String, Span)>,
    pub public: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<String>,
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
