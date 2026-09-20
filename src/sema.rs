use crate::{ast::*, diagnostic::Diagnostics};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug)]
pub struct CheckedModule {
    pub module: Module,
    pub types: HashMap<String, TypeInfo>,
    pub expression_types: HashMap<usize, Type>,
    pub captures: HashMap<usize, Vec<(String, Type)>>,
}
#[derive(Clone, Debug)]
pub enum TypeInfo {
    External(FunctionDecl),
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
    expression_types: HashMap<usize, Type>,
    loops: Vec<Option<String>>,
    indexing: usize,
    value_targets: Vec<Type>,
    capture_frames: Vec<(usize, HashMap<String, Type>)>,
    captures: HashMap<usize, Vec<(String, Type)>>,
}

pub fn check(module: Module, _path: &Path) -> Result<CheckedModule, Diagnostics> {
    let mut c = Checker::new();
    for item in &module.items {
        c.declare(item)?;
    }
    for item in &module.items {
        if !matches!(item, Item::Function(_)) {
            c.item(item)?;
        }
    }
    for item in &module.items {
        if matches!(item, Item::Function(_)) {
            c.item(item)?;
        }
    }
    Ok(CheckedModule {
        module,
        types: c.types,
        expression_types: c.expression_types,
        captures: c.captures,
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
            expression_types: HashMap::new(),
            loops: vec![],
            indexing: 0,
            value_targets: vec![],
            capture_frames: vec![],
            captures: HashMap::new(),
        }
    }
    fn fail<T>(&self, s: impl Into<String>) -> Result<T, Diagnostics> {
        Err(Diagnostics::one(s, 0..0))
    }
    fn declare(&mut self, item: &Item) -> Result<(), Diagnostics> {
        match item {
            Item::Extern { functions, .. } => {
                for function in functions {
                    self.add_type(&function.name, TypeInfo::External(function.clone()))?;
                }
            }
            Item::Struct(x) => self.add_type(&x.name, TypeInfo::Struct(x.clone()))?,
            Item::Enum(x) => self.add_type(&x.name, TypeInfo::Enum(x.clone()))?,
            Item::TypeAlias { name, ty, .. } => self.add_type(name, TypeInfo::Alias(ty.clone()))?,
            Item::Function(x) => self.add_type(&x.name, TypeInfo::Function(x.clone()))?,
            Item::Global(_) => {}
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
            Item::TypeAlias { name, ty, .. } => {
                self.validate_type(ty)?;
                let mut seen = HashSet::new();
                let mut current = named(name);
                while let Type::Named(n, _) = current {
                    if !seen.insert(n.clone()) {
                        return self.fail("cyclic nominal type definition");
                    }
                    if let Some(TypeInfo::Alias(base)) = self.types.get(&n) {
                        current = base.clone();
                    } else {
                        break;
                    }
                }
            }
            Item::Function(x) => self.with_generics(&x.generics, |this| {
                this.validate_type(&x.return_type)?;
                this.push();
                for p in &x.params {
                    this.validate_type(&p.ty)?;
                    this.bind(&p.name, p.ty.clone(), false)?
                }
                this.function_return = Some(x.return_type.clone());
                this.block(&x.body)?;
                if x.return_type != Type::void()
                    && x.return_type != Type::ErrorUnion(Box::new(Type::void()))
                    && !returns(&x.body)
                {
                    return this.fail(format!(
                        "function `{}` may finish without returning a value",
                        x.name
                    ));
                }
                this.function_return = None;
                this.pop();
                Ok(())
            })?,
            Item::Global(x) => {
                self.validate_type(&x.ty)?;
                self.expected(&x.value, &x.ty)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable)?;
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
                if let Type::Function(_, ret) = t {
                    self.validate_type(ret)?;
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
        if let (Pattern::Tuple(patterns), Type::Tuple(types)) = (p, &ty) {
            for (pattern, ty) in patterns.iter().zip(types) {
                self.bind_pattern(pattern, ty.clone(), mutable)?;
            }
            return Ok(());
        }
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
            Stmt::Block(b) => self.block(b)?,
            Stmt::Var(x) => {
                self.validate_type(&x.ty)?;
                self.expected(&x.value, &x.ty)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable)?
            }
            Stmt::Assign { target, value } => {
                if matches!(target, Expr::Name(n) if n == "_") {
                    self.expr(value)?;
                    return Ok(());
                }
                let expected = self.lvalue(target)?;
                self.expression_types
                    .insert(target as *const Expr as usize, expected.clone());
                self.expected(value, &expected)?;
            }
            Stmt::Expr(x) | Stmt::Assert(x) => {
                let t = self.expr(x)?;
                if matches!(s, Stmt::Assert(_)) && !self.in_test {
                    return self.fail("assert is only available inside test blocks");
                }
                if matches!(s, Stmt::Expr(Expr::Call { .. })) && t != Type::void() {
                    return self
                        .fail("return value of function not used; assign it to `_` to discard it");
                }
                if matches!(s, Stmt::Assert(_)) && t != named("bool") {
                    return self.fail("assertion requires bool");
                }
            }
            Stmt::Return(x) => {
                let expected = self
                    .function_return
                    .clone()
                    .ok_or_else(|| Diagnostics::one("return outside function", 0..0))?;
                if let Some(value) = x {
                    self.expected(value, &expected)?;
                } else {
                    self.assignable(
                        if let Type::ErrorUnion(inner) = &expected {
                            inner
                        } else {
                            &expected
                        },
                        &Type::void(),
                    )?;
                }
            }
            Stmt::Throw(x) => {
                if self
                    .function_return
                    .as_ref()
                    .is_some_and(|t| !matches!(t, Type::ErrorUnion(_)))
                {
                    return self.fail("throw requires a throwing function return type (`!`)");
                }
                let actual = self.expr(x)?;
                if actual != named("error") {
                    self.assignable(&named("str"), &actual)?;
                }
            }
            Stmt::For {
                name,
                iterable,
                body,
                label,
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
                self.loops.push(label.clone());
                self.block(body)?;
                self.loops.pop();
                self.pop()
            }
            Stmt::While {
                condition,
                body,
                label,
            } => {
                let actual = self.expr(condition)?;
                self.assignable(&named("bool"), &actual)?;
                self.loops.push(label.clone());
                self.block(body)?;
                self.loops.pop();
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
            Stmt::Break(Some(value), None) if !self.value_targets.is_empty() => {
                let expected = self.value_targets.last().unwrap().clone();
                self.expected(value, &expected)?;
            }
            Stmt::Break(_, label) | Stmt::Continue(label) => {
                if self.loops.is_empty() {
                    return self.fail("loop control used outside a loop");
                }
                if let Some(label) = label {
                    if !self.loops.iter().any(|x| x.as_ref() == Some(label)) {
                        return self.fail(format!("unknown loop label `{label}`"));
                    }
                }
            }
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
            Expr::Index { object, .. } | Expr::Member { object, .. } => {
                self.lvalue(object)?;
                self.expr(e)
            }
            _ => self.fail("invalid assignment target"),
        }
    }
    fn expr(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        let ty = self.expr_inner(e)?;
        self.expression_types
            .insert(e as *const Expr as usize, ty.clone());
        Ok(ty)
    }
    fn expected(&mut self, e: &Expr, ty: &Type) -> Result<(), Diagnostics> {
        if let Type::Named(n, _) = ty {
            if let Some(TypeInfo::Alias(base)) = self.types.get(n).cloned() {
                if matches!(
                    e,
                    Expr::Int(_)
                        | Expr::Float(_)
                        | Expr::String(_)
                        | Expr::Char(_)
                        | Expr::Bool(_)
                        | Expr::Array(_)
                        | Expr::Tuple(_)
                        | Expr::Map(_)
                ) {
                    self.expected(e, &base)?;
                    return Ok(());
                }
            }
        }
        if matches!(e, Expr::If { .. }) {
            self.value_targets.push(ty.clone());
            let result = self.expr(e);
            self.value_targets.pop();
            result?;
            return Ok(());
        }
        if let Type::ErrorUnion(inner) = ty {
            if matches!(e, Expr::Name(_) | Expr::Call { .. }) {
                let actual = self.expr(e)?;
                if &actual == ty {
                    return Ok(());
                }
            }
            return self.expected(e, inner);
        }
        if let Type::Optional(inner) = ty {
            if matches!(e, Expr::None) {
                self.expression_types
                    .insert(e as *const Expr as usize, ty.clone());
                return Ok(());
            }
            if matches!(e, Expr::Name(_) | Expr::Call { .. }) {
                let actual = self.expr(e)?;
                if &actual == ty {
                    return Ok(());
                }
            }
            self.expected(e, inner)?;
            return Ok(());
        }
        match (e, ty) {
            (Expr::Map(entries), Type::Map(key, value)) => {
                for (k, v) in entries {
                    self.expected(k, key)?;
                    self.expected(v, value)?;
                }
            }
            (Expr::Array(values), Type::Map(_, _)) if values.is_empty() => {}
            (Expr::StructInit { name, fields }, Type::Named(expected, _)) if name == expected => {
                let Some(TypeInfo::Struct(declaration)) = self.types.get(name).cloned() else {
                    return self.fail(format!("`{name}` is not a struct"));
                };
                let mut seen = HashSet::new();
                for (name, value) in fields {
                    if !seen.insert(name) {
                        return self.fail(format!("duplicate field `{name}`"));
                    }
                    let Some(field) = declaration.fields.iter().find(|f| f.name == *name) else {
                        return self.fail(format!("unknown field `{name}`"));
                    };
                    self.expected(value, &field.ty)?;
                }
                if seen.len() != declaration.fields.len() {
                    return self.fail("missing struct fields");
                }
            }
            (Expr::Int(text), Type::Named(n, _))
                if matches!(n.as_str(), "byte" | "int" | "uint") =>
            {
                let v = integer(text)?;
                let max = match n.as_str() {
                    "byte" => 255,
                    "int" => i64::MAX as u64,
                    _ => u64::MAX,
                };
                if v > max || (text.ends_with('u') && n != "uint") {
                    return self.fail(format!("integer literal does not fit `{n}`"));
                }
            }
            (Expr::Array(values), Type::Array(element, size)) => {
                if size.is_some_and(|n| n != values.len()) {
                    return self.fail("array literal length does not match fixed-size array type");
                }
                for value in values {
                    self.expected(value, element)?;
                }
            }
            (Expr::Tuple(values), Type::Tuple(types)) if values.len() == types.len() => {
                for (value, ty) in values.iter().zip(types) {
                    self.expected(value, ty)?;
                }
            }
            _ => {
                let got = self.expr(e)?;
                self.assignable(ty, &got)?;
            }
        }
        self.expression_types
            .insert(e as *const Expr as usize, ty.clone());
        Ok(())
    }
    fn expr_inner(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        match e {
            Expr::Lambda(f) => {
                self.validate_type(&f.return_type)?;
                let old_scopes = self.scopes.clone();
                for scope in &mut self.scopes {
                    for binding in scope.values_mut() {
                        binding.mutable = false;
                    }
                }
                self.capture_frames
                    .push((self.scopes.len(), HashMap::new()));
                self.push();
                for p in &f.params {
                    self.validate_type(&p.ty)?;
                    self.bind(&p.name, p.ty.clone(), false)?;
                }
                let ret = self.function_return.replace(f.return_type.clone());
                let loops = std::mem::take(&mut self.loops);
                let targets = std::mem::take(&mut self.value_targets);
                let indexing = std::mem::replace(&mut self.indexing, 0);
                let in_test = std::mem::replace(&mut self.in_test, false);
                self.block(&f.body)?;
                if f.return_type != Type::void()
                    && f.return_type != Type::ErrorUnion(Box::new(Type::void()))
                    && !returns(&f.body)
                {
                    return self.fail("anonymous function may finish without returning a value");
                }
                self.scopes = old_scopes;
                self.function_return = ret;
                self.loops = loops;
                self.value_targets = targets;
                self.indexing = indexing;
                self.in_test = in_test;
                let mut captures: Vec<_> =
                    self.capture_frames.pop().unwrap().1.into_iter().collect();
                captures.sort_by(|a, b| a.0.cmp(&b.0));
                self.captures.insert(e as *const Expr as usize, captures);
                Ok(Type::Function(
                    f.params.iter().map(|p| p.ty.clone()).collect(),
                    Box::new(f.return_type.clone()),
                ))
            }
            Expr::Cast { ty, value } => {
                self.validate_type(ty)?;
                let from = self.expr(value)?;
                if let Type::Named(n, _) = ty {
                    if matches!(self.types.get(n),Some(TypeInfo::Alias(base)) if *base == from) {
                        return Ok(ty.clone());
                    }
                }
                if let Type::Named(n, _) = &from {
                    if matches!(self.types.get(n),Some(TypeInfo::Alias(base)) if base == ty) {
                        return Ok(ty.clone());
                    }
                }
                if let (Type::Array(a, None), Type::Array(b, Some(_))) = (ty, &from) {
                    if a == b {
                        return Ok(ty.clone());
                    }
                }
                if !(numeric(ty) && numeric(&from)) && ty != &from && *ty != named("str") {
                    return self.fail("this cast is not implemented");
                }
                Ok(ty.clone())
            }
            Expr::Int(s) => Ok(if s.ends_with('u') {
                integer(s)?;
                named("uint")
            } else {
                if integer(s)? > i64::MAX as u64 {
                    return self.fail("integer literal exceeds int range");
                }
                named("int")
            }),
            Expr::Float(_) => Ok(named("float")),
            Expr::String(_) => Ok(named("str")),
            Expr::Char(_) => Ok(named("char")),
            Expr::Bool(_) => Ok(named("bool")),
            Expr::None => self.fail("cannot infer type of none"),
            Expr::Name(n) if n == "$" && self.indexing > 0 => Ok(named("uint")),
            Expr::Name(n) => {
                if let Some((i, ty)) = self
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, s)| s.get(n).map(|v| (i, v.ty.clone())))
                {
                    for (depth, captures) in &mut self.capture_frames {
                        if i < *depth {
                            captures.insert(n.clone(), ty.clone());
                        }
                    }
                }
                self.lookup(n)
                    .map(|b| b.ty.clone())
                    .or_else(|| match self.types.get(n) {
                        Some(TypeInfo::External(function)) => Some(Type::Function(
                            function.params.iter().map(|p| p.ty.clone()).collect(),
                            Box::new(function.return_type.clone()),
                        )),
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
                Ok(Type::Array(Box::new(t), Some(xs.len())))
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
                if *op == UnaryOp::Neg
                    && matches!(&**value, Expr::Int(text) if !text.ends_with('u') && integer(text).ok() == Some(1u64 << 63))
                {
                    self.expression_types
                        .insert(&**value as *const Expr as usize, named("uint"));
                    return Ok(named("int"));
                }
                let t = self.expr(value)?;
                match op {
                    UnaryOp::Not => self.assignable(&named("bool"), &t)?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        if !numeric(&t) {
                            return self.fail("numeric unary operation required");
                        }
                        if *op == UnaryOp::BitNot && t == named("float") {
                            return self.fail("bitwise operations require integers");
                        }
                    }
                }
                Ok(t)
            }
            Expr::Binary { left, op, right } => {
                let l = if matches!(&**left, Expr::Int(_))
                    && !matches!(&**right, Expr::Int(_))
                    && *op != BinaryOp::In
                {
                    let r = self.expr(right)?;
                    if numeric(&r) && r != named("float") {
                        self.expected(left, &r)?;
                        r
                    } else {
                        self.expr(left)?
                    }
                } else {
                    self.expr(left)?
                };
                let r = if matches!(&**right, Expr::Int(_)) && numeric(&l) && l != named("float") {
                    self.expected(right, &l)?;
                    l.clone()
                } else {
                    self.expr(right)?
                };
                if *op == BinaryOp::In {
                    match &r {
                        Type::Array(element, _) => self.assignable(element, &l)?,
                        Type::Map(key, _) => self.assignable(key, &l)?,
                        Type::Named(n, _)
                            if n == "str"
                                && matches!(&l, Type::Named(n, _) if n == "str" || n == "char") => {
                        }
                        _ => return self.fail("in requires a compatible container and element"),
                    }
                    return Ok(named("bool"));
                }
                if let (Type::Array(a, n), Type::Array(b, m)) = (&l, &r) {
                    self.assignable(a, b)?;
                    return match op {
                        BinaryOp::Concat => Ok(Type::Array(
                            a.clone(),
                            n.zip(*m).and_then(|(n, m)| n.checked_add(m)),
                        )),
                        BinaryOp::Eq | BinaryOp::Ne => Ok(named("bool")),
                        _ => self.fail("unsupported operator for arrays"),
                    };
                }
                if *op == BinaryOp::Concat && l != named("str") && !matches!(l, Type::Map(_, _)) {
                    return self.fail("concatenation requires arrays, maps, or strings");
                }
                self.assignable(&l, &r)?;
                match op {
                    BinaryOp::And | BinaryOp::Or => self.assignable(&named("bool"), &l)?,
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow => {
                        if !numeric(&l) {
                            return self.fail("arithmetic requires numeric operands");
                        }
                    }
                    BinaryOp::BitAnd
                    | BinaryOp::BitOr
                    | BinaryOp::BitXor
                    | BinaryOp::Shl
                    | BinaryOp::Shr => {
                        if !numeric(&l) || l == named("float") {
                            return self.fail("bitwise operations require integers");
                        }
                    }
                    _ => {}
                }
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
                    self.expected(a, p)?;
                }
                Ok(*ret)
            }
            Expr::Index { object, index } => {
                let o = self.expr(object)?;
                if let Type::Map(key, value) = &o {
                    self.expected(index, key)?;
                    return Ok((**value).clone());
                }
                if let Type::Tuple(types) = &o {
                    if let Expr::Int(text) = &**index {
                        let n = integer(text)? as usize;
                        self.expr(index)?;
                        return types
                            .get(n)
                            .cloned()
                            .ok_or_else(|| Diagnostics::one("tuple index out of bounds", 0..0));
                    }
                    return self.fail("tuple index must be an integer literal");
                }
                self.indexing += 1;
                let index_type = self.expr(index)?;
                self.indexing -= 1;
                if index_type != named("uint") && index_type != named("int") {
                    return self.fail("array index must be int or uint");
                }
                match o {
                    Type::Array(t, _) => Ok(*t),
                    Type::Tuple(_) => self.fail("tuple index must be a compile-time value"),
                    _ => self.fail("value is not indexable"),
                }
            }
            Expr::Member { object, name } => {
                let o = self.expr(object)?;
                if let Type::Named(n, _) = &o {
                    if let Some(TypeInfo::Enum(declaration)) = self.types.get(n) {
                        let Some(variant) = declaration.variants.iter().find(|v| v.name == *name)
                        else {
                            return self.fail(format!("unknown enum variant `{name}`"));
                        };
                        return Ok(if variant.values.is_empty() {
                            o.clone()
                        } else {
                            Type::Function(variant.values.clone(), Box::new(o.clone()))
                        });
                    }
                    if let Some(TypeInfo::Struct(declaration)) = self.types.get(n) {
                        return declaration
                            .fields
                            .iter()
                            .find(|f| f.name == *name)
                            .map(|f| f.ty.clone())
                            .ok_or_else(|| {
                                Diagnostics::one(format!("unknown field `{name}`"), 0..0)
                            });
                    }
                }
                let has_len = matches!(o, Type::Array(_, _) | Type::Map(_, _))
                    || matches!(o, Type::Named(ref n, _) if n == "str");
                if name == "len" && has_len {
                    return Ok(named("uint"));
                }
                self.fail(format!("type does not have member `{name}`"))
            }
            Expr::If { subject, arms } => {
                let subject_type = if let Some(subject) = subject {
                    self.expr(subject)?
                } else {
                    named("bool")
                };
                let mut wildcard = false;
                let mut booleans = HashSet::new();
                let mut variants = HashSet::new();
                let target = self.value_targets.last().cloned();
                for (patterns, body) in arms {
                    self.push();
                    for pattern in patterns {
                        let total = self.check_pattern(pattern, &subject_type)?;
                        match pattern {
                            Pattern::Wildcard => wildcard = true,
                            Pattern::Literal(value) => {
                                if let Expr::Bool(b) = &**value {
                                    booleans.insert(*b);
                                }
                                if let Expr::Member { name, .. } = &**value {
                                    variants.insert(name.clone());
                                }
                            }
                            Pattern::Variant { name, .. } if total => {
                                variants.insert(name.rsplit('.').next().unwrap().to_string());
                            }
                            _ => wildcard |= total,
                        }
                    }
                    if let Some(ty) = &target {
                        self.value_block(body, ty)?;
                    } else {
                        self.block(body)?;
                    }
                    self.pop();
                }
                let enum_complete = if let Type::Named(n, _) = &subject_type {
                    if let Some(TypeInfo::Enum(e)) = self.types.get(n) {
                        e.variants.iter().all(|v| variants.contains(&v.name))
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !wildcard
                    && !enum_complete
                    && !(subject_type == named("bool") && booleans.len() == 2)
                {
                    return self.fail("conditional is not exhaustive; add a `_` fallback branch");
                }
                if arms.is_empty() {
                    return Ok(Type::void());
                }
                Ok(target.unwrap_or_else(Type::void))
            }
            Expr::Async(x) => Ok(Type::Future(Box::new(self.expr(x)?))),
            Expr::Await(x) => match self.expr(x)? {
                Type::Future(t) => Ok(*t),
                _ => self.fail("await expects a future"),
            },
            Expr::Try(x) => {
                if self
                    .function_return
                    .as_ref()
                    .is_some_and(|t| !matches!(t, Type::ErrorUnion(_)))
                {
                    return self.fail("try requires a throwing function return type (`!`)");
                }
                match self.expr(x)? {
                    Type::ErrorUnion(inner) => Ok(*inner),
                    _ => self.fail("try requires an error union"),
                }
            }
            Expr::Else { value, fallback } => {
                let Type::Optional(inner) = self.expr(value)? else {
                    return self.fail("else requires an optional value");
                };
                self.value_targets.push((*inner).clone());
                let result = self.value_block(fallback, &inner);
                self.value_targets.pop();
                result?;
                Ok(*inner)
            }
            Expr::Catch { value, name, body } => {
                let Type::ErrorUnion(inner) = self.expr(value)? else {
                    return self.fail("catch requires an error union");
                };
                self.push();
                self.bind(name, named("error"), false)?;
                self.value_targets.push((*inner).clone());
                let result = if *inner == Type::void() {
                    self.block(body)
                } else {
                    self.value_block(body, &inner)
                };
                self.value_targets.pop();
                self.pop();
                result?;
                Ok(*inner)
            }
            Expr::StructInit { name, .. } => {
                let ty = named(name);
                self.expected(e, &ty)?;
                Ok(ty)
            }
        }
    }
    fn assignable(&self, expected: &Type, got: &Type) -> Result<(), Diagnostics> {
        if let (Type::Array(a, None), Type::Array(b, _)) = (expected, got) {
            return self.assignable(a, b);
        }
        if expected == got || matches!(expected,Type::Optional(x)if **x==*got) {
            Ok(())
        } else {
            self.fail(format!("expected `{expected:?}`, found `{got:?}`"))
        }
    }
    fn value_block(&mut self, block: &Block, expected: &Type) -> Result<(), Diagnostics> {
        self.push();
        for (i, statement) in block.statements.iter().enumerate() {
            if i + 1 == block.statements.len() {
                if let Stmt::Expr(value) = statement {
                    self.expected(value, expected)?;
                    continue;
                }
            }
            self.stmt(statement)?;
        }
        let exits = block.statements.last().is_some_and(|s| {
            matches!(
                s,
                Stmt::Expr(_) | Stmt::Break(Some(_), _) | Stmt::Return(_) | Stmt::Throw(_)
            )
        });
        self.pop();
        if !exits {
            return self.fail("value-producing branch must provide a value or exit the function");
        }
        Ok(())
    }
    fn check_pattern(&mut self, p: &Pattern, ty: &Type) -> Result<bool, Diagnostics> {
        match p {
            Pattern::Wildcard => Ok(true),
            Pattern::Name(n) => {
                if let Some(binding) = self.lookup(n) {
                    self.assignable(ty, &binding.ty)?;
                    Ok(false)
                } else {
                    self.bind(n, ty.clone(), false)?;
                    Ok(true)
                }
            }
            Pattern::Literal(value) => {
                self.expected(value, ty)?;
                Ok(false)
            }
            Pattern::Array(patterns) => {
                let Type::Array(element, size) = ty else {
                    return self.fail("array pattern requires an array subject");
                };
                let mut total = size == &Some(patterns.len());
                for p in patterns {
                    total &= self.check_pattern(p, element)?;
                }
                Ok(total)
            }
            Pattern::Tuple(patterns) => {
                let Type::Tuple(types) = ty else {
                    return self.fail("tuple pattern requires a tuple subject");
                };
                if patterns.len() != types.len() {
                    return self.fail("tuple pattern has wrong arity");
                }
                let mut total = true;
                for (p, t) in patterns.iter().zip(types) {
                    total &= self.check_pattern(p, t)?;
                }
                Ok(total)
            }
            Pattern::Struct { name, fields } => {
                self.assignable(ty, &named(name))?;
                let Some(TypeInfo::Struct(declaration)) = self.types.get(name).cloned() else {
                    return self.fail("unknown struct pattern");
                };
                let mut total = true;
                for (name, p) in fields {
                    let Some(f) = declaration.fields.iter().find(|f| f.name == *name) else {
                        return self.fail("unknown field in struct pattern");
                    };
                    total &= self.check_pattern(p, &f.ty)?;
                }
                Ok(total)
            }
            Pattern::Variant { name, values } => {
                let Some((owner, variant)) = name.rsplit_once('.') else {
                    return self.fail("invalid enum pattern");
                };
                self.assignable(ty, &named(owner))?;
                let Some(TypeInfo::Enum(declaration)) = self.types.get(owner).cloned() else {
                    return self.fail("unknown enum pattern");
                };
                let Some(v) = declaration.variants.iter().find(|v| v.name == variant) else {
                    return self.fail("unknown enum variant");
                };
                if values.len() != v.values.len() {
                    return self.fail("incorrect enum payload arity");
                }
                let mut total = true;
                for (p, t) in values.iter().zip(&v.values) {
                    total &= self.check_pattern(p, t)?;
                }
                Ok(total)
            }
        }
    }
}
fn named(x: &str) -> Type {
    Type::Named(x.into(), vec![])
}
fn numeric(t: &Type) -> bool {
    matches!(t,Type::Named(n,_)if matches!(n.as_str(),"byte"|"int"|"uint"|"float"))
}

pub fn integer(text: &str) -> Result<u64, Diagnostics> {
    let text = text.strip_suffix('u').unwrap_or(text);
    let (digits, base) = if let Some(x) = text.strip_prefix("0x") {
        (x, 16)
    } else if let Some(x) = text.strip_prefix("0b") {
        (x, 2)
    } else if let Some(x) = text.strip_prefix("0o") {
        (x, 8)
    } else {
        (text, 10)
    };
    u64::from_str_radix(digits, base)
        .map_err(|_| Diagnostics::one("invalid or overflowing integer literal", 0..0))
}

fn returns(block: &Block) -> bool {
    block.statements.iter().any(|statement| match statement {
        Stmt::Return(_) | Stmt::Throw(_) => true,
        Stmt::Block(block) => returns(block),
        Stmt::Expr(Expr::If { arms, .. }) => {
            !arms.is_empty() && arms.iter().all(|(_, block)| returns(block))
        }
        _ => false,
    })
}
