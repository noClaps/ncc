use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Clone, Debug)]
pub struct Module {
    pub items: Vec<Item>,
}
#[derive(Clone, Debug)]
pub enum Item {
    Import {
        path: String,
        alias: String,
    },
    Extern {
        path: String,
        alias: String,
        functions: Vec<FunctionDecl>,
    },
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias {
        public: bool,
        name: String,
        ty: Type,
    },
    Function(Function),
    Global(VarDecl),
    Statement(Stmt),
    Test {
        name: String,
        body: Block,
    },
}
#[derive(Clone, Debug)]
pub struct StructDecl {
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}
#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    pub values: Vec<Type>,
}
#[derive(Clone, Debug)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub throws: bool,
    pub symbol: String,
}
#[derive(Clone, Debug)]
pub struct Function {
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub throws: bool,
    pub body: Block,
}
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}
#[derive(Clone, Debug)]
pub struct VarDecl {
    pub public: bool,
    pub mutable: bool,
    pub mutex: bool,
    pub pattern: Pattern,
    pub ty: Type,
    pub value: Expr,
}
#[derive(Clone, Debug)]
pub struct Block {
    pub statements: Vec<Stmt>,
}
#[derive(Clone, Debug)]
pub enum Stmt {
    Block(Block),
    Var(VarDecl),
    Assign {
        target: Expr,
        value: Expr,
    },
    Expr(Expr),
    Return(Option<Expr>),
    Throw(Expr),
    Break(Option<Expr>, Option<String>),
    Continue(Option<String>),
    Assert(Expr),
    For {
        label: Option<String>,
        name: String,
        iterable: Expr,
        body: Block,
    },
    While {
        label: Option<String>,
        condition: Expr,
        body: Block,
    },
    Lock {
        label: Option<String>,
        name: String,
        body: Block,
    },
}
#[derive(Clone, Debug)]
pub enum Expr {
    Lambda(Box<Function>),
    Cast {
        ty: Type,
        value: Box<Expr>,
    },
    Int(String),
    Float(String),
    String(String),
    Char(String),
    Bool(bool),
    None,
    Name(String),
    Discard,
    Array(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Tuple(Vec<Expr>),
    StructInit {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        generics: Vec<Type>,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Member {
        object: Box<Expr>,
        name: String,
    },
    Unary {
        op: UnaryOp,
        value: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    If {
        subject: Option<Box<Expr>>,
        arms: Vec<(Vec<Pattern>, Block)>,
    },
    Async(Box<Expr>),
    Await(Box<Expr>),
    Try(Box<Expr>),
    Else {
        value: Box<Expr>,
        fallback: Block,
    },
    Catch {
        value: Box<Expr>,
        name: String,
        body: Block,
    },
}
#[derive(Clone, Debug)]
pub enum Pattern {
    Wildcard,
    Name(String),
    Literal(Box<Expr>),
    Tuple(Vec<Pattern>),
    Array(Vec<Pattern>),
    Variant {
        name: String,
        values: Vec<Pattern>,
    },
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    In,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Named(String, Vec<Type>),
    Array(Box<Type>, Option<usize>),
    Map(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
    Function(Vec<Type>, Box<Type>),
    Optional(Box<Type>),
    ErrorUnion(Box<Type>),
    Future(Box<Type>),
}
impl Type {
    pub fn void() -> Self {
        Self::Named("void".into(), vec![])
    }
}
