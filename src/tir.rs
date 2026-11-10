//! Typed intermediate representation, produced by the checker and consumed by the
//! evaluator, optimizer and C backend.

use crate::ast::{BinOp, UnOp};
use crate::diag::Span;
use crate::types::{TyId, Types};

pub type FuncId = usize;
pub type LocalId = usize;
pub type GlobalId = usize;
pub type ExternId = usize;
/// Identifier of a breakable construct (loop, labelled if/lock, value context).
pub type JumpId = usize;

#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub enum Lit {
    Bool(bool),
    Byte(u8),
    Int(i64),
    UInt(u64),
    Float(f64),
    Char(u32),
    Str(String),
    Void,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarRef {
    Local(LocalId),
    Global(GlobalId),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: EK,
    pub ty: TyId,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintTo {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug)]
pub enum Callee {
    Direct(FuncId),
    Extern(ExternId),
    Value(Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum EK {
    Lit(Lit),
    /// `none` of the optional type `ty`
    None,
    Var(VarRef),
    /// Reference to a top-level function as a value
    FnRef(FuncId),
    /// Create a closure for a nested function / function literal, capturing per `Func::captures`
    Closure(FuncId),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(Callee, Vec<Expr>),
    Print(PrintTo, bool, Vec<Expr>),
    /// `@as` conversion to `ty`
    Cast(Box<Expr>),
    /// String conversion (format strings)
    ToStr(Box<Expr>),
    /// Interpolated string: concatenation of str parts
    Format(Vec<Expr>),
    Field(Box<Expr>, usize),
    TupleIndex(Box<Expr>, usize),
    /// Array / fixed array / str / map index. `$` inside the index refers to this base.
    Index(Box<Expr>, Box<Expr>),
    /// `$` of the innermost enclosing index: len - 1
    Dollar,
    Len(Box<Expr>),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Struct(Vec<Expr>),
    /// Enum constructor: variant index and payload
    Variant(usize, Vec<Expr>),
    /// T -> T?
    Some(Box<Expr>),
    /// Fixed array -> dynamic array (same representation, type change only)
    Retype(Box<Expr>),
    If(Box<If>),
    /// `opt else ...`
    Else(Box<Expr>, Fallback),
    /// `x catch name { ... }`
    Catch(Box<Expr>, Option<LocalId>, Block, JumpId),
    Try(Box<Expr>),
    Async(Callee, Vec<Expr>),
    Await(Box<Expr>),
    /// `@args()`: the command line arguments, program name first
    Args,
    /// `@env()`: a snapshot of the environment variables
    Env,
    /// `@target()`: `(os, cpu)` of the compilation target
    Target,
    /// `@embed(path)`: the file's bytes, read at compile time
    Embed(std::rc::Rc<Vec<u8>>, std::path::PathBuf),
}

#[derive(Clone, Debug)]
pub enum Fallback {
    Expr(Box<Expr>),
    Block(Block, JumpId),
}

#[derive(Clone, Debug)]
pub struct If {
    pub id: JumpId,
    /// Whether the if produces a value (via `break value`)
    pub is_value: bool,
    pub scrutinee: Option<Expr>,
    pub arms: Vec<Arm>,
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub patterns: Vec<Pat>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Bind(LocalId),
    /// Compare with `==`
    Value(Expr),
    /// Bare `if` arm: a boolean condition
    Cond(Expr),
    Tuple(Vec<Pat>),
    Struct(Vec<Pat>),
    /// Array pattern: matches arrays of exactly this many elements
    Array(Vec<Pat>),
    Variant(usize, Vec<Pat>),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Place {
    Var(VarRef),
    Field(Box<Place>, usize),
    TupleIndex(Box<Place>, usize),
    Index(Box<Place>, Box<Expr>),
}

#[derive(Clone, Debug)]
pub enum ForKind {
    /// Arrays, fixed arrays and strings: index loop (uint)
    Index,
    /// Maps: iterate over a snapshot of the keys
    Keys,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: SK,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum SK {
    /// Declare and initialise a variable (None = discarded `_`).
    Let(Option<VarRef>, Expr),
    /// Tuple destructuring: (target, first element, element count). A count of 1 binds the
    /// element itself; a larger count binds a sub-tuple of that many elements.
    Destructure(Vec<(Option<VarRef>, usize, usize)>, Expr),
    Assign(Place, Expr),
    Expr(Expr),
    Return(Option<Expr>),
    Throw(Expr),
    Break(JumpId, Option<Expr>),
    Continue(JumpId),
    While {
        id: JumpId,
        cond: Expr,
        body: Block,
    },
    For {
        id: JumpId,
        var: LocalId,
        kind: ForKind,
        iter: Expr,
        body: Block,
    },
    /// `lock`: the mutex variable and the local alias bound inside the body
    Lock {
        id: JumpId,
        mutex: Expr,
        alias: LocalId,
        body: Block,
    },
    Block(Block),
    Assert(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    Normal,
    Param,
    /// Captured by value from the enclosing function (index into captures)
    CapVal(usize),
    /// Captured by reference (boxed cell) from the enclosing function
    CapRef(usize),
    /// `lock` alias: pointer to the mutex payload
    LockAlias,
}

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: TyId,
    pub mutable: bool,
    pub kind: LocalKind,
    /// Mutable local captured by reference by some nested function: stored in a heap cell
    pub boxed: bool,
}

#[derive(Clone, Debug)]
pub struct Capture {
    /// Variable in the enclosing function
    pub outer: LocalId,
    pub by_ref: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Func {
    pub name: String,
    pub params: Vec<LocalId>,
    pub ret: TyId,
    pub locals: Vec<Local>,
    pub captures: Vec<Capture>,
    pub body: Block,
    pub span: Span,
    /// Index of the parent function for nested functions
    pub parent: Option<FuncId>,
    /// Local in this function that refers to the closure itself (recursive nested fns)
    pub self_local: Option<LocalId>,
    pub is_never: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Global {
    pub name: String,
    pub ty: TyId,
    pub mutable: bool,
    pub module: usize,
}

#[derive(Clone, Debug)]
pub struct Extern {
    pub name: String,
    pub symbol: String,
    pub params: Vec<TyId>,
    pub ret: TyId,
    /// C file implementing the function
    pub file: std::path::PathBuf,
}

#[derive(Clone, Debug)]
pub struct Test {
    pub name: String,
    pub func: FuncId,
}

pub struct Program {
    pub types: Types,
    pub funcs: Vec<Func>,
    pub globals: Vec<Global>,
    pub externs: Vec<Extern>,
    /// The program entry: a function running the module initialisers and the entry module's top level.
    pub main: FuncId,
    pub tests: Vec<Test>,
    /// `(os, cpu)` of the C compiler's target, when known (see `@target`)
    pub target: Option<(String, String)>,
}
