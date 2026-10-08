use crate::{
    ast::{
        BUILTIN_TYPES, BinaryOp, Block, EnumDecl, Expr, Function, FunctionDecl, Item, Module,
        Pattern, SourceLocation, Stmt, StructDecl, Type, UnaryOp, VarDecl,
    },
    diagnostic::Diagnostics,
    lexer::integer,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug)]
pub struct CheckedModule {
    pub module: Module,
    pub types: HashMap<String, TypeInfo>,
    pub expression_types: HashMap<usize, Type>,
    pub captures: HashMap<usize, Vec<Capture>>,
    pub shared_accesses: HashSet<usize>,
    pub async_sites: HashMap<usize, SourceLocation>,
    pub constant_sources: HashMap<usize, Option<usize>>,
    pub constant_patterns: HashMap<usize, (Pattern, Type)>,
}
#[derive(Clone, Debug)]
pub struct Capture {
    pub name: String,
    pub mutable: bool,
    pub declaration: Option<usize>,
    pub ty: Type,
    pub mutex: bool,
    pub initializer: Option<usize>,
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
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    ty: Type,
    declaration: Option<usize>,
    locked: bool,
    mutable: bool,
    mutex: bool,
    initializer: Option<usize>,
}
struct Checker {
    types: HashMap<String, TypeInfo>,
    locations: HashMap<String, SourceLocation>,
    scopes: Vec<HashMap<String, Binding>>,
    function_return: Option<Type>,
    in_test: bool,
    generics: HashSet<String>,
    expression_types: HashMap<usize, Type>,
    // Label, accepts continue, accepts an unlabelled break.
    loops: Vec<(Option<String>, bool, bool)>,
    indexing: usize,
    value_targets: Vec<Type>,
    capture_frames: Vec<(usize, HashMap<String, Binding>)>,
    captures: HashMap<usize, Vec<Capture>>,
    shared_accesses: HashSet<usize>,
    async_sites: HashMap<usize, SourceLocation>,
    constant_sources: HashMap<usize, Option<usize>>,
    constant_patterns: HashMap<usize, (Pattern, Type)>,
}

/// Check declarations, types, control flow, and capture/access rules in a module.
///
/// # Errors
/// Returns diagnostics for invalid declarations or types, incompatible expressions,
/// invalid control flow, non-exhaustive conditionals, or invalid future/mutex access.
pub fn check(module: Module, _path: &Path) -> Result<CheckedModule, Diagnostics> {
    let mut c = Checker::new();
    let mut external_symbols = HashMap::new();
    for item in &module.items {
        c.declare(item).map_err(|error| at_item(error, item))?;
        if let Item::Extern { functions, .. } = item {
            for f in functions {
                let signature = (
                    f.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>(),
                    f.return_type.clone(),
                );
                if external_symbols
                    .insert(&f.symbol, signature.clone())
                    .is_some_and(|previous| previous != signature)
                {
                    return Checker::fail(format!(
                        "conflicting declarations for external C symbol `{}`",
                        f.symbol
                    ))
                    .map_err(|error| at_item(error, item));
                }
            }
        }
    }
    c.validate_layouts()?;
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
        shared_accesses: c.shared_accesses,
        async_sites: c.async_sites,
        constant_sources: c.constant_sources,
        constant_patterns: c.constant_patterns,
    })
}
fn at_item(error: Diagnostics, item: &Item) -> Diagnostics {
    if let Some((path, span)) = item.source() {
        error.at_source(path, span.clone())
    } else {
        error
    }
}
impl Checker {
    fn contains_future(&self, ty: &Type) -> bool {
        self.contains_type(ty, &|ty| matches!(ty, Type::Future(_)))
    }
    fn contains_type(&self, ty: &Type, predicate: &impl Fn(&Type) -> bool) -> bool {
        fn visit(
            checker: &Checker,
            ty: &Type,
            seen: &mut HashSet<String>,
            predicate: &impl Fn(&Type) -> bool,
        ) -> bool {
            if predicate(ty) {
                return true;
            }
            match ty {
                Type::Future(_) | Type::Function(_, _) => false,
                Type::Named(name, args) => {
                    if args.iter().any(|ty| visit(checker, ty, seen, predicate)) {
                        return true;
                    }
                    if !seen.insert(name.clone()) {
                        return false;
                    }
                    match checker.types.get(name) {
                        Some(TypeInfo::Alias(ty)) => visit(checker, ty, seen, predicate),
                        Some(TypeInfo::Struct(s)) => s
                            .fields
                            .iter()
                            .any(|f| visit(checker, &f.ty, seen, predicate)),
                        Some(TypeInfo::Enum(e)) => e.variants.iter().any(|v| {
                            v.values
                                .iter()
                                .any(|ty| visit(checker, ty, seen, predicate))
                        }),
                        _ => false,
                    }
                }
                Type::Array(ty, _) | Type::Optional(ty) | Type::ErrorUnion(ty) => {
                    visit(checker, ty, seen, predicate)
                }
                Type::Map(key, value) => {
                    visit(checker, key, seen, predicate) || visit(checker, value, seen, predicate)
                }
                Type::Tuple(types) => types.iter().any(|ty| visit(checker, ty, seen, predicate)),
            }
        }
        visit(self, ty, &mut HashSet::new(), predicate)
    }
    fn value_operation(&self, ty: &Type, operation: &str) -> Result<(), Diagnostics> {
        if self.contains_type(ty, &|ty| {
            matches!(ty, Type::Function(_, _) | Type::Future(_))
        }) {
            return Checker::fail(format!(
                "{operation} is not defined for functions or unawaited futures, including inside composite values"
            ));
        }
        if *ty == Type::void() {
            return Checker::fail(format!("{operation} requires a value, not void"));
        }
        if operation == "string conversion" {
            if self.contains_type(ty, &|ty| matches!(ty, Type::Named(name, _) if matches!(self.types.get(name), Some(TypeInfo::Alias(_))))) {
                return Checker::fail("string conversion requires custom types to be explicitly converted to their underlying base types, including inside composite values");
            }
            if self.contains_type(ty, &|ty| *ty == Type::void()) {
                return Checker::fail(
                    "string conversion is not defined for void, including inside composite values",
                );
            }
        }
        Ok(())
    }
    fn validate_layouts(&self) -> Result<(), Diagnostics> {
        fn visit(
            checker: &Checker,
            ty: &Type,
            active: &mut HashSet<String>,
            completed: &mut HashSet<String>,
            aliases_only: bool,
        ) -> Result<(), Diagnostics> {
            match ty {
                Type::Named(name, _) => {
                    if completed.contains(name) {
                        return Ok(());
                    }
                    let children = match checker.types.get(name) {
                        Some(TypeInfo::Alias(base)) => vec![base],
                        Some(TypeInfo::Struct(declaration)) if !aliases_only => {
                            declaration.fields.iter().map(|field| &field.ty).collect()
                        }
                        _ => return Ok(()),
                    };
                    if !active.insert(name.clone()) {
                        return Checker::fail(if aliases_only {
                            format!("cyclic nominal type definition involving `{name}`")
                        } else {
                            format!(
                                "recursive type `{name}` has infinite size; use an array or enum payload to break the cycle"
                            )
                        });
                    }
                    for child in children {
                        visit(checker, child, active, completed, aliases_only)?;
                    }
                    active.remove(name);
                    completed.insert(name.clone());
                }
                Type::Tuple(types) => {
                    for ty in types {
                        visit(checker, ty, active, completed, aliases_only)?;
                    }
                }
                Type::Optional(ty) | Type::ErrorUnion(ty) => {
                    visit(checker, ty, active, completed, aliases_only)?;
                }
                Type::Array(ty, _) | Type::Future(ty) if aliases_only => {
                    visit(checker, ty, active, completed, true)?;
                }
                Type::Map(key, value) if aliases_only => {
                    visit(checker, key, active, completed, true)?;
                    visit(checker, value, active, completed, true)?;
                }
                Type::Function(params, ret) if aliases_only => {
                    for ty in params {
                        visit(checker, ty, active, completed, true)?;
                    }
                    visit(checker, ret, active, completed, true)?;
                }
                _ => {}
            }
            Ok(())
        }
        let mut names = self.types.keys().collect::<Vec<_>>();
        names.sort();
        // Alias cycles and finite layouts follow different edges through containers.
        let mut completed_by_mode = [HashSet::new(), HashSet::new()];
        for name in names {
            for (aliases_only, completed) in [true, false].into_iter().zip(&mut completed_by_mode) {
                visit(
                    self,
                    &named(name),
                    &mut HashSet::new(),
                    completed,
                    aliases_only,
                )
                .map_err(|error| {
                    if let Some(location) = self.locations.get(name) {
                        error.at_source(&location.path, location.span.clone())
                    } else {
                        error
                    }
                })?;
            }
        }
        Ok(())
    }
    fn future_variable(&self, v: &VarDecl) -> Result<(), Diagnostics> {
        if v.mutex && !matches!(v.pattern, Pattern::Name(_)) {
            return Checker::fail(
                "mutex declarations require a single binding; lock the tuple before destructuring it",
            );
        }
        if matches!(v.ty, Type::Future(_)) {
            if v.mutable || v.mutex {
                return Checker::fail("futures cannot be mutable or mutex protected");
            }
            if !matches!(v.value.unlocated(), Expr::Async(_)) {
                return Checker::fail("a future must be initialized by an async function call");
            }
        } else if self.contains_future(&v.ty) {
            return Checker::fail(
                "futures must be declared directly from async calls, not stored in composite or nominal values",
            );
        }
        Ok(())
    }
    fn new() -> Self {
        let mut types = HashMap::new();
        for n in BUILTIN_TYPES {
            types.insert(n.into(), TypeInfo::Builtin);
        }
        Self {
            types,
            locations: HashMap::new(),
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
            shared_accesses: HashSet::new(),
            async_sites: HashMap::new(),
            constant_sources: HashMap::new(),
            constant_patterns: HashMap::new(),
        }
    }
    fn fail<T>(s: impl Into<String>) -> Result<T, Diagnostics> {
        Err(Diagnostics::one(s, 0..0))
    }
    fn declare(&mut self, item: &Item) -> Result<(), Diagnostics> {
        let name = match item {
            Item::Struct(s) => Some(&s.name),
            Item::Enum(e) => Some(&e.name),
            Item::TypeAlias { name, .. } => Some(name),
            _ => None,
        };
        if let Some(name) = name
            && let Some((path, span)) = item.source()
        {
            self.locations.insert(
                name.clone(),
                SourceLocation {
                    path: path.into(),
                    span: span.clone(),
                },
            );
        }
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
            Item::Global(_) | Item::Statement(_) | Item::Import { .. } | Item::Test { .. } => {}
        }
        Ok(())
    }
    fn add_type(&mut self, n: &str, i: TypeInfo) -> Result<(), Diagnostics> {
        if self.types.insert(n.into(), i).is_some() {
            Checker::fail(format!("duplicate declaration `{n}`"))
        } else {
            Ok(())
        }
    }
    fn item(&mut self, item: &Item) -> Result<(), Diagnostics> {
        self.item_inner(item).map_err(|error| at_item(error, item))
    }
    fn item_inner(&mut self, item: &Item) -> Result<(), Diagnostics> {
        match item {
            Item::Extern {
                path, functions, ..
            } => self.check_extern(path, functions)?,
            Item::Struct(x) => self.with_generics(&x.generics, |this| {
                for f in &x.fields {
                    this.validate_type(&f.ty)?;
                }
                Ok(())
            })?,
            Item::Enum(x) => self.with_generics(&x.generics, |this| {
                for v in &x.variants {
                    for t in &v.values {
                        this.validate_type(t)?;
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
                        return Checker::fail("cyclic nominal type definition");
                    }
                    if let Some(TypeInfo::Alias(base)) = self.types.get(&n) {
                        current = base.clone();
                    } else {
                        break;
                    }
                }
            }
            Item::Function(x) => self.with_generics(&x.generics, |this| {
                if this.contains_future(&x.return_type) {
                    return Checker::fail("futures cannot be returned from functions");
                }
                this.validate_type(&x.return_type)?;
                this.push();
                for p in &x.params {
                    this.validate_type(&p.ty)?;
                    this.bind(&p.name, p.ty.clone(), false);
                }
                this.function_return = Some(x.return_type.clone());
                this.block(&x.body)?;
                if x.return_type != Type::void()
                    && x.return_type != Type::ErrorUnion(Box::new(Type::void()))
                    && !crate::flow::returns(&x.body, &this.expression_types)
                {
                    return Checker::fail(format!(
                        "function `{}` may finish without returning a value",
                        x.name
                    ));
                }
                this.function_return = None;
                this.pop();
                Ok(())
            })?,
            Item::Global(x) => {
                self.future_variable(x)?;
                self.validate_type(&x.ty)?;
                self.declaration_value(x)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable);
                self.mark_mutex(x);
            }
            Item::Statement(statement) => self.stmt(statement)?,
            Item::Test { body, .. } => {
                self.push();
                self.in_test = true;
                self.block(body)?;
                self.in_test = false;
                self.pop();
            }
            Item::Import { .. } => {}
        }
        Ok(())
    }
    fn check_extern(&self, path: &str, functions: &[FunctionDecl]) -> Result<(), Diagnostics> {
        if std::path::Path::new(path).extension() != Some(std::ffi::OsStr::new("c")) {
            return Checker::fail("external implementations must be C source files (.c)");
        }
        for function in functions {
            Self::check_external_symbol(&function.symbol)?;
            self.validate_type(&function.return_type)?;
            if self.contains_future(&function.return_type) {
                return Checker::fail("futures cannot be returned from external functions");
            }
            for param in &function.params {
                self.validate_type(&param.ty)?;
            }
        }
        Ok(())
    }
    fn check_external_symbol(symbol: &str) -> Result<(), Diagnostics> {
        if symbol.is_empty()
            || !symbol
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        {
            return Checker::fail("external symbol must be a C identifier");
        }
        if matches!(
            symbol,
            "auto"
                | "break"
                | "case"
                | "char"
                | "const"
                | "continue"
                | "default"
                | "do"
                | "double"
                | "else"
                | "enum"
                | "extern"
                | "float"
                | "for"
                | "goto"
                | "if"
                | "inline"
                | "int"
                | "long"
                | "register"
                | "restrict"
                | "return"
                | "short"
                | "signed"
                | "sizeof"
                | "static"
                | "struct"
                | "switch"
                | "typedef"
                | "union"
                | "unsigned"
                | "void"
                | "volatile"
                | "while"
                | "_Alignas"
                | "_Alignof"
                | "_Atomic"
                | "_Bool"
                | "_Complex"
                | "_Generic"
                | "_Imaginary"
                | "_Noreturn"
                | "_Static_assert"
                | "_Thread_local"
        ) {
            return Checker::fail("external symbol cannot be a C keyword");
        }
        Ok(())
    }
    fn validate_type(&self, t: &Type) -> Result<(), Diagnostics> {
        match t {
            Type::Named(n, args) => {
                if matches!(
                    self.types.get(n),
                    Some(TypeInfo::Function(_) | TypeInfo::External(_))
                ) {
                    return Checker::fail(format!("`{n}` is a function, not a type"));
                }
                if !self.types.contains_key(n) && !self.generic_in_scope(n) {
                    return Checker::fail(format!("unknown type `{n}`"));
                }
                for a in args {
                    self.validate_type(a)?;
                }
            }
            Type::Array(x, _) | Type::Optional(x) | Type::ErrorUnion(x) | Type::Future(x) => {
                self.validate_type(x)?;
            }
            Type::Map(k, v) => {
                self.validate_type(k)?;
                self.value_operation(k, "map-key equality")?;
                self.validate_type(v)?;
            }
            Type::Tuple(xs) | Type::Function(xs, _) => {
                for x in xs {
                    self.validate_type(x)?;
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
        self.scopes.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn bind(&mut self, n: &str, ty: Type, mutable: bool) {
        self.scopes.last_mut().unwrap().insert(
            n.into(),
            Binding {
                ty,
                declaration: None,
                locked: false,
                mutable,
                mutex: false,
                initializer: None,
            },
        );
    }
    fn bind_pattern(&mut self, p: &Pattern, ty: Type, mutable: bool) {
        if let (Pattern::Tuple(patterns), Type::Tuple(types)) = (p, &ty) {
            for (pattern, ty) in patterns.iter().zip(types) {
                self.bind_pattern(pattern, ty.clone(), mutable);
            }
            return;
        }
        if let Pattern::Name(n) = p
            && n != "_"
        {
            self.bind(n, ty, mutable);
        }
    }
    fn lookup(&self, n: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }
    fn mark_mutex(&mut self, v: &VarDecl) {
        let key = v.value.id();
        for name in v.binding_names() {
            if let Some(binding) = self.scopes.last_mut().unwrap().get_mut(name) {
                binding.mutex = v.mutex;
                binding.declaration = Some(key);
                if !v.mutable && !v.mutex {
                    binding.initializer = Some(key);
                }
            }
        }
        if !v.mutable && !v.mutex && matches!(v.pattern, Pattern::Tuple(_)) {
            self.constant_patterns
                .insert(key, (v.pattern.clone(), v.ty.clone()));
        }
    }
    fn block(&mut self, b: &Block) -> Result<(), Diagnostics> {
        self.push();
        for s in &b.statements {
            self.stmt(s)?;
        }
        self.pop();
        Ok(())
    }
    fn stmt(&mut self, s: &Stmt) -> Result<(), Diagnostics> {
        self.stmt_inner(s).map_err(|error| match s.source() {
            Some((path, span)) => error.at_source(path, span.clone()),
            None => error,
        })
    }
    fn stmt_inner(&mut self, s: &Stmt) -> Result<(), Diagnostics> {
        match s {
            Stmt::Located(statement, _) => self.stmt(statement)?,
            Stmt::LabeledIf { label, value } => {
                self.loops.push((Some(label.clone()), false, false));
                self.expr(value)?;
                self.loops.pop();
            }
            Stmt::Block(b) => self.block(b)?,
            Stmt::Var(x) => {
                self.future_variable(x)?;
                self.validate_type(&x.ty)?;
                self.declaration_value(x)?;
                self.bind_pattern(&x.pattern, x.ty.clone(), x.mutable);
                self.mark_mutex(x);
            }
            Stmt::Assign { target, value } => {
                if matches!(target.unlocated(), Expr::Name(n) if n == "_") {
                    self.expr(value)?;
                    return Ok(());
                }
                let expected = self.lvalue(target)?;
                self.expression_types.insert(target.id(), expected.clone());
                self.expected(value, &expected)?;
            }
            Stmt::Expr(x) | Stmt::Assert(x) => {
                let t = self.expr(x)?;
                if matches!(s, Stmt::Assert(_)) && !self.in_test {
                    return Checker::fail("assert is only available inside test blocks");
                }
                if matches!(s, Stmt::Expr(value) if matches!(value.unlocated(), Expr::Call { .. }))
                    && t != Type::void()
                {
                    return Checker::fail(
                        "return value of function not used; assign it to `_` to discard it",
                    );
                }
                if matches!(s, Stmt::Assert(_)) && t != named("bool") {
                    return Checker::fail("assertion requires bool");
                }
            }
            Stmt::Return(value) => self.check_return(value.as_ref())?,
            Stmt::Throw(x) => {
                if self
                    .function_return
                    .as_ref()
                    .is_some_and(|t| !matches!(t, Type::ErrorUnion(_)))
                {
                    return Checker::fail("throw requires a throwing function return type (`!`)");
                }
                let actual = self.expr(x)?;
                if actual != named("error") {
                    Checker::assignable(&named("str"), &actual)?;
                }
            }
            Stmt::For {
                name,
                iterable,
                body,
                label,
            } => {
                self.check_for(name, iterable, body, label.as_deref())?;
            }
            Stmt::While {
                condition,
                body,
                label,
            } => {
                let actual = self.expr(condition)?;
                Checker::assignable(&named("bool"), &actual)?;
                self.loops.push((label.clone(), true, true));
                self.block(body)?;
                self.loops.pop();
            }
            Stmt::Lock { name, body, label } => self.check_lock(name, body, label.as_deref())?,
            Stmt::Break(Some(value), _) => {
                let Some(expected) = self.value_targets.last().cloned() else {
                    return Checker::fail(
                        "break with a value requires a value-producing conditional, else, or catch block",
                    );
                };
                self.expected(value, &expected)?;
            }
            Stmt::Break(_, label) | Stmt::Continue(label) => {
                self.check_jump(label.as_deref(), matches!(s, Stmt::Continue(_)))?;
            }
        }
        Ok(())
    }
    fn check_jump(&self, label: Option<&str>, continuing: bool) -> Result<(), Diagnostics> {
        if !self
            .loops
            .iter()
            .rev()
            .any(|(name, can_continue, can_break)| {
                label.is_none_or(|label| name.as_deref() == Some(label))
                    && (!continuing || *can_continue)
                    && (label.is_some() || continuing || *can_break)
            })
        {
            return Checker::fail("no valid target for break or continue");
        }
        Ok(())
    }
    fn check_return(&mut self, value: Option<&Expr>) -> Result<(), Diagnostics> {
        let expected = self
            .function_return
            .clone()
            .ok_or_else(|| Diagnostics::one("return outside function", 0..0))?;
        if let Some(value) = value {
            self.expected(value, &expected)?;
            if self
                .expression_types
                .get(&value.id())
                .is_some_and(|ty| self.is_error_value(ty))
            {
                return Checker::fail("errors cannot be returned; they must be thrown");
            }
        } else {
            Checker::assignable(
                if let Type::ErrorUnion(inner) = &expected {
                    inner
                } else {
                    &expected
                },
                &Type::void(),
            )?;
        }
        Ok(())
    }
    fn is_error_value<'a>(&'a self, mut ty: &'a Type) -> bool {
        while let Type::Named(name, _) = ty {
            if name == "error" {
                return true;
            }
            let Some(TypeInfo::Alias(base)) = self.types.get(name) else {
                break;
            };
            ty = base;
        }
        false
    }
    fn check_for(
        &mut self,
        name: &str,
        iterable: &Expr,
        body: &Block,
        label: Option<&str>,
    ) -> Result<(), Diagnostics> {
        let iter = self.expr(iterable)?;
        let key = match iter {
            Type::Array(_, _) => named("uint"),
            Type::Map(k, _) => *k,
            Type::Named(n, _) if n == "str" => named("uint"),
            _ => return Checker::fail("for loop expects an array, map, or str"),
        };
        self.push();
        self.bind(name, key, false);
        self.loops.push((label.map(str::to_owned), true, true));
        self.block(body)?;
        self.loops.pop();
        self.pop();
        Ok(())
    }
    fn check_lock(
        &mut self,
        name: &str,
        body: &Block,
        label: Option<&str>,
    ) -> Result<(), Diagnostics> {
        if let Some((i, binding)) = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, s)| s.get(name).map(|v| (i, v.clone())))
        {
            for (depth, captures) in &mut self.capture_frames {
                if i < *depth {
                    captures.insert(name.to_owned(), binding.clone());
                }
            }
        }
        let b = self
            .lookup(name)
            .ok_or_else(|| Diagnostics::one(format!("unknown variable `{name}`"), 0..0))?
            .clone();
        if !b.mutex {
            return Checker::fail("lock requires a mutex variable");
        }
        self.push();
        self.bind(name, b.ty, true);
        self.scopes
            .last_mut()
            .unwrap()
            .get_mut(name)
            .unwrap()
            .locked = true;
        self.loops.push((label.map(str::to_owned), false, true));
        self.block(body)?;
        self.loops.pop();
        self.pop();
        Ok(())
    }
    fn lvalue(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        if let Expr::Located(value, location) = e {
            return self
                .lvalue(value)
                .map_err(|error| error.at_source(&location.path, location.span.clone()));
        }
        match e {
            Expr::Name(n) => {
                let b = self
                    .lookup(n)
                    .ok_or_else(|| Diagnostics::one(format!("unknown variable `{n}`"), 0..0))?;
                if !b.mutable {
                    return Checker::fail(format!("cannot mutate immutable `{n}`"));
                }
                self.expr(e)
            }
            Expr::Member { object, name } if name == "len" => {
                let ty = self.expr(object)?;
                if matches!(ty, Type::Array(_, _) | Type::Map(_, _)) || ty == named("str") {
                    return Checker::fail("container length is read-only");
                }
                self.lvalue(object)?;
                self.expr(e)
            }
            Expr::Index { object, .. } | Expr::Member { object, .. } => {
                self.lvalue(object)?;
                self.expr(e)
            }
            _ => Checker::fail("invalid assignment target"),
        }
    }
    fn expr(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        if let Expr::Located(value, location) = e {
            if matches!(value.unlocated(), Expr::Async(_)) {
                self.async_sites.insert(e.id(), location.clone());
            }
            return self
                .expr(value)
                .map_err(|error| error.at_source(&location.path, location.span.clone()));
        }
        let ty = self.expr_inner(e)?;
        self.expression_types.insert(e.id(), ty.clone());
        Ok(ty)
    }
    fn declaration_value(&mut self, v: &VarDecl) -> Result<(), Diagnostics> {
        if let (Pattern::Tuple(_), Type::Tuple(types)) = (&v.pattern, &v.ty)
            && types.iter().any(|ty| matches!(ty, Type::Tuple(_)))
        {
            let count = match v.value.unlocated() {
                Expr::Tuple(values) => Some(values.len()),
                _ => match self.expr(&v.value)? {
                    Type::Tuple(fields) => Some(fields.len()),
                    _ => None,
                },
            };
            if count.is_some_and(|count| count != types.len()) {
                fn flatten(ty: &Type, fields: &mut Vec<Type>) {
                    match ty {
                        Type::Tuple(types) => types.iter().for_each(|ty| flatten(ty, fields)),
                        ty => fields.push(ty.clone()),
                    }
                }
                let mut fields = Vec::new();
                flatten(&v.ty, &mut fields);
                if count != Some(fields.len()) {
                    return Checker::fail(format!(
                        "tuple destructuring expects {} grouped or {} flat elements, found {}",
                        types.len(),
                        fields.len(),
                        count.unwrap()
                    ));
                }
                return self.expected(&v.value, &Type::Tuple(fields));
            }
        }
        self.expected(&v.value, &v.ty)
    }
    fn expected(&mut self, e: &Expr, ty: &Type) -> Result<(), Diagnostics> {
        if let Expr::Located(value, location) = e {
            if matches!(value.unlocated(), Expr::Async(_)) {
                self.async_sites.insert(e.id(), location.clone());
            }
            return self
                .expected(value, ty)
                .map_err(|error| error.at_source(&location.path, location.span.clone()));
        }
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.types.get(n).cloned()
            && matches!(
                e,
                Expr::Int(_)
                    | Expr::Float(_)
                    | Expr::String(_)
                    | Expr::Char(_)
                    | Expr::Bool(_)
                    | Expr::Array(_)
                    | Expr::Tuple(_)
                    | Expr::Map(_)
            )
        {
            self.expected(e, &base)?;
            return Ok(());
        }
        if matches!(e, Expr::If { .. }) {
            self.expression_types.insert(e.id(), ty.clone());
            self.value_targets.push(ty.clone());
            let result = self.expr(e);
            self.value_targets.pop();
            result?;
            return Ok(());
        }
        if let Type::ErrorUnion(inner) = ty {
            if matches!(
                e,
                Expr::Name(_)
                    | Expr::Call { .. }
                    | Expr::Member { .. }
                    | Expr::Index { .. }
                    | Expr::Else { .. }
                    | Expr::Catch { .. }
                    | Expr::Try(_)
                    | Expr::Await(_)
            ) {
                let actual = self.expr(e)?;
                if &actual == ty {
                    return Ok(());
                }
            }
            return self.expected(e, inner);
        }
        if let Type::Optional(inner) = ty {
            if matches!(e, Expr::None) {
                self.expression_types.insert(e.id(), ty.clone());
                return Ok(());
            }
            if matches!(
                e,
                Expr::Name(_)
                    | Expr::Call { .. }
                    | Expr::Member { .. }
                    | Expr::Index { .. }
                    | Expr::Else { .. }
                    | Expr::Catch { .. }
                    | Expr::Try(_)
                    | Expr::Await(_)
            ) {
                let actual = self.expr(e)?;
                if &actual == ty {
                    return Ok(());
                }
            }
            self.expected(e, inner)?;
            return Ok(());
        }
        self.expected_value(e, ty)?;
        self.expression_types.insert(e.id(), ty.clone());
        Ok(())
    }
    fn expected_value(&mut self, e: &Expr, ty: &Type) -> Result<(), Diagnostics> {
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
                    return Checker::fail(format!("`{name}` is not a struct"));
                };
                let mut seen = HashSet::new();
                for (name, value) in fields {
                    if !seen.insert(name) {
                        return Checker::fail(format!("duplicate field `{name}`"));
                    }
                    let Some(field) = declaration.fields.iter().find(|f| f.name == *name) else {
                        return Checker::fail(format!("unknown field `{name}`"));
                    };
                    self.expected(value, &field.ty)?;
                }
                if seen.len() != declaration.fields.len() {
                    return Checker::fail("missing struct fields");
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
                    return Checker::fail(format!("integer literal does not fit `{n}`"));
                }
            }
            (Expr::Array(values), Type::Array(element, size)) => {
                if size.is_some_and(|n| n != values.len()) {
                    return Checker::fail(
                        "array literal length does not match fixed-size array type",
                    );
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
                Checker::assignable(ty, &got)?;
            }
        }
        Ok(())
    }
    fn expr_inner(&mut self, e: &Expr) -> Result<Type, Diagnostics> {
        match e {
            Expr::Located(_, _) => unreachable!("expression locations are handled by expr"),
            Expr::Bytes(_) => Ok(Type::Array(Box::new(named("byte")), None)),
            Expr::Embed { path, .. } => {
                self.expected(path, &named("str"))?;
                Ok(Type::Array(Box::new(named("byte")), None))
            }
            Expr::Lambda(f) => self.check_lambda(e, f),
            Expr::Cast {
                ty,
                value,
                implicit,
            } => self.check_cast(ty, value, *implicit),
            Expr::Int(s) => Ok(if s.ends_with('u') {
                integer(s)?;
                named("uint")
            } else {
                if integer(s)? > i64::MAX as u64 {
                    return Checker::fail("integer literal exceeds int range");
                }
                named("int")
            }),
            Expr::Float(_) => Ok(named("float")),
            Expr::String(_) => Ok(named("str")),
            Expr::Char(_) => Ok(named("char")),
            Expr::Bool(_) => Ok(named("bool")),
            Expr::None => Checker::fail("cannot infer type of none"),
            Expr::Name(n) if n == "$" && self.indexing > 0 => Ok(named("uint")),
            Expr::Name(n) => self.check_name(e, n),
            Expr::Discard => Ok(Type::void()),
            Expr::Array(xs) => self.check_array(xs),
            Expr::Map(xs) => self.check_map(xs),
            Expr::Tuple(xs) => Ok(Type::Tuple(
                xs.iter().map(|x| self.expr(x)).collect::<Result<_, _>>()?,
            )),
            Expr::Unary { op, value } => self.check_unary(*op, value),
            Expr::Binary { left, op, right } => self.check_binary(left, *op, right),
            Expr::Call { callee, args, .. } => self.check_call(callee, args),
            Expr::Index { object, index } => self.check_index(object, index),
            Expr::Member { object, name } => self.check_member(object, name),
            Expr::If { subject, arms } => self.check_if(e, subject.as_deref(), arms),
            Expr::Async(x) => {
                if !matches!(x.unlocated(), Expr::Call { .. }) {
                    return Checker::fail("async requires a function call");
                }
                Ok(Type::Future(Box::new(self.expr(x)?)))
            }
            Expr::Await(x) => match self.expr(x)? {
                Type::Future(t) => Ok(*t),
                _ => Checker::fail("await expects a future"),
            },
            Expr::Try(x) => self.check_try(x),
            Expr::Else { value, fallback } => self.check_else(value, fallback),
            Expr::Catch { value, name, body } => self.check_catch(value, name, body),
            Expr::StructInit { name, .. } => {
                let ty = named(name);
                self.expected(e, &ty)?;
                Ok(ty)
            }
        }
    }
    fn check_lambda(&mut self, e: &Expr, f: &Function) -> Result<Type, Diagnostics> {
        if self.contains_future(&f.return_type) {
            return Checker::fail("futures cannot be returned from functions");
        }
        self.validate_type(&f.return_type)?;
        let old_scopes = self.scopes.clone();
        // A function does not inherit permission granted by an enclosing
        // lock. Capture the mutex itself and require a new lock to write.
        for binding in self.scopes.iter_mut().flat_map(|scope| scope.values_mut()) {
            if binding.locked {
                binding.mutable = false;
                binding.mutex = true;
                binding.locked = false;
            }
        }
        self.capture_frames
            .push((self.scopes.len(), HashMap::new()));
        self.push();
        for p in &f.params {
            self.validate_type(&p.ty)?;
            self.bind(&p.name, p.ty.clone(), false);
        }
        let ret = self.function_return.replace(f.return_type.clone());
        let loops = std::mem::take(&mut self.loops);
        let targets = std::mem::take(&mut self.value_targets);
        let indexing = std::mem::replace(&mut self.indexing, 0);
        let in_test = std::mem::replace(&mut self.in_test, false);
        self.block(&f.body)?;
        if f.return_type != Type::void()
            && f.return_type != Type::ErrorUnion(Box::new(Type::void()))
            && !crate::flow::returns(&f.body, &self.expression_types)
        {
            return Checker::fail("anonymous function may finish without returning a value");
        }
        self.scopes = old_scopes;
        self.function_return = ret;
        self.loops = loops;
        self.value_targets = targets;
        self.indexing = indexing;
        self.in_test = in_test;
        let mut captures: Vec<_> = self
            .capture_frames
            .pop()
            .unwrap()
            .1
            .into_iter()
            .map(|(name, binding)| Capture {
                name,
                mutable: binding.mutable,
                declaration: binding.declaration,
                ty: binding.ty,
                mutex: binding.mutex,
                initializer: binding.initializer,
            })
            .collect();
        captures.sort_by(|a, b| a.name.cmp(&b.name));
        if captures
            .iter()
            .any(|capture| self.contains_future(&capture.ty))
        {
            return Checker::fail(
                "functions cannot capture futures; await the value before capturing it",
            );
        }
        self.captures.insert(e.id(), captures);
        Ok(Type::Function(
            f.params.iter().map(|p| p.ty.clone()).collect(),
            Box::new(f.return_type.clone()),
        ))
    }
    fn check_cast(&mut self, ty: &Type, value: &Expr, implicit: bool) -> Result<Type, Diagnostics> {
        self.validate_type(ty)?;
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.types.get(n).cloned()
        {
            // Give literals the base type's context (empty containers, unsigned
            // values and optional promotions need it before they can be checked).
            if matches!(
                value.unlocated(),
                Expr::Int(_)
                    | Expr::Float(_)
                    | Expr::String(_)
                    | Expr::Char(_)
                    | Expr::Bool(_)
                    | Expr::Array(_)
                    | Expr::Tuple(_)
                    | Expr::Map(_)
                    | Expr::None
            ) {
                self.expected(value, &base)?;
            } else {
                let from = self.expr(value)?;
                if from != *ty
                    && !matches!(&from, Type::Named(name, _) if matches!(self.types.get(name), Some(TypeInfo::Alias(inner)) if inner == ty))
                {
                    Checker::assignable(&base, &from)?;
                }
            }
            return Ok(ty.clone());
        }
        if *ty == named("byte") && matches!(value.unlocated(), Expr::Int(_)) {
            // Integer literals may use byte context; typed integer values may not.
            self.expected(value, ty)?;
            return Ok(ty.clone());
        }
        let from = self.expr(value)?;
        if !implicit
            && let Type::Named(n, _) = &from
            && matches!(self.types.get(n),Some(TypeInfo::Alias(base)) if base == ty)
        {
            return Ok(ty.clone());
        }
        if numeric(ty) && numeric(&from) {
            if ty == &from || numeric_cast_allowed(&from, ty) {
                return Ok(ty.clone());
            }
            return Checker::fail(format!("cannot cast `{from}` to `{ty}`"));
        }
        if *ty == named("str") {
            self.value_operation(&from, "string conversion")?;
        }
        if let (Type::Array(a, None), Type::Array(b, Some(_))) = (ty, &from)
            && a == b
        {
            return Ok(ty.clone());
        }
        let string_array = matches!(ty,Type::Array(t,None) if (**t == named("char") && from == named("str")) || (**t == named("byte") && (from == named("str") || from == named("char"))));
        let bool_integer = from == named("bool") && (*ty == named("int") || *ty == named("uint"));
        if ty != &from
            && *ty != named("str")
            && !string_array
            && !bool_integer
            && !(from == named("byte") && *ty == named("char"))
            && !(matches!(ty,Type::Array(t,None) if **t == named("byte"))
                && matches!(&from,Type::Named(n,_) if matches!(n.as_str(),"int"|"uint"|"float")))
        {
            return Checker::fail("this cast is not implemented");
        }
        Ok(ty.clone())
    }
    fn check_name(&mut self, e: &Expr, n: &str) -> Result<Type, Diagnostics> {
        if let Some((i, binding)) = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, s)| s.get(n).map(|v| (i, v.clone())))
        {
            if binding.mutex {
                return Checker::fail(format!("cannot read mutex `{n}` outside a lock scope"));
            }
            if i == 0 && binding.mutable && !binding.mutex {
                self.shared_accesses.insert(e.id());
            }
            self.constant_sources.insert(e.id(), binding.initializer);
            for (depth, captures) in &mut self.capture_frames {
                if i < *depth {
                    captures.insert(n.to_owned(), binding.clone());
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
                Some(_) | None => None,
            })
            .ok_or_else(|| Diagnostics::one(format!("unknown name `{n}`"), 0..0))
    }
    fn check_array(&mut self, xs: &[Expr]) -> Result<Type, Diagnostics> {
        if xs.is_empty() {
            return Checker::fail("cannot infer type of empty array");
        }
        let t = self.expr(&xs[0])?;
        for x in &xs[1..] {
            let actual = self.expr(x)?;
            Checker::assignable(&t, &actual)?;
        }
        Ok(Type::Array(Box::new(t), Some(xs.len())))
    }
    fn check_map(&mut self, xs: &[(Expr, Expr)]) -> Result<Type, Diagnostics> {
        if xs.is_empty() {
            return Checker::fail("cannot infer type of empty map");
        }
        let (k, v) = &xs[0];
        let kt = self.expr(k)?;
        self.value_operation(&kt, "map-key equality")?;
        let vt = self.expr(v)?;
        for (k, v) in &xs[1..] {
            let actual_key = self.expr(k)?;
            let actual_value = self.expr(v)?;
            Checker::assignable(&kt, &actual_key)?;
            Checker::assignable(&vt, &actual_value)?;
        }
        Ok(Type::Map(Box::new(kt), Box::new(vt)))
    }
    fn check_unary(&mut self, op: UnaryOp, value: &Expr) -> Result<Type, Diagnostics> {
        if op == UnaryOp::Neg
            && matches!(value.unlocated(), Expr::Int(text) if !text.ends_with('u') && integer(text).ok() == Some(1u64 << 63))
        {
            self.expression_types.insert(value.id(), named("uint"));
            return Ok(named("int"));
        }
        let t = self.expr(value)?;
        match op {
            UnaryOp::Not => Checker::assignable(&named("bool"), &t)?,
            UnaryOp::Neg | UnaryOp::BitNot => {
                if !numeric(&t) {
                    return Checker::fail("numeric unary operation required");
                }
                if op == UnaryOp::BitNot && t == named("float") {
                    return Checker::fail("bitwise operations require integers");
                }
            }
        }
        Ok(t)
    }
    fn check_binary(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<Type, Diagnostics> {
        let (l, r) = self.binary_operands(left, op, right)?;
        if op == BinaryOp::In {
            self.value_operation(&l, "equality")?;
            match &r {
                Type::Array(element, _) => Checker::assignable(element, &l)?,
                Type::Map(key, _) => Checker::assignable(key, &l)?,
                Type::Named(n, _)
                    if n == "str"
                        && matches!(&l, Type::Named(n, _) if n == "str" || n == "char") => {}
                _ => {
                    return Checker::fail("in requires a compatible container and element");
                }
            }
            return Ok(named("bool"));
        }
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            self.value_operation(&l, "equality")?;
        }
        if let (Type::Array(a, n), Type::Array(b, m)) = (&l, &r) {
            Checker::assignable(a, b)?;
            return match op {
                BinaryOp::Concat => Ok(Type::Array(
                    a.clone(),
                    n.zip(*m).and_then(|(n, m)| n.checked_add(m)),
                )),
                BinaryOp::Eq | BinaryOp::Ne => Ok(named("bool")),
                _ => Checker::fail("unsupported operator for arrays"),
            };
        }
        if op == BinaryOp::Concat && l != named("str") && !matches!(l, Type::Map(_, _)) {
            return Checker::fail("concatenation requires arrays, maps, or strings");
        }
        Checker::assignable(&l, &r)?;
        match op {
            BinaryOp::And | BinaryOp::Or => Checker::assignable(&named("bool"), &l)?,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge if !numeric(&l) => {
                return Checker::fail("ordered comparisons require numeric operands");
            }
            BinaryOp::Mod if !numeric(&l) || l == named("float") => {
                return Checker::fail("modulo requires integer operands");
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Pow => {
                if !numeric(&l) {
                    return Checker::fail("arithmetic requires numeric operands");
                }
            }
            BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr
                if (!numeric(&l) || l == named("float")) =>
            {
                return Checker::fail("bitwise operations require integers");
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
    fn binary_operands(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(Type, Type), Diagnostics> {
        let l = if matches!(left.unlocated(), Expr::Int(_))
            && !matches!(right.unlocated(), Expr::Int(_))
            && op != BinaryOp::In
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
        let r = if matches!(right.unlocated(), Expr::Int(_)) && numeric(&l) && l != named("float") {
            self.expected(right, &l)?;
            l.clone()
        } else {
            self.expr(right)?
        };
        Ok((l, r))
    }
    fn check_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Type, Diagnostics> {
        if let Expr::Name(name) = callee.unlocated() {
            let ty = match name.as_str() {
                "@args" => Some(Type::Array(Box::new(named("str")), None)),
                "@env" => Some(Type::Map(Box::new(named("str")), Box::new(named("str")))),
                "@target" => Some(Type::Tuple(vec![named("str"), named("str")])),
                _ => None,
            };
            if let Some(ty) = ty {
                if !args.is_empty() {
                    return Checker::fail(format!("{name} expects no arguments"));
                }
                return Ok(ty);
            }
        }
        if let Expr::Name(name) = callee.unlocated()
            && matches!(
                name.as_str(),
                "@print" | "@println" | "@eprint" | "@eprintln"
            )
        {
            for arg in args {
                let ty = self.expr(arg)?;
                self.value_operation(&ty, "string conversion")?;
            }
            return Ok(Type::void());
        }
        let callee_t = self.expr(callee)?;
        let Type::Function(params, ret) = callee_t else {
            return Checker::fail("called value is not a function");
        };
        if params.len() != args.len() {
            return Checker::fail("incorrect number of arguments");
        }
        for (p, a) in params.iter().zip(args) {
            self.expected(a, p)?;
        }
        Ok(*ret)
    }
    fn check_index(&mut self, object: &Expr, index: &Expr) -> Result<Type, Diagnostics> {
        let o = self.expr(object)?;
        if let Type::Map(key, value) = &o {
            self.expected(index, key)?;
            return Ok((**value).clone());
        }
        if let Type::Tuple(types) = &o {
            if let Expr::Int(text) = index.unlocated() {
                let n = usize::try_from(integer(text)?)
                    .map_err(|_| Diagnostics::one("tuple index out of bounds", 0..0))?;
                self.expr(index)?;
                return types
                    .get(n)
                    .cloned()
                    .ok_or_else(|| Diagnostics::one("tuple index out of bounds", 0..0));
            }
            return Checker::fail("tuple index must be an integer literal");
        }
        self.indexing += 1;
        let index_type = self.expr(index)?;
        self.indexing -= 1;
        if index_type != named("uint") && index_type != named("int") {
            return Checker::fail("array index must be int or uint");
        }
        match o {
            Type::Array(t, _) => Ok(*t),
            Type::Named(n, _) if n == "str" => Ok(named("char")),
            Type::Tuple(_) => Checker::fail("tuple index must be a compile-time value"),
            _ => Checker::fail("value is not indexable"),
        }
    }
    fn check_member(&mut self, object: &Expr, name: &str) -> Result<Type, Diagnostics> {
        let namespace = matches!(object.unlocated(), Expr::Name(n)
        if self.lookup(n).is_none() && matches!(self.types.get(n), Some(TypeInfo::Enum(_))));
        let o = if namespace {
            let Expr::Name(n) = object.unlocated() else {
                unreachable!()
            };
            let ty = named(n);
            self.expression_types.insert(object.id(), ty.clone());
            ty
        } else {
            self.expr(object)?
        };
        if let Type::Named(n, _) = &o {
            if let Some(TypeInfo::Enum(declaration)) = self.types.get(n) {
                if !namespace {
                    return Checker::fail(
                        "enum variants must be accessed through the enum type, not an enum value",
                    );
                }
                let Some(variant) = declaration.variants.iter().find(|v| v.name == name) else {
                    return Checker::fail(format!("unknown enum variant `{name}`"));
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
                    .find(|f| f.name == name)
                    .map(|f| f.ty.clone())
                    .ok_or_else(|| Diagnostics::one(format!("unknown field `{name}`"), 0..0));
            }
        }
        let has_len = matches!(o, Type::Array(_, _) | Type::Map(_, _))
            || matches!(o, Type::Named(ref n, _) if n == "str");
        if name == "len" && has_len {
            return Ok(named("uint"));
        }
        Checker::fail(format!("type does not have member `{name}`"))
    }
    fn check_if(
        &mut self,
        e: &Expr,
        subject: Option<&Expr>,
        arms: &[(Vec<Pattern>, Block)],
    ) -> Result<Type, Diagnostics> {
        let subject_type = if let Some(subject) = subject {
            self.expr(subject)?
        } else {
            named("bool")
        };
        if matches!(subject_type, Type::Map(_, _)) {
            return Checker::fail(
                "maps cannot be matched directly; use a bare `if` with comparisons",
            );
        }
        let mut wildcard = false;
        let mut booleans = HashSet::new();
        let mut bytes = HashSet::new();
        let mut variants = HashSet::new();
        let target = self
            .expression_types
            .get(&e.id())
            .filter(|ty| **ty != Type::void())
            .cloned();
        for (patterns, body) in arms {
            self.push();
            let mut bindings = None;
            for pattern in patterns {
                self.scopes.last_mut().unwrap().clear();
                let total = self.check_pattern(pattern, &subject_type)?;
                let current = self.scopes.last().unwrap();
                if bindings
                    .as_ref()
                    .is_some_and(|bindings| bindings != current)
                {
                    return Checker::fail(
                        "alternative patterns must bind the same names with the same types",
                    );
                }
                bindings = Some(current.clone());
                match pattern {
                    Pattern::Wildcard => wildcard = true,
                    Pattern::Literal(value) => {
                        if let Expr::Bool(b) = value.unlocated() {
                            booleans.insert(*b);
                        }
                        if subject_type == named("byte")
                            && let Expr::Int(text) = value.unlocated()
                        {
                            bytes.insert(integer(text)?);
                        }
                        if let Expr::Member { name, .. } = value.unlocated() {
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
        if !(wildcard
            || enum_complete
            || (subject_type == named("bool") && booleans.len() == 2)
            || (subject_type == named("byte") && bytes.len() == 256))
        {
            return Checker::fail("conditional is not exhaustive; add a `_` fallback branch");
        }
        if arms.is_empty() {
            return Ok(Type::void());
        }
        Ok(target.unwrap_or_else(Type::void))
    }
    fn check_try(&mut self, x: &Expr) -> Result<Type, Diagnostics> {
        if self
            .function_return
            .as_ref()
            .is_some_and(|t| !matches!(t, Type::ErrorUnion(_)))
        {
            return Checker::fail("try requires a throwing function return type (`!`)");
        }
        match self.expr(x)? {
            Type::ErrorUnion(inner) => Ok(*inner),
            _ => Checker::fail("try requires an error union"),
        }
    }
    fn check_else(&mut self, value: &Expr, fallback: &Block) -> Result<Type, Diagnostics> {
        let Type::Optional(inner) = self.expr(value)? else {
            return Checker::fail("else requires an optional value");
        };
        self.value_targets.push((*inner).clone());
        let result = self.value_block(fallback, &inner);
        self.value_targets.pop();
        result?;
        Ok(*inner)
    }
    fn check_catch(&mut self, value: &Expr, name: &str, body: &Block) -> Result<Type, Diagnostics> {
        let Type::ErrorUnion(inner) = self.expr(value)? else {
            return Checker::fail("catch requires an error union");
        };
        self.push();
        self.bind(name, named("error"), false);
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
    fn assignable(expected: &Type, got: &Type) -> Result<(), Diagnostics> {
        if let (Type::Array(a, None), Type::Array(b, _)) = (expected, got) {
            return Checker::assignable(a, b);
        }
        if expected == got || matches!(expected,Type::Optional(x)if **x==*got) {
            Ok(())
        } else {
            Checker::fail(format!("expected `{expected}`, found `{got}`"))
        }
    }
    fn value_block(&mut self, block: &Block, expected: &Type) -> Result<(), Diagnostics> {
        self.push();
        for (i, statement) in block.statements.iter().enumerate() {
            if i + 1 == block.statements.len()
                && let Stmt::Expr(value) = statement.unlocated()
            {
                self.expected(value, expected)
                    .map_err(|error| match statement.source() {
                        Some((path, span)) => error.at_source(path, span.clone()),
                        None => error,
                    })?;
                continue;
            }
            self.stmt(statement)?;
        }
        let exits = block.statements.last().is_some_and(|s| {
            matches!(
                s.unlocated(),
                Stmt::Expr(_) | Stmt::Break(Some(_), _) | Stmt::Return(_) | Stmt::Throw(_)
            )
        }) || !crate::flow::block_reaches_next(block, &self.expression_types);
        self.pop();
        if !exits {
            return Checker::fail(
                "value-producing branch must provide a value or exit the function",
            );
        }
        Ok(())
    }
    fn check_pattern(&mut self, p: &Pattern, ty: &Type) -> Result<bool, Diagnostics> {
        match p {
            Pattern::Wildcard => Ok(true),
            Pattern::Name(n) => {
                if let Some(binding) = self.lookup(n) {
                    if binding.mutex {
                        return Checker::fail(format!(
                            "cannot read mutex `{n}` outside a lock scope"
                        ));
                    }
                    self.value_operation(ty, "pattern equality")?;
                    Checker::assignable(ty, &binding.ty)?;
                    Ok(false)
                } else {
                    self.bind(n, ty.clone(), false);
                    Ok(true)
                }
            }
            Pattern::Literal(value) => {
                self.value_operation(ty, "pattern equality")?;
                self.expected(value, ty)?;
                Ok(false)
            }
            Pattern::Array(patterns) => {
                let Type::Array(element, size) = ty else {
                    return Checker::fail("array pattern requires an array subject");
                };
                if size.is_some_and(|size| size != patterns.len()) {
                    return Checker::fail("fixed-size array pattern has wrong length");
                }
                let mut total = size.is_some();
                for p in patterns {
                    total &= self.check_pattern(p, element)?;
                }
                Ok(total)
            }
            Pattern::Tuple(patterns) => {
                let Type::Tuple(types) = ty else {
                    return Checker::fail("tuple pattern requires a tuple subject");
                };
                if patterns.len() != types.len() {
                    return Checker::fail("tuple pattern has wrong arity");
                }
                let mut total = true;
                for (p, t) in patterns.iter().zip(types) {
                    total &= self.check_pattern(p, t)?;
                }
                Ok(total)
            }
            Pattern::Struct { name, fields } => {
                Checker::assignable(ty, &named(name))?;
                let Some(TypeInfo::Struct(declaration)) = self.types.get(name).cloned() else {
                    return Checker::fail("unknown struct pattern");
                };
                let mut total = true;
                let mut seen = HashSet::new();
                for (name, p) in fields {
                    if !seen.insert(name) {
                        return Checker::fail(format!(
                            "duplicate field `{name}` in struct pattern"
                        ));
                    }
                    let Some(f) = declaration.fields.iter().find(|f| f.name == *name) else {
                        return Checker::fail("unknown field in struct pattern");
                    };
                    total &= self.check_pattern(p, &f.ty)?;
                }
                Ok(total)
            }
            Pattern::Variant { name, values } => {
                let Some((owner, variant)) = name.rsplit_once('.') else {
                    return Checker::fail("invalid enum pattern");
                };
                Checker::assignable(ty, &named(owner))?;
                let Some(TypeInfo::Enum(declaration)) = self.types.get(owner).cloned() else {
                    return Checker::fail("unknown enum pattern");
                };
                let Some(v) = declaration.variants.iter().find(|v| v.name == variant) else {
                    return Checker::fail("unknown enum variant");
                };
                if values.len() != v.values.len() {
                    return Checker::fail("incorrect enum payload arity");
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
fn numeric_cast_allowed(from: &Type, to: &Type) -> bool {
    matches!((from, to), (Type::Named(from, _), Type::Named(to, _)) if matches!(
        (from.as_str(), to.as_str()),
        ("byte" | "float", "int" | "uint")
            | ("int", "uint" | "float")
            | ("uint", "int" | "float")
    ))
}
