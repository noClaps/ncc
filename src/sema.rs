use crate::{ast::*, diagnostic::Diagnostics};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct CheckedModule {
    pub module: Module,
    pub types: HashMap<String, TypeInfo>,
}
#[derive(Clone, Debug)]
pub enum TypeInfo {
    Builtin,
    Alias(Type),
    Struct(StructDecl),
    Enum(EnumDecl),
    Function(Function),
}
#[derive(Clone, Debug)]
struct Binding {
    ty: Type,
    mutable: bool,
}
struct Checker {
    types: HashMap<String, TypeInfo>,
    scopes: Vec<HashMap<String, Binding>>,
    function_return: Option<Type>,
    in_test: bool,
    generics: HashSet<String>,
}

pub fn check(module: Module, _path: &Path) -> Result<CheckedModule, Diagnostics> {
    let mut c = Checker::new();
    for item in &module.items {
        c.declare(item)?;
    }
    for item in &module.items {
        c.item(item)?;
    }
    Ok(CheckedModule {
        module,
        types: c.types,
    })
}
impl Checker {
    fn new() -> Self {
        let mut types = HashMap::new();
        for n in [
            "bool", "byte", "char", "int", "uint", "float", "str", "void", "error",
        ] {
            types.insert(n.into(), TypeInfo::Builtin);
        }
        Self {
            types,
            scopes: vec![HashMap::new()],
            function_return: None,
            in_test: false,
            generics: HashSet::new(),
        }
    }
    fn fail<T>(&self, s: impl Into<String>) -> Result<T, Diagnostics> {
        Err(Diagnostics::one(s, 0..0))
    }
    fn declare(&mut self, item: &Item) -> Result<(), Diagnostics> {
        match item {
            Item::Struct(x) => self.add_type(&x.name, TypeInfo::Struct(x.clone()))?,
            Item::Enum(x) => self.add_type(&x.name, TypeInfo::Enum(x.clone()))?,
            Item::TypeAlias { name, ty, .. } => self.add_type(name, TypeInfo::Alias(ty.clone()))?,
            Item::Function(x) => self.add_type(&x.name, TypeInfo::Function(x.clone()))?,
            Item::Global(x) => {
                self.validate_type(&x.ty)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable)?
            }
            Item::Statement(_) => {}
            _ => {}
        }
        Ok(())
    }
    fn add_type(&mut self, n: &str, i: TypeInfo) -> Result<(), Diagnostics> {
        if self.types.insert(n.into(), i).is_some() {
            self.fail(format!("duplicate declaration `{n}`"))
        } else {
            Ok(())
        }
    }
    fn item(&mut self, item: &Item) -> Result<(), Diagnostics> {
        match item {
            Item::Struct(x) => self.with_generics(&x.generics, |this| {
                for f in &x.fields {
                    this.validate_type(&f.ty)?
                }
                Ok(())
            })?,
            Item::Enum(x) => self.with_generics(&x.generics, |this| {
                for v in &x.variants {
                    for t in &v.values {
                        this.validate_type(t)?
                    }
                }
                Ok(())
            })?,
            Item::TypeAlias { ty, .. } => self.validate_type(ty)?,
            Item::Function(x) => self.with_generics(&x.generics, |this| {
                this.push();
                for p in &x.params {
                    this.validate_type(&p.ty)?;
                    this.bind(&p.name, p.ty.clone(), false)?
                }
                this.function_return = Some(x.return_type.clone());
                this.block(&x.body)?;
                this.function_return = None;
                this.pop();
                Ok(())
            })?,
            Item::Global(x) => {
                let got = self.expr(&x.value)?;
                self.assignable(&x.ty, &got)?
            }
            Item::Statement(statement) => self.stmt(statement)?,
            Item::Test { body, .. } => {
                self.push();
                self.in_test = true;
                self.block(body)?;
                self.in_test = false;
                self.pop()
            }
            _ => {}
        }
        Ok(())
    }
    fn validate_type(&self, t: &Type) -> Result<(), Diagnostics> {
        match t {
            Type::Named(n, args) => {
                if !self.types.contains_key(n) && !self.generic_in_scope(n) {
                    return self.fail(format!("unknown type `{n}`"));
                }
                for a in args {
                    self.validate_type(a)?
                }
            }
            Type::Array(x, _) | Type::Optional(x) | Type::ErrorUnion(x) | Type::Future(x) => {
                self.validate_type(x)?
            }
            Type::Map(k, v) => {
                self.validate_type(k)?;
                self.validate_type(v)?
            }
            Type::Tuple(xs) | Type::Function(xs, _) => {
                for x in xs {
                    self.validate_type(x)?
                }
            }
        }
        Ok(())
    }
    fn generic_in_scope(&self, name: &str) -> bool {
        self.generics.contains(name)
    }
    fn with_generics<T>(
        &mut self,
        names: &[String],
        body: impl FnOnce(&mut Self) -> Result<T, Diagnostics>,
    ) -> Result<T, Diagnostics> {
        let old = std::mem::take(&mut self.generics);
        self.generics = names.iter().cloned().collect();
        let result = body(self);
        self.generics = old;
        result
    }
    fn push(&mut self) {
        self.scopes.push(HashMap::new())
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn bind(&mut self, n: &str, ty: Type, mutable: bool) -> Result<(), Diagnostics> {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(n.into(), Binding { ty, mutable });
        Ok(())
    }
    fn bind_pattern(&mut self, p: &Pattern, ty: Type, mutable: bool) -> Result<(), Diagnostics> {
        if let Pattern::Name(n) = p {
            if n != "_" {
                self.bind(n, ty, mutable)?
            }
        }
        Ok(())
    }
    fn lookup(&self, n: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }
    fn block(&mut self, b: &Block) -> Result<(), Diagnostics> {
        self.push();
        for s in &b.statements {
            self.stmt(s)?
        }
        self.pop();
        Ok(())
    }
    fn stmt(&mut self, s: &Stmt) -> Result<(), Diagnostics> {
        match s {
            Stmt::Var(x) => {
                self.validate_type(&x.ty)?;
                let got = self.expr(&x.value)?;
                self.assignable(&x.ty, &got)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable)?
            }
            Stmt::Assign { target, value } => {
                let got = self.expr(value)?;
                let expected = self.lvalue(target)?;
                self.assignable(&expected, &got)?
            }
            Stmt::Expr(x) | Stmt::Assert(x) => {
                let t = self.expr(x)?;
                if matches!(s, Stmt::Assert(_)) && t != named("bool") {
                    return self.fail("assertion requires bool");
                }
            }
            Stmt::Return(x) => {
                let expected = self
                    .function_return
                    .clone()
                    .ok_or_else(|| Diagnostics::one("return outside function", 0..0))?;
                let got = x
                    .as_ref()
                    .map_or(Type::void(), |e| self.expr(e).unwrap_or(Type::void()));
                self.assignable(&expected, &got)?
            }
            Stmt::Throw(x) => {
                let actual = self.expr(x)?;
                self.assignable(&named("str"), &actual)?
            }
            Stmt::For {
                name,
                iterable,
                body,
                ..
            } => {
                let iter = self.expr(iterable)?;
                let key = match iter {
                    Type::Array(_, _) => named("uint"),
                    Type::Map(k, _) => *k,
                    Type::Named(n, _) if n == "str" => named("uint"),
                    _ => return self.fail("for loop expects an array, map, or str"),
                };
                self.push();
                self.bind(name, key, false)?;
                self.block(body)?;
                self.pop()
            }
            Stmt::While {
                condition, body, ..
            } => {
                let actual = self.expr(condition)?;
                self.assignable(&named("bool"), &actual)?;
                self.block(body)?
            }
            Stmt::Lock { name, body, .. } => {
                let b = self
                    .lookup(name)
                    .ok_or_else(|| Diagnostics::one(format!("unknown variable `{name}`"), 0..0))?
                    .clone();
                self.push();
                self.bind(name, b.ty, true)?;
                self.block(body)?;
                self.pop()
            }
            Stmt::Break(_, _) | Stmt::Continue(_) => {}
        }
        Ok(())
    }
    fn lvalue(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        match e {
            Expr::Name(n) => {
                let b = self
                    .lookup(n)
                    .ok_or_else(|| Diagnostics::one(format!("unknown variable `{n}`"), 0..0))?;
                if !b.mutable {
                    return self.fail(format!("cannot mutate immutable `{n}`"));
                }
                Ok(b.ty.clone())
            }
            Expr::Index { object, .. } | Expr::Member { object, .. } => self.expr(object),
            _ => self.fail("invalid assignment target"),
        }
    }
    fn expr(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        match e {
            Expr::Int(s) => Ok(if s.ends_with('u') {
                named("uint")
            } else {
                named("int")
            }),
            Expr::Float(_) => Ok(named("float")),
            Expr::String(_) => Ok(named("str")),
            Expr::Char(_) => Ok(named("char")),
            Expr::Bool(_) => Ok(named("bool")),
            Expr::None => self.fail("cannot infer type of none"),
            Expr::Name(n) => {
                if n.starts_with('@') {
                    return Ok(Type::Function(vec![], Box::new(Type::void())));
                }
                self.lookup(n)
                    .map(|b| b.ty.clone())
                    .or_else(|| match self.types.get(n) {
                        Some(TypeInfo::Function(function)) => Some(Type::Function(
                            function
                                .params
                                .iter()
                                .map(|param| param.ty.clone())
                                .collect(),
                            Box::new(function.return_type.clone()),
                        )),
                        Some(_) => Some(named(n)),
                        None => None,
                    })
                    .ok_or_else(|| Diagnostics::one(format!("unknown name `{n}`"), 0..0))
            }
            Expr::Discard => Ok(Type::void()),
            Expr::Array(xs) => {
                if xs.is_empty() {
                    return self.fail("cannot infer type of empty array");
                };
                let t = self.expr(&xs[0])?;
                for x in &xs[1..] {
                    let actual = self.expr(x)?;
                    self.assignable(&t, &actual)?
                }
                Ok(Type::Array(Box::new(t), None))
            }
            Expr::Map(xs) => {
                if xs.is_empty() {
                    return self.fail("cannot infer type of empty map");
                };
                let (k, v) = &xs[0];
                let kt = self.expr(k)?;
                let vt = self.expr(v)?;
                for (k, v) in &xs[1..] {
                    let actual_key = self.expr(k)?;
                    let actual_value = self.expr(v)?;
                    self.assignable(&kt, &actual_key)?;
                    self.assignable(&vt, &actual_value)?
                }
                Ok(Type::Map(Box::new(kt), Box::new(vt)))
            }
            Expr::Tuple(xs) => Ok(Type::Tuple(
                xs.iter().map(|x| self.expr(x)).collect::<Result<_, _>>()?,
            )),
            Expr::Unary { op, value } => {
                let t = self.expr(value)?;
                match op {
                    UnaryOp::Not => self.assignable(&named("bool"), &t)?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        if !numeric(&t) {
                            return self.fail("numeric unary operation required");
                        }
                    }
                }
                Ok(t)
            }
            Expr::Binary { left, op, right } => {
                let l = self.expr(left)?;
                let r = self.expr(right)?;
                if *op == BinaryOp::In {
                    return Ok(named("bool"));
                }
                self.assignable(&l, &r)?;
                if matches!(
                    op,
                    BinaryOp::Eq
                        | BinaryOp::Ne
                        | BinaryOp::Lt
                        | BinaryOp::Le
                        | BinaryOp::Gt
                        | BinaryOp::Ge
                        | BinaryOp::And
                        | BinaryOp::Or
                ) {
                    Ok(named("bool"))
                } else {
                    Ok(l)
                }
            }
            Expr::Call { callee, args, .. } => {
                if let Expr::Name(name) = &**callee {
                    if matches!(
                        name.as_str(),
                        "@print" | "@println" | "@eprint" | "@eprintln"
                    ) {
                        for arg in args {
                            self.expr(arg)?;
                        }
                        return Ok(Type::void());
                    }
                }
                let callee_t = self.expr(callee)?;
                let Type::Function(params, ret) = callee_t else {
                    return self.fail("called value is not a function");
                };
                if params.len() != args.len() {
                    return self.fail("incorrect number of arguments");
                };
                for (p, a) in params.iter().zip(args) {
                    let actual = self.expr(a)?;
                    self.assignable(p, &actual)?
                }
                Ok(*ret)
            }
            Expr::Index { object, index } => {
                let o = self.expr(object)?;
                let index_type = self.expr(index)?;
                self.assignable(&named("uint"), &index_type)?;
                match o {
                    Type::Array(t, _) => Ok(*t),
                    Type::Tuple(_) => self.fail("tuple index must be a compile-time value"),
                    _ => self.fail("value is not indexable"),
                }
            }
            Expr::Member { object, name } => {
                let o = self.expr(object)?;
                let has_len = matches!(o, Type::Array(_, _) | Type::Map(_, _))
                    || matches!(o, Type::Named(ref n, _) if n == "str");
                if name == "len" && has_len {
                    return Ok(named("uint"));
                }
                self.fail(format!("type does not have member `{name}`"))
            }
            Expr::If { subject, arms } => {
                if let Some(subject) = subject {
                    self.expr(subject)?;
                }
                if arms.is_empty() {
                    return Ok(Type::void());
                }
                let mut result = None;
                for (_, b) in arms {
                    self.block(b)?;
                    let t = Type::void();
                    if let Some(expected) = &result {
                        self.assignable(expected, &t)?
                    } else {
                        result = Some(t)
                    }
                }
                Ok(result.unwrap())
            }
            Expr::Async(x) => Ok(Type::Future(Box::new(self.expr(x)?))),
            Expr::Await(x) => match self.expr(x)? {
                Type::Future(t) => Ok(*t),
                _ => self.fail("await expects a future"),
            },
            Expr::Try(x) => self.expr(x),
            Expr::Else { value, .. } | Expr::Catch { value, .. } => self.expr(value),
            Expr::StructInit { name, .. } => Ok(named(name)),
        }
    }
    fn assignable(&self, expected: &Type, got: &Type) -> Result<(), Diagnostics> {
        if expected == got || matches!(expected,Type::Optional(x)if **x==*got) {
            Ok(())
        } else {
            self.fail(format!("expected `{expected:?}`, found `{got:?}`"))
        }
    }
}
fn named(x: &str) -> Type {
    Type::Named(x.into(), vec![])
}
fn numeric(t: &Type) -> bool {
    matches!(t,Type::Named(n,_)if matches!(n.as_str(),"byte"|"int"|"uint"|"float"))
}
