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
pub enum Lit {
    Bool(bool),
    Byte(u8),
    Int(i64),
    UInt(u64),
    Float(f64),
    Char(u32),
    Str(String),
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
    /// Array / fixed array / str slice `x[lo:hi]`, bounds inclusive. `$` in the bounds is this base's.
    Slice(Box<Expr>, Box<Expr>, Box<Expr>),
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
    Tuple(Vec<Self>),
    Struct(Vec<Self>),
    /// Array pattern: matches arrays of exactly this many elements
    Array(Vec<Self>),
    Variant(usize, Vec<Self>),
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Place {
    Var(VarRef),
    Field(Box<Self>, usize),
    TupleIndex(Box<Self>, usize),
    Index(Box<Self>, Box<Expr>),
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

#[derive(Clone, Debug, Default)]
pub struct Func {
    pub name: String,
    pub params: Vec<LocalId>,
    pub ret: TyId,
    pub locals: Vec<Local>,
    pub captures: Vec<Capture>,
    pub body: Block,
    /// Index of the parent function for nested functions
    pub parent: Option<FuncId>,
    /// Local in this function that refers to the closure itself (recursive nested fns)
    pub self_local: Option<LocalId>,
}

#[derive(Clone, Debug)]
pub struct Global {
    pub name: String,
    pub ty: TyId,
    pub mutable: bool,
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
    /// Source file of the entry module, once release tree shaking has run (see `shake`)
    pub entry_file: Option<u32>,
}

/// The sub-expressions of an expression kind, outside its nested blocks and `if` patterns.
/// `$opt` is `as_ref` or `as_mut`, matching how `$kind` is borrowed.
macro_rules! for_each_child {
    ($kind:expr, $f:ident, $opt:ident) => {
        match $kind {
            EK::Lit(_)
            | EK::None
            | EK::Var(_)
            | EK::FnRef(_)
            | EK::Closure(_)
            | EK::Dollar
            | EK::Args
            | EK::Env
            | EK::Target
            | EK::Embed(..) => {}
            EK::Unary(_, a)
            | EK::Cast(a)
            | EK::ToStr(a)
            | EK::Field(a, _)
            | EK::TupleIndex(a, _)
            | EK::Len(a)
            | EK::Some(a)
            | EK::Retype(a)
            | EK::Try(a)
            | EK::Await(a)
            | EK::Catch(a, ..) => $f(a),
            EK::Binary(_, a, b) | EK::Index(a, b) => {
                $f(a);
                $f(b);
            }
            EK::Slice(a, lo, hi) => {
                $f(a);
                $f(lo);
                $f(hi);
            }
            EK::Call(c, xs) | EK::Async(c, xs) => {
                if let Callee::Value(x) = c {
                    $f(x);
                }
                for x in xs {
                    $f(x);
                }
            }
            EK::Print(_, _, xs)
            | EK::Format(xs)
            | EK::Tuple(xs)
            | EK::Array(xs)
            | EK::Struct(xs)
            | EK::Variant(_, xs) => {
                for x in xs {
                    $f(x);
                }
            }
            EK::Map(es) => {
                for (k, v) in es {
                    $f(k);
                    $f(v);
                }
            }
            EK::If(i) => {
                if let Some(s) = i.scrutinee.$opt() {
                    $f(s);
                }
            }
            EK::Else(a, fb) => {
                $f(a);
                if let Fallback::Expr(b) = fb {
                    $f(b);
                }
            }
        }
    };
}

impl Expr {
    pub fn new(kind: EK, ty: TyId, span: Span) -> Self {
        Self { kind, ty, span }
    }

    /// Apply `f` to each direct sub-expression, not those in nested blocks or `if` patterns.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        for_each_child!(&self.kind, f, as_ref)
    }

    /// Like `for_each_child`, with mutable access.
    pub fn for_each_child_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        for_each_child!(&mut self.kind, f, as_mut)
    }
}

impl Pat {
    /// Apply `f` to each expression in the pattern.
    pub fn for_each_expr<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        match self {
            Pat::Wild | Pat::Bind(_) => {}
            Pat::Value(e) | Pat::Cond(e) => f(e),
            Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
                ps.iter().for_each(|p| p.for_each_expr(f));
            }
        }
    }

    /// Like `for_each_expr`, with mutable access.
    pub fn for_each_expr_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        match self {
            Pat::Wild | Pat::Bind(_) => {}
            Pat::Value(e) | Pat::Cond(e) => f(e),
            Pat::Tuple(ps) | Pat::Struct(ps) | Pat::Array(ps) | Pat::Variant(_, ps) => {
                ps.iter_mut().for_each(|p| p.for_each_expr_mut(f));
            }
        }
    }
}

/// The expressions of a statement itself (including indices of its target), not those of the
/// statements nested in it. `$iter` is `iter` or `iter_mut`, matching how `$kind` is borrowed.
macro_rules! own_exprs {
    ($kind:expr, $iter:ident) => {
        match $kind {
            SK::Let(_, e)
            | SK::Destructure(_, e)
            | SK::Expr(e)
            | SK::Throw(e)
            | SK::Assert(e)
            | SK::While { cond: e, .. }
            | SK::For { iter: e, .. }
            | SK::Lock { mutex: e, .. } => vec![e],
            SK::Return(e) | SK::Break(_, e) => e.$iter().collect(),
            SK::Assign(pl, e) => {
                // the value, then the indices from the outermost place in: reversed, the
                // evaluation order
                let mut pl = pl;
                let mut out = vec![e];
                loop {
                    match pl {
                        Place::Var(_) => break,
                        Place::Field(b, _) | Place::TupleIndex(b, _) => pl = b,
                        Place::Index(b, i) => {
                            out.push(i);
                            pl = b;
                        }
                    }
                }
                out.reverse();
                out
            }
            SK::Continue(_) | SK::Block(_) => vec![],
        }
    };
}

impl Stmt {
    /// The statement's own expressions in evaluation order: not those in its body or in the
    /// blocks of its expressions.
    pub fn own_exprs(&self) -> Vec<&Expr> {
        own_exprs!(&self.kind, iter)
    }

    /// Like `own_exprs`, with mutable access.
    pub fn own_exprs_mut(&mut self) -> Vec<&mut Expr> {
        own_exprs!(&mut self.kind, iter_mut)
    }

    /// The body of a loop, `lock` or block statement.
    pub fn body(&self) -> Option<&Block> {
        match &self.kind {
            SK::While { body, .. }
            | SK::For { body, .. }
            | SK::Lock { body, .. }
            | SK::Block(body) => Some(body),
            _ => None,
        }
    }

    /// Like `body`, with mutable access.
    pub fn body_mut(&mut self) -> Option<&mut Block> {
        match &mut self.kind {
            SK::While { body, .. }
            | SK::For { body, .. }
            | SK::Lock { body, .. }
            | SK::Block(body) => Some(body),
            _ => None,
        }
    }
}
