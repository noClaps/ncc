use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Clone, Debug)]
pub struct SourceLocation {
    pub path: std::path::PathBuf,
    pub span: Span,
}

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
        location: SourceLocation,
        path: String,
        alias: String,
        functions: Vec<FunctionDecl>,
    },
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias {
        location: SourceLocation,
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
    pub location: SourceLocation,
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<Field>,
}
impl Item {
    pub fn source(&self) -> Option<(&std::path::Path, &Span)> {
        match self {
            Self::Function(f) => Some((&f.source_path, &f.span)),
            Self::Global(v) => Some((&v.source_path, &v.span)),
            Self::Struct(s) => Some((&s.location.path, &s.location.span)),
            Self::Enum(e) => Some((&e.location.path, &e.location.span)),
            Self::TypeAlias { location, .. } | Self::Extern { location, .. } => {
                Some((&location.path, &location.span))
            }
            Self::Statement(statement) => statement.source(),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub location: SourceLocation,
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
    pub symbol: String,
}
#[derive(Clone, Debug)]
pub struct Function {
    pub source_path: std::path::PathBuf,
    pub span: Span,
    pub public: bool,
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Block,
}
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}
#[derive(Clone, Debug)]
pub struct VarDecl {
    pub source_path: std::path::PathBuf,
    pub span: Span,
    pub public: bool,
    pub mutable: bool,
    pub mutex: bool,
    pub pattern: Pattern,
    pub ty: Type,
    pub value: Expr,
}
impl VarDecl {
    pub fn binding_names(&self) -> Vec<&str> {
        fn collect<'a>(pattern: &'a Pattern, names: &mut Vec<&'a str>) {
            match pattern {
                Pattern::Name(name) if name != "_" => names.push(name),
                Pattern::Tuple(patterns) => {
                    for pattern in patterns {
                        collect(pattern, names);
                    }
                }
                _ => {}
            }
        }
        let mut names = Vec::new();
        collect(&self.pattern, &mut names);
        names
    }
}
#[derive(Clone, Debug)]
pub struct Block {
    pub statements: Vec<Stmt>,
}
#[derive(Clone, Debug)]
pub enum Stmt {
    Located(Box<Stmt>, SourceLocation),
    LabeledIf {
        label: String,
        value: Expr,
    },
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
impl Stmt {
    pub fn unlocated(&self) -> &Self {
        match self {
            Self::Located(statement, _) => statement.unlocated(),
            _ => self,
        }
    }
    pub fn unlocated_mut(&mut self) -> &mut Self {
        match self {
            Self::Located(statement, _) => statement.unlocated_mut(),
            _ => self,
        }
    }
    pub fn source(&self) -> Option<(&std::path::Path, &Span)> {
        match self {
            Self::Located(_, location) => Some((&location.path, &location.span)),
            Self::Var(v) => Some((&v.source_path, &v.span)),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Expr {
    Located(Box<Expr>, SourceLocation),
    Bytes(Vec<u8>),
    Embed {
        path: Box<Expr>,
        source_path: std::path::PathBuf,
        span: Span,
    },
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
impl Expr {
    pub fn unlocated(&self) -> &Self {
        match self {
            Self::Located(value, _) => value.unlocated(),
            _ => self,
        }
    }
    pub fn unlocated_mut(&mut self) -> &mut Self {
        match self {
            Self::Located(value, _) => value.unlocated_mut(),
            _ => self,
        }
    }
    pub fn into_unlocated(self) -> Self {
        match self {
            Self::Located(value, _) => value.into_unlocated(),
            value => value,
        }
    }
    pub fn located(self, location: SourceLocation) -> Self {
        Self::Located(Box::new(self.into_unlocated()), location)
    }
    pub fn id(&self) -> usize {
        self.unlocated() as *const Self as usize
    }
    pub fn location(&self) -> Option<&SourceLocation> {
        match self {
            Self::Located(_, location) => Some(location),
            _ => None,
        }
    }
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

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn list(f: &mut std::fmt::Formatter<'_>, types: &[Type]) -> std::fmt::Result {
            for (i, ty) in types.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{ty}")?;
            }
            Ok(())
        }
        match self {
            Self::Named(name, args) => {
                write!(f, "{name}")?;
                if !args.is_empty() {
                    write!(f, "<")?;
                    list(f, args)?;
                    write!(f, ">")?;
                }
                Ok(())
            }
            Self::Array(inner, size) => {
                write!(f, "{inner}[")?;
                if let Some(size) = size {
                    write!(f, "{size}")?;
                }
                write!(f, "]")
            }
            Self::Map(key, value) => write!(f, "[{key}]{value}"),
            Self::Tuple(types) => {
                write!(f, "(")?;
                list(f, types)?;
                write!(f, ")")
            }
            Self::Function(params, result) => {
                write!(f, "(fn(")?;
                list(f, params)?;
                write!(f, ") {result})")
            }
            Self::Optional(inner) => write!(f, "{inner}?"),
            Self::ErrorUnion(inner) => write!(f, "{inner}!"),
            Self::Future(inner) => write!(f, "fut {inner}"),
        }
    }
}
