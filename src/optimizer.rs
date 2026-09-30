//! Fuel-bounded, type-aware evaluation of pure expressions, functions and loops.
//! Failed/impure evaluation leaves the original program in place.
use crate::{
    ast::*,
    diagnostic::Diagnostics,
    sema::{CheckedModule, TypeInfo},
};
use std::collections::{HashMap, HashSet};

/// Embedding is mandatory compile-time evaluation, including in debug builds.
pub fn resolve_embeds(
    checked: CheckedModule,
    source: &std::path::Path,
) -> Result<CheckedModule, Diagnostics> {
    let mut has_embeds = false;
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| has_embeds |= matches!(e, Expr::Embed { .. }));
    }
    if !has_embeds {
        return Ok(checked);
    }
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ncc-embed".into())
            .stack_size(32 * 1024 * 1024)
            .spawn_scoped(scope, || embed_module(checked, source))
            .map_err(|e| Diagnostics::one(format!("cannot start embed evaluator: {e}"), 0..0))?
            .join()
            .map_err(|_| Diagnostics::one("embed evaluator panicked", 0..0))?
    })
}

fn embed_module(
    checked: CheckedModule,
    source: &std::path::Path,
) -> Result<CheckedModule, Diagnostics> {
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(f) = item {
                Some((f.name.clone(), f))
            } else {
                None
            }
        })
        .collect();
    let mut replacements = HashMap::new();
    let mut embeds = Vec::new();
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| {
            if matches!(e, Expr::Embed { .. }) {
                embeds.push(e);
            }
        });
    }
    for e in embeds {
        let Expr::Embed {
            source_path, span, ..
        } = e
        else {
            unreachable!()
        };
        let value = evaluate(e, &HashMap::new(), &functions, &checked, &mut 100_000)
            .map_err(|error| error.at_source(source_path, span.clone()))?;
        let Some(Value::Array(bytes)) = value else {
            return Err(Diagnostics::one(
                "@embed path must be a compile-time string; runtime values, side effects, or evaluation limits prevent evaluating this path", span.clone()
            ).at_source(source_path, span.clone()));
        };
        let bytes = bytes
            .into_iter()
            .map(|v| match v {
                Value::Byte(b) => b,
                _ => unreachable!(),
            })
            .collect();
        replacements.insert(e.id(), Expr::Bytes(bytes));
    }
    let mut module = checked.module;
    for item in &mut module.items {
        crate::visit::rewrite(item, &mut |e| {
            if let Some(value) = replacements.remove(&e.id()) {
                *e = value;
            }
        });
    }
    crate::sema::check(module, source)
}

fn embedded_bytes(path: &str, source: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let file = source
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(path);
    let read = || {
        let mut component_path = std::path::PathBuf::new();
        for component in file.components() {
            component_path.push(component);
            if std::fs::symlink_metadata(&component_path)?
                .file_type()
                .is_symlink()
            {
                return Err(std::io::Error::other("@embed does not follow symlinks"));
            }
        }
        if !std::fs::metadata(&file)?.is_file() {
            return Err(std::io::Error::other("@embed requires a regular file"));
        }
        std::fs::read(&file)
    };
    read()
        .map_err(|e| std::io::Error::new(e.kind(), format!("cannot embed {}: {e}", file.display())))
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Value {
    Void(Vec<Type>),
    Int(i64),
    Uint(u64),
    Byte(u8),
    Float(u64),
    Bool(bool),
    String(String),
    Char(String),
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Struct(String, Vec<(String, Value)>),
    Enum(String, String, Vec<Value>),
    Optional(Type, Option<Box<Value>>),
    Success(Type, Box<Value>),
    Failure(Type, String),
    Function(String),
    Closure(usize, Vec<(String, Value)>),
    Cell(usize),
}
// Match the C backend's %.17g formatting, including its explicit decimal suffix.
fn float_string(value: f64) -> Option<String> {
    if value.is_nan() {
        return Some("NaN".into());
    }
    if value.is_infinite() {
        return Some(
            if value.is_sign_negative() {
                "-inf"
            } else {
                "inf"
            }
            .into(),
        );
    }
    let scientific = format!("{value:.16e}");
    let (mantissa, exponent) = scientific.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    if !(-4..17).contains(&exponent) {
        return Some(format!(
            "{}e{exponent:+03}",
            mantissa.trim_end_matches('0').trim_end_matches('.')
        ));
    }
    let precision = (16 - exponent) as usize;
    let fixed = format!("{value:.precision$}");
    let mut result = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        fixed
    };
    if !result.contains('.') {
        result.push_str(".0");
    }
    Some(result)
}
impl Value {
    fn contains_cell(&self) -> bool {
        match self {
            Self::Cell(_) => true,
            Self::Closure(_, values) | Self::Struct(_, values) => {
                values.iter().any(|(_, value)| value.contains_cell())
            }
            Self::Array(values) | Self::Tuple(values) | Self::Enum(_, _, values) => {
                values.iter().any(Self::contains_cell)
            }
            Self::Map(values) => values
                .iter()
                .any(|(key, value)| key.contains_cell() || value.contains_cell()),
            Self::Optional(_, Some(value)) | Self::Success(_, value) => value.contains_cell(),
            _ => false,
        }
    }
    fn materializable(&self) -> bool {
        match self {
            Self::Closure(..) | Self::Cell(_) => false,
            Self::Array(values) | Self::Tuple(values) | Self::Enum(_, _, values) => {
                values.iter().all(Self::materializable)
            }
            Self::Map(values) => values
                .iter()
                .all(|(k, v)| k.materializable() && v.materializable()),
            Self::Struct(_, fields) => fields.iter().all(|(_, v)| v.materializable()),
            Self::Optional(_, Some(v)) | Self::Success(_, v) => v.materializable(),
            _ => true,
        }
    }
    fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Float(a), Self::Float(b)) => f64::from_bits(*a) == f64::from_bits(*b),
            (Self::Array(a), Self::Array(b)) | (Self::Tuple(a), Self::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.equals(b))
            }
            (Self::Map(a), Self::Map(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, value)| {
                        b.iter()
                            .any(|(other, result)| key.equals(other) && value.equals(result))
                    })
            }
            (Self::Struct(a, fields), Self::Struct(b, others)) => {
                a == b
                    && fields.len() == others.len()
                    && fields.iter().all(|(name, value)| {
                        others
                            .iter()
                            .any(|(other, result)| name == other && value.equals(result))
                    })
            }
            (Self::Enum(a, variant, values), Self::Enum(b, other, results)) => {
                a == b
                    && variant == other
                    && values.len() == results.len()
                    && values.iter().zip(results).all(|(a, b)| a.equals(b))
            }
            (Self::Optional(_, Some(a)), Self::Optional(_, Some(b)))
            | (Self::Success(_, a), Self::Success(_, b)) => a.equals(b),
            _ => self == other,
        }
    }
    fn expr(self) -> Expr {
        match self {
            Self::Void(aliases) => aliases.into_iter().rev().fold(
                constant_statement(Stmt::Return(None), Type::void()),
                |value, ty| Expr::Cast {
                    implicit: false,
                    ty,
                    value: Box::new(value),
                },
            ),
            Self::Int(n) if n < 0 => Expr::Unary {
                op: UnaryOp::Neg,
                value: Box::new(Expr::Int(n.unsigned_abs().to_string())),
            },
            Self::Int(n) => Expr::Int(n.to_string()),
            Self::Uint(n) => Expr::Int(format!("{n}u")),
            Self::Byte(n) => Expr::Cast {
                implicit: false,
                ty: Type::Named("byte".into(), vec![]),
                value: Box::new(Expr::Int(n.to_string())),
            },
            Self::Float(bits) => {
                let value = f64::from_bits(bits);
                Expr::Float(if value.is_finite() {
                    format!("{value:?}")
                } else {
                    float_string(value).expect("nonfinite floats have canonical spellings")
                })
            }
            Self::Bool(b) => Expr::Bool(b),
            Self::String(s) => Expr::String(s),
            Self::Char(s) => Expr::Char(s),
            Self::Array(values) => Expr::Array(values.into_iter().map(Value::expr).collect()),
            Self::Tuple(values) => Expr::Tuple(values.into_iter().map(Value::expr).collect()),
            Self::Map(values) => Expr::Map(
                values
                    .into_iter()
                    .map(|(key, value)| (key.expr(), value.expr()))
                    .collect(),
            ),
            Self::Struct(name, fields) => Expr::StructInit {
                name,
                fields: fields
                    .into_iter()
                    .map(|(name, value)| (name, value.expr()))
                    .collect(),
            },
            Self::Enum(name, variant, values) => {
                let member = Expr::Member {
                    object: Box::new(Expr::Name(name)),
                    name: variant,
                };
                if values.is_empty() {
                    member
                } else {
                    Expr::Call {
                        callee: Box::new(member),
                        args: values.into_iter().map(Value::expr).collect(),
                        generics: vec![],
                    }
                }
            }
            Self::Optional(_, None) => Expr::None,
            Self::Success(inner, _) if inner == Type::void() => {
                constant_statement(Stmt::Return(None), Type::ErrorUnion(Box::new(inner)))
            }
            Self::Optional(inner, Some(value)) | Self::Success(inner, value) => {
                constant_thunk(value.expr(), inner)
            }
            Self::Function(name) => Expr::Name(name),
            Self::Failure(inner, message) => constant_statement(
                Stmt::Throw(Expr::String(message)),
                Type::ErrorUnion(Box::new(inner)),
            ),
            Self::Closure(..) | Self::Cell(_) => {
                unreachable!("closure constants retain their original source")
            }
        }
    }
}
pub fn optimize(checked: CheckedModule) -> Result<Module, Diagnostics> {
    // The evaluator has an explicit depth limit. Give it a consistent stack
    // budget instead of inheriting a small editor or test-runner thread stack.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ncc-constants".into())
            .stack_size(32 * 1024 * 1024)
            .spawn_scoped(scope, || optimize_module(checked))
            .map_err(|error| {
                Diagnostics::one(format!("cannot start constant evaluator: {error}"), 0..0)
            })?
            .join()
            .map_err(|_| Diagnostics::one("constant evaluator panicked", 0..0))?
    })
}
fn optimize_module(checked: CheckedModule) -> Result<Module, Diagnostics> {
    let mut functions: HashMap<String, &Function> = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(f) = item {
                Some((f.name.clone(), f))
            } else {
                None
            }
        })
        .collect();
    let mut env = HashMap::new();
    let mut replacements = Vec::new();
    for (index, item) in checked.module.items.iter().enumerate() {
        match item {
            Item::Global(v) => {
                replacements.push((index, fold(&v.value, &env, &functions, &checked)?));
                let constant = if !v.mutable && !v.mutex {
                    evaluate(&v.value, &env, &functions, &checked, &mut 100_000)?
                } else {
                    None
                };
                for n in v.binding_names() {
                    env.remove(n);
                    functions.remove(n);
                }
                if let Some(value) = constant
                    && let Some(value) = declaration_value(&v.pattern, &v.ty, value)
                {
                    let _ = bind_declaration(&v.pattern, value, &mut env, &mut Vec::new());
                }
            }
            Item::Statement(statement) => {
                if let Stmt::Expr(e) = statement.unlocated() {
                    replacements.push((index, fold(e, &env, &functions, &checked)?))
                }
            }
            _ => {}
        }
    }
    let mut body_replacements = expression_constants(&checked, &functions);
    let mut module = checked.module;
    for item in &mut module.items {
        crate::visit::rewrite(item, &mut |e| {
            if let Some(value) = body_replacements.remove(&e.id()) {
                *e = value;
            }
        });
    }
    for (index, replacement) in replacements {
        if let Some(replacement) = replacement {
            match &mut module.items[index] {
                Item::Global(v) => v.value = replacement,
                Item::Statement(statement) => match statement.unlocated_mut() {
                    Stmt::Expr(e) => *e = replacement,
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            }
        }
    }
    let mut reachable = HashSet::new();
    let references = |item: &Item, names: &mut HashSet<String>| {
        crate::visit::item(item, &mut |e| {
            if let Expr::Name(n) = e {
                names.insert(n.clone());
            }
        });
    };
    for item in &module.items {
        if !matches!(item, Item::Function(_)) {
            references(item, &mut reachable);
        }
    }
    loop {
        let before = reachable.len();
        for item in &module.items {
            if let Item::Function(f) = item
                && reachable.contains(&f.name)
            {
                references(item, &mut reachable);
            }
        }
        if reachable.len() == before {
            break;
        }
    }
    module
        .items
        .retain(|item| !matches!(item, Item::Function(f) if !reachable.contains(&f.name)));
    Ok(module)
}
fn expression_constants(
    checked: &CheckedModule,
    functions: &HashMap<String, &Function>,
) -> HashMap<usize, Expr> {
    let mut replacements = HashMap::new();
    let mut preserved = HashSet::new();
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| {
            if let Expr::Async(call) = e {
                preserved.insert(call.id());
            }
            // Pattern literals must retain their syntax for exhaustiveness
            // checking and pattern lowering (not become constant thunks).
            if let Expr::If { arms, .. } = e {
                for (patterns, _) in arms {
                    for pattern in patterns {
                        crate::visit::pattern(pattern, &mut |value| {
                            preserved.insert(value.id());
                        });
                    }
                }
            }
        });
    }
    for item in &checked.module.items {
        let mut fuel = 100_000;
        crate::visit::item(item, &mut |e| {
            if fuel == 0
                || preserved.contains(&e.id())
                || !matches!(
                    e,
                    Expr::Call { .. }
                        | Expr::Cast { .. }
                        | Expr::Unary { .. }
                        | Expr::Binary { .. }
                        | Expr::Index { .. }
                        | Expr::Member { .. }
                        | Expr::If { .. }
                        | Expr::Else { .. }
                        | Expr::Catch { .. }
                        | Expr::Try(_)
                )
            {
                return;
            }
            // Failed evaluation may be unreachable at runtime: keep it intact.
            if let Ok(Some(value)) = evaluate(e, &HashMap::new(), functions, checked, &mut fuel)
                && value.materializable()
            {
                replacements.insert(e.id(), materialize(value, e, checked));
            }
        });
    }
    replacements
}
fn fold(
    e: &Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, &Function>,
    checked: &CheckedModule,
) -> Result<Option<Expr>, Diagnostics> {
    // Only fold whole pure evaluations. Do not rewrite expressions inside a
    // short-circuited or potentially effectful expression independently.
    if let Some(v) =
        evaluate(e, env, functions, checked, &mut 100_000)?.filter(Value::materializable)
    {
        return Ok(Some(materialize(v, e, checked)));
    } else if let Expr::Call {
        callee,
        args,
        generics,
    } = e.unlocated()
        && matches!(callee.unlocated(),Expr::Name(n) if n.starts_with('@'))
    {
        let args = args
            .iter()
            .map(|arg| {
                evaluate(arg, env, functions, checked, &mut 100_000).map(|value| {
                    value
                        .filter(Value::materializable)
                        .map_or_else(|| arg.clone(), |value| materialize(value, arg, checked))
                })
            })
            .collect::<Result<_, _>>()?;
        return Ok(Some(Expr::Call {
            callee: callee.clone(),
            args,
            generics: generics.clone(),
        }));
    }
    Ok(None)
}
fn materialize(value: Value, original: &Expr, checked: &CheckedModule) -> Expr {
    if matches!(value, Value::Void(_))
        || matches!(value, Value::Success(ref inner, _) if *inner == Type::void())
    {
        return value.expr();
    }
    let expr = value.expr();
    let ty = checked.expression_types.get(&original.id());
    // A typed constant thunk supplies context for empty arrays, optionals, and
    // nominal values even when the caller (e.g. @println) supplies no type.
    if let Some(ty) = ty
        && !matches!(ty, Type::Named(name, _) if matches!(name.as_str(), "int" | "uint" | "byte" | "float" | "str" | "char" | "bool"))
    {
        let mut base = ty;
        let mut aliases = Vec::new();
        while let Type::Named(name, _) = base
            && let Some(TypeInfo::Alias(inner)) = checked.types.get(name)
        {
            aliases.push(base.clone());
            base = inner;
        }
        return aliases
            .into_iter()
            .rev()
            .fold(constant_thunk(expr, base.clone()), |value, ty| Expr::Cast {
                ty,
                value: Box::new(value),
                implicit: false,
            });
    }
    expr
}
fn constant_thunk(expr: Expr, ty: Type) -> Expr {
    constant_statement(Stmt::Return(Some(expr)), ty)
}
fn constant_statement(statement: Stmt, ty: Type) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Lambda(Box::new(Function {
            source_path: "<constant>".into(),
            span: 0..0,
            public: false,
            name: "constant".into(),
            generics: vec![],
            params: vec![],
            return_type: ty,
            body: Block {
                statements: vec![statement],
            },
        }))),
        args: vec![],
        generics: vec![],
    }
}
fn evaluate(
    e: &Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, &Function>,
    checked: &CheckedModule,
    fuel: &mut usize,
) -> Result<Option<Value>, Diagnostics> {
    let mut lambdas = HashMap::new();
    let mut expressions = HashMap::new();
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| {
            expressions.insert(e.id(), e);
            if let Expr::Lambda(f) = e {
                lambdas.insert(e.id(), &**f);
            }
        });
    }
    let mut evaluator = Evaluator {
        functions,
        lambdas,
        expressions,
        embed_error: None,
        checked,
        fuel,
        memo: HashMap::new(),
        cells: Vec::new(),
        depth: 0,
        arithmetic_failure: false,
        arithmetic_location: None,
        flow: None,
        indices: vec![],
    };
    let value = evaluator.evaluate(e, &mut env.clone());
    if let Some(error) = evaluator.embed_error {
        Err(error)
    } else if evaluator.arithmetic_failure {
        let error = Diagnostics::one(
            "constant evaluation failed: integer overflow, division by zero, invalid shift/exponent, or numeric cast out of range",
            0..0,
        );
        Err(if let Some(location) = evaluator.arithmetic_location {
            error.at_source(&location.path, location.span)
        } else {
            error
        })
    } else {
        // Cell identities are private to this evaluation, never reusable constants.
        Ok(value.filter(|value| !value.contains_cell()))
    }
}
struct Evaluator<'a> {
    functions: &'a HashMap<String, &'a Function>,
    checked: &'a CheckedModule,
    fuel: &'a mut usize,
    lambdas: HashMap<usize, &'a Function>,
    expressions: HashMap<usize, &'a Expr>,
    embed_error: Option<Diagnostics>,
    memo: HashMap<(Value, Vec<Value>), Value>,
    cells: Vec<Value>,
    depth: usize,
    arithmetic_failure: bool,
    arithmetic_location: Option<SourceLocation>,
    // A return/throw inside a value expression exits its enclosing function.
    flow: Option<Flow>,
    indices: Vec<u64>,
}
impl Evaluator<'_> {
    fn constant_binding(&mut self, key: usize, name: &str) -> Option<Value> {
        let initializer = *self.expressions.get(&key)?;
        let value = self.evaluate(initializer, &mut HashMap::new())?;
        // Re-evaluating an outer initializer must not recreate shared state.
        if value.contains_cell() {
            return None;
        }
        if let Some((pattern, ty)) = self.checked.constant_patterns.get(&key) {
            let value = declaration_value(pattern, ty, value)?;
            let mut bindings = HashMap::new();
            bind_declaration(pattern, value, &mut bindings, &mut Vec::new())?;
            return bindings.remove(name);
        }
        Some(value)
    }
    fn index(
        &mut self,
        index: &Expr,
        object: &Value,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        let length = match object {
            Value::Array(values) | Value::Tuple(values) => values.len(),
            Value::Map(entries) => entries.len(),
            Value::String(text) => crate::unicode::boundaries(text).len(),
            _ => return None,
        };
        self.indices.push(length as u64);
        let result = self.evaluate(index, env);
        self.indices.pop();
        result
    }
    fn place(
        &mut self,
        e: &Expr,
        env: &mut HashMap<String, Value>,
        path: &mut Vec<Access>,
    ) -> Option<String> {
        match e.unlocated() {
            Expr::Name(name) => Some(name.clone()),
            Expr::Member { object, name } => {
                let root = self.place(object, env, path)?;
                path.push(Access::Field(name.clone()));
                Some(root)
            }
            Expr::Index { object, index } => {
                let root = self.place(object, env, path)?;
                let object = self.evaluate(object, env)?;
                path.push(Access::Index(self.index(index, &object, env)?));
                Some(root)
            }
            _ => None,
        }
    }
    fn coerce(&self, value: Value, ty: &Type) -> Option<Value> {
        if matches!(value, Value::Void(_)) && self.base_type(ty) == &Type::void() {
            let mut aliases = Vec::new();
            let mut current = ty;
            while *current != Type::void() {
                aliases.push(current.clone());
                let Type::Named(name, _) = current else {
                    return None;
                };
                let TypeInfo::Alias(base) = self.checked.types.get(name)? else {
                    return None;
                };
                current = base;
            }
            return Some(Value::Void(aliases));
        }
        match (value, self.base_type(ty)) {
            (value @ Value::Failure(_, _), Type::ErrorUnion(inner)) if matches!(&value, Value::Failure(actual, _) if actual == &**inner) => {
                Some(value)
            }
            (value @ Value::Optional(_, _), Type::Optional(inner)) if matches!(&value, Value::Optional(actual, _) if actual == &**inner) => {
                Some(value)
            }
            (value, Type::Optional(inner)) => Some(Value::Optional(
                (**inner).clone(),
                Some(Box::new(self.coerce(value, inner)?)),
            )),
            (value @ Value::Success(_, _), Type::ErrorUnion(inner)) if matches!(&value, Value::Success(actual, _) if actual == &**inner) => {
                Some(value)
            }
            (value, Type::ErrorUnion(inner)) => Some(Value::Success(
                (**inner).clone(),
                Box::new(self.coerce(value, inner)?),
            )),
            (Value::Array(values), Type::Array(inner, _)) => Some(Value::Array(
                values
                    .into_iter()
                    .map(|v| self.coerce(v, inner))
                    .collect::<Option<_>>()?,
            )),
            (Value::Array(values), Type::Map(_, _)) if values.is_empty() => {
                Some(Value::Map(vec![]))
            }
            (Value::Tuple(values), Type::Tuple(types)) => Some(Value::Tuple(
                values
                    .into_iter()
                    .zip(types)
                    .map(|(v, ty)| self.coerce(v, ty))
                    .collect::<Option<_>>()?,
            )),
            (Value::Map(entries), Type::Map(key, value)) => Some(Value::Map(
                entries
                    .into_iter()
                    .map(|(k, v)| Some((self.coerce(k, key)?, self.coerce(v, value)?)))
                    .collect::<Option<_>>()?,
            )),
            (Value::Struct(name, fields), Type::Named(_, _)) => {
                let TypeInfo::Struct(declaration) = self.checked.types.get(&name)? else {
                    return None;
                };
                let fields = fields
                    .into_iter()
                    .map(|(name, value)| {
                        let ty = &declaration
                            .fields
                            .iter()
                            .find(|field| field.name == name)?
                            .ty;
                        Some((name, self.coerce(value, ty)?))
                    })
                    .collect::<Option<_>>()?;
                Some(Value::Struct(name, fields))
            }
            (Value::Enum(name, variant, values), Type::Named(_, _)) => {
                let TypeInfo::Enum(declaration) = self.checked.types.get(&name)? else {
                    return None;
                };
                let types = &declaration
                    .variants
                    .iter()
                    .find(|v| v.name == variant)?
                    .values;
                let values = values
                    .into_iter()
                    .zip(types)
                    .map(|(value, ty)| self.coerce(value, ty))
                    .collect::<Option<_>>()?;
                Some(Value::Enum(name, variant, values))
            }
            (value, _) => Some(value),
        }
    }
    fn string(&self, value: &Value, ty: &Type) -> Option<String> {
        let ty = self.base_type(ty);
        let sequence = |values: &[Value], types: &[Type], quoted: bool| {
            values
                .iter()
                .zip(types)
                .map(|(value, ty)| {
                    let text = self.string(value, ty)?;
                    Some(if quoted && *ty == Type::Named("str".into(), vec![]) {
                        format!("\"{text}\"")
                    } else {
                        text
                    })
                })
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.join(", "))
        };
        Some(match (value, ty) {
            (Value::String(s) | Value::Char(s), _) => s.clone(),
            (Value::Int(n), _) => n.to_string(),
            (Value::Uint(n), _) => n.to_string(),
            (Value::Byte(n), _) => n.to_string(),
            (Value::Float(bits), _) => float_string(f64::from_bits(*bits))?,
            (Value::Bool(b), _) => b.to_string(),
            (Value::Optional(_, None), _) => "none".into(),
            (Value::Failure(_, message), _) => format!("error: {message}"),
            (Value::Success(inner, value), _) => self.string(value, inner)?,
            (Value::Optional(_, Some(value)), Type::Optional(inner)) => {
                self.string(value, inner)?
            }
            (Value::Array(values), Type::Array(inner, _)) => {
                let types = vec![(**inner).clone(); values.len()];
                format!("[{}]", sequence(values, &types, true)?)
            }
            (Value::Tuple(values), Type::Tuple(types)) => {
                format!("({})", sequence(values, types, false)?)
            }
            (Value::Map(entries), Type::Map(key, val)) => {
                let parts = entries
                    .iter()
                    .map(|(k, v)| {
                        Some(format!(
                            "{}: {}",
                            self.string(k, key)?,
                            self.string(v, val)?
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                format!("[{}]", parts.join(", "))
            }
            (Value::Struct(name, values), _) => {
                let TypeInfo::Struct(declaration) = self.checked.types.get(name)? else {
                    return None;
                };
                let parts = declaration
                    .fields
                    .iter()
                    .map(|field| {
                        let (_, value) = values.iter().find(|(name, _)| *name == field.name)?;
                        Some(format!(
                            ".{} = {}",
                            field.name,
                            self.string(value, &field.ty)?
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                format!("{name}{{{}}}", parts.join(", "))
            }
            (Value::Enum(name, variant, values), _) => {
                let TypeInfo::Enum(declaration) = self.checked.types.get(name)? else {
                    return None;
                };
                let types = &declaration
                    .variants
                    .iter()
                    .find(|v| v.name == *variant)?
                    .values;
                if values.is_empty() {
                    format!("{name}.{variant}")
                } else {
                    format!("{name}.{variant}({})", sequence(values, types, true)?)
                }
            }
            _ => return None,
        })
    }
    fn cast(&mut self, ty: &Type, from: &Type, value: Value) -> Option<Value> {
        if *self.base_type(ty) == Type::Named("str".into(), vec![]) {
            return self.string(&value, from).map(Value::String);
        }
        if let Type::Array(inner, None) = self.base_type(ty) {
            if **inner == Type::Named("byte".into(), vec![]) {
                let bytes = match value {
                    Value::Int(n) => n.to_le_bytes().to_vec(),
                    Value::Uint(n) | Value::Float(n) => n.to_le_bytes().to_vec(),
                    Value::String(s) | Value::Char(s) => s.into_bytes(),
                    Value::Array(values) => return Some(Value::Array(values)),
                    _ => return None,
                };
                return Some(Value::Array(bytes.into_iter().map(Value::Byte).collect()));
            }
            if **inner == Type::Named("char".into(), vec![])
                && let Value::String(s) = value
            {
                let starts = crate::unicode::boundaries(&s);
                return Some(Value::Array(
                    starts
                        .iter()
                        .enumerate()
                        .map(|(i, start)| {
                            Value::Char(
                                s[*start..starts.get(i + 1).copied().unwrap_or(s.len())].into(),
                            )
                        })
                        .collect(),
                ));
            }
            return if let Value::Array(_) = value {
                Some(value)
            } else {
                None
            };
        }
        let Type::Named(name, _) = self.base_type(ty) else {
            return None;
        };
        let integer = match &value {
            Value::Int(n) => Some(*n as i128),
            Value::Uint(n) => Some(*n as i128),
            Value::Byte(n) => Some(*n as i128),
            Value::Bool(b) => Some(i128::from(*b)),
            Value::Float(bits) => {
                let n = f64::from_bits(*bits);
                let fits = match name.as_str() {
                    "byte" => (0.0..=255.0).contains(&n),
                    "uint" => (0.0..18446744073709551616.0).contains(&n),
                    _ => (-9223372036854775808.0..9223372036854775808.0).contains(&n),
                };
                (n.is_finite() && fits).then(|| n.trunc() as i128)
            }
            _ => None,
        };
        let result = match name.as_str() {
            "int" => integer.and_then(|n| i64::try_from(n).ok()).map(Value::Int),
            "uint" => integer.and_then(|n| u64::try_from(n).ok()).map(Value::Uint),
            "byte" => integer.and_then(|n| u8::try_from(n).ok()).map(Value::Byte),
            "float" => match value {
                Value::Float(_) => Some(value),
                Value::Int(n) => Some(Value::Float((n as f64).to_bits())),
                Value::Uint(n) => Some(Value::Float((n as f64).to_bits())),
                Value::Byte(n) => Some(Value::Float((n as f64).to_bits())),
                _ => None,
            },
            "char" if matches!(value, Value::Char(_)) => Some(value),
            "char" if matches!(value, Value::Byte(_)) => {
                let Value::Byte(value) = value else {
                    unreachable!()
                };
                Some(Value::Char(char::from(value).to_string()))
            }
            "bool" if matches!(value, Value::Bool(_)) => Some(value),
            _ => None,
        };
        if result.is_none() && matches!(name.as_str(), "int" | "uint" | "byte") {
            self.arithmetic_failure = true;
        }
        result
    }
    fn unsigned(&mut self, a: u64, b: u64, op: BinaryOp, byte: bool) -> Option<Value> {
        use BinaryOp::*;
        let comparison = match op {
            Eq => Some(a == b),
            Ne => Some(a != b),
            Lt => Some(a < b),
            Le => Some(a <= b),
            Gt => Some(a > b),
            Ge => Some(a >= b),
            _ => None,
        };
        if let Some(value) = comparison {
            return Some(Value::Bool(value));
        }
        let result = match op {
            Add => a.checked_add(b),
            Sub => a.checked_sub(b),
            Mul => a.checked_mul(b),
            Div => a.checked_div(b),
            Mod => a.checked_rem(b),
            Pow => {
                if a <= 1 {
                    Some(if b == 0 { 1 } else { a })
                } else {
                    u32::try_from(b).ok().and_then(|b| a.checked_pow(b))
                }
            }
            BitAnd => Some(a & b),
            BitOr => Some(a | b),
            BitXor => Some(a ^ b),
            Shl => u32::try_from(b)
                .ok()
                .filter(|b| *b < if byte { 8 } else { 64 })
                .and_then(|b| u64::try_from((a as u128) << b).ok()),
            Shr => u32::try_from(b)
                .ok()
                .filter(|b| *b < if byte { 8 } else { 64 })
                .and_then(|b| a.checked_shr(b)),
            _ => return None,
        }
        .filter(|n| !byte || *n <= u8::MAX as u64);
        if result.is_none() {
            self.arithmetic_failure = true;
        }
        result.map(|n| {
            if byte {
                Value::Byte(n as u8)
            } else {
                Value::Uint(n)
            }
        })
    }
    fn float(&mut self, a: f64, b: f64, op: BinaryOp) -> Option<Value> {
        use BinaryOp::*;
        let comparison = match op {
            Eq => Some(a == b),
            Ne => Some(a != b),
            Lt => Some(a < b),
            Le => Some(a <= b),
            Gt => Some(a > b),
            Ge => Some(a >= b),
            _ => None,
        };
        if let Some(value) = comparison {
            return Some(Value::Bool(value));
        }
        let result = match op {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Div => a / b,
            Mod => a % b,
            Pow => a.powf(b),
            _ => return None,
        };
        Some(Value::Float(result.to_bits()))
    }
    fn base_type<'a>(&'a self, ty: &'a Type) -> &'a Type {
        if let Type::Named(name, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(name)
        {
            self.base_type(base)
        } else {
            ty
        }
    }
    fn expr_type(&self, e: &Expr) -> Option<&Type> {
        self.checked
            .expression_types
            .get(&e.id())
            .map(|ty| self.base_type(ty))
    }
    fn evaluate(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        if self.depth >= 512 {
            return None;
        }
        self.depth += 1;
        let result = self.expression(e, env).and_then(|value| {
            if let Some(ty) = self.checked.expression_types.get(&e.id()) {
                self.coerce(value, ty)
            } else {
                Some(value)
            }
        });
        self.depth -= 1;
        if self.arithmetic_failure && self.arithmetic_location.is_none() {
            self.arithmetic_location = e.location().cloned();
        }
        result
    }
    fn expression(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        let e = e.unlocated();
        let fuel = &mut *self.fuel;
        *fuel = fuel.checked_sub(1)?;
        if let Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } = e
            && let Expr::Int(n) = value.unlocated()
            && crate::lexer::integer(n).ok()? == 1u64 << 63
        {
            return Some(Value::Int(i64::MIN));
        }
        match e {
            Expr::Int(s) => {
                let n = crate::lexer::integer(s).ok()?;
                match self.expr_type(e) {
                    Some(Type::Named(name, _)) if name == "uint" => Some(Value::Uint(n)),
                    Some(Type::Named(name, _)) if name == "byte" => {
                        u8::try_from(n).ok().map(Value::Byte)
                    }
                    _ => i64::try_from(n).ok().map(Value::Int),
                }
            }
            Expr::Float(s) => {
                let value: f64 = s.parse().ok()?;
                Some(Value::Float(value.to_bits()))
            }
            Expr::Bool(b) => Some(Value::Bool(*b)),
            Expr::Lambda(_) => {
                let key = e.id();
                let captures = self.checked.captures.get(&key)?;
                let values = captures
                    .iter()
                    .map(|capture| {
                        if capture.mutex {
                            return None;
                        }
                        if capture.mutable {
                            return match env.get(&capture.name)? {
                                value @ Value::Cell(_) => {
                                    Some((capture.name.clone(), value.clone()))
                                }
                                _ => None,
                            };
                        }
                        let value = env.get(&capture.name).cloned().or_else(|| {
                            self.constant_binding(capture.initializer?, &capture.name)
                        })?;
                        Some((capture.name.clone(), value))
                    })
                    .collect::<Option<_>>()?;
                Some(Value::Closure(key, values))
            }
            Expr::Bytes(bytes) => Some(Value::Array(
                bytes.iter().copied().map(Value::Byte).collect(),
            )),
            Expr::Embed {
                path,
                source_path,
                span,
            } => {
                let Value::String(path) = self.evaluate(path, env)? else {
                    return None;
                };
                match embedded_bytes(&path, source_path) {
                    Ok(bytes) => Some(Value::Array(bytes.into_iter().map(Value::Byte).collect())),
                    Err(error) => {
                        self.embed_error = Some(
                            Diagnostics::one(error.to_string(), span.clone())
                                .at_source(source_path, span.clone()),
                        );
                        None
                    }
                }
            }
            Expr::String(s) => Some(Value::String(s.clone())),
            Expr::Char(s) => Some(Value::Char(s.clone())),
            Expr::Array(values) => {
                let value = Value::Array(
                    values
                        .iter()
                        .map(|v| self.evaluate(v, env))
                        .collect::<Option<_>>()?,
                );
                self.coerce(value, self.expr_type(e)?)
            }
            Expr::Tuple(values) => Some(Value::Tuple(
                values
                    .iter()
                    .map(|v| self.evaluate(v, env))
                    .collect::<Option<_>>()?,
            )),
            Expr::Map(entries) => {
                let mut result: Vec<(Value, Value)> = vec![];
                for (key, value) in entries {
                    let key = self.evaluate(key, env)?;
                    let value = self.evaluate(value, env)?;
                    if let Some((_, existing)) =
                        result.iter_mut().find(|(other, _)| key.equals(other))
                    {
                        *existing = value;
                    } else {
                        result.push((key, value));
                    }
                }
                Some(Value::Map(result))
            }
            Expr::StructInit { name, fields } => Some(Value::Struct(
                name.clone(),
                fields
                    .iter()
                    .map(|(name, value)| Some((name.clone(), self.evaluate(value, env)?)))
                    .collect::<Option<_>>()?,
            )),
            Expr::None => {
                let Type::Optional(inner) = self.expr_type(e)? else {
                    return None;
                };
                Some(Value::Optional((**inner).clone(), None))
            }
            Expr::Member { object, name } => {
                if let Expr::Name(owner) = object.unlocated()
                    && let Some(TypeInfo::Enum(_)) = self.checked.types.get(owner)
                {
                    return Some(Value::Enum(owner.clone(), name.clone(), vec![]));
                }
                match self.evaluate(object, env)? {
                    Value::Struct(_, fields) => fields
                        .into_iter()
                        .find(|(field, _)| field == name)
                        .map(|(_, value)| value),
                    Value::Array(values) if name == "len" => Some(Value::Uint(values.len() as u64)),
                    Value::Map(values) if name == "len" => Some(Value::Uint(values.len() as u64)),
                    Value::String(value) if name == "len" => {
                        Some(Value::Uint(crate::unicode::boundaries(&value).len() as u64))
                    }
                    _ => None,
                }
            }
            Expr::Index { object, index } => {
                let object = self.evaluate(object, env)?;
                let index = self.index(index, &object, env)?;
                if let Value::Map(entries) = object {
                    return entries
                        .into_iter()
                        .find(|(key, _)| key.equals(&index))
                        .map(|(_, value)| value);
                }
                let n = match index {
                    Value::Int(n) => usize::try_from(n).ok()?,
                    Value::Uint(n) => usize::try_from(n).ok()?,
                    _ => return None,
                };
                match object {
                    Value::Array(values) | Value::Tuple(values) => values.get(n).cloned(),
                    Value::String(value) => {
                        let bounds = crate::unicode::boundaries(&value);
                        Some(Value::Char(
                            value
                                .get(
                                    *bounds.get(n)?
                                        ..bounds.get(n + 1).copied().unwrap_or(value.len()),
                                )?
                                .into(),
                        ))
                    }
                    _ => None,
                }
            }
            Expr::Cast { ty, value, .. } => {
                let from = self.expr_type(value)?.clone();
                let value = self.evaluate(value, env)?;
                if self.base_type(ty) == self.base_type(&from) {
                    self.coerce(value, ty)
                } else {
                    self.cast(ty, &from, value)
                }
            }
            Expr::Name(n) if n == "$" => self
                .indices
                .last()
                .and_then(|n| n.checked_sub(1))
                .map(Value::Uint),
            Expr::Name(n) => {
                if let Some(value) = env.get(n) {
                    return match value {
                        Value::Cell(index) => self.cells.get(*index).cloned(),
                        value => Some(value.clone()),
                    };
                }
                if let Some(key) = self.checked.constant_sources.get(&e.id()) {
                    return self.constant_binding((*key)?, n);
                }
                self.functions
                    .contains_key(n)
                    .then(|| Value::Function(n.clone()))
            }
            Expr::Unary { op, value } => match (op, self.evaluate(value, env)?) {
                (UnaryOp::Neg, Value::Int(v)) => {
                    let result = v.checked_neg();
                    if result.is_none() {
                        self.arithmetic_failure = true;
                    }
                    result.map(Value::Int)
                }
                (UnaryOp::Neg, Value::Float(v)) => {
                    Some(Value::Float((-f64::from_bits(v)).to_bits()))
                }
                (UnaryOp::Not, Value::Bool(b)) => Some(Value::Bool(!b)),
                (UnaryOp::BitNot, Value::Int(v)) => Some(Value::Int(!v)),
                (UnaryOp::BitNot, Value::Uint(v)) => Some(Value::Uint(!v)),
                (UnaryOp::BitNot, Value::Byte(v)) => Some(Value::Byte(!v)),
                _ => None,
            },
            Expr::Binary { left, op, right } => {
                let left = self.evaluate(left, env)?;
                if *op == BinaryOp::And && left == Value::Bool(false) {
                    return Some(left);
                }
                if *op == BinaryOp::Or && left == Value::Bool(true) {
                    return Some(left);
                }
                let right = self.evaluate(right, env)?;
                if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && !matches!(left, Value::Float(_)) {
                    let equal = left.equals(&right);
                    return Some(Value::Bool(if *op == BinaryOp::Eq {
                        equal
                    } else {
                        !equal
                    }));
                }
                match (left, right) {
                    (Value::Map(mut a), Value::Map(b)) if *op == BinaryOp::Concat => {
                        for (key, value) in b {
                            if let Some((_, existing)) =
                                a.iter_mut().find(|(other, _)| other.equals(&key))
                            {
                                *existing = value;
                            } else {
                                a.push((key, value));
                            }
                        }
                        Some(Value::Map(a))
                    }
                    (Value::Array(mut a), Value::Array(b)) if *op == BinaryOp::Concat => {
                        a.extend(b);
                        Some(Value::Array(a))
                    }
                    (value, Value::Array(values)) if *op == BinaryOp::In => Some(Value::Bool(
                        values.iter().any(|element| element.equals(&value)),
                    )),
                    (value, Value::Map(values)) if *op == BinaryOp::In => Some(Value::Bool(
                        values.iter().any(|(key, _)| key.equals(&value)),
                    )),
                    (Value::Int(a), Value::Int(b)) => {
                        use BinaryOp::*;
                        let result = match op {
                            Add => a.checked_add(b).map(Value::Int),
                            Sub => a.checked_sub(b).map(Value::Int),
                            Mul => a.checked_mul(b).map(Value::Int),
                            Div => a.checked_div(b).map(Value::Int),
                            Mod => a.checked_rem(b).map(Value::Int),
                            Pow if (-1..=1).contains(&a) && b >= 0 => Some(Value::Int(if b == 0 {
                                1
                            } else if a == -1 {
                                if b % 2 == 0 { 1 } else { -1 }
                            } else {
                                a
                            })),
                            Pow => u32::try_from(b)
                                .ok()
                                .and_then(|b| a.checked_pow(b))
                                .map(Value::Int),
                            BitAnd => Some(Value::Int(a & b)),
                            BitOr => Some(Value::Int(a | b)),
                            BitXor => Some(Value::Int(a ^ b)),
                            Shl => u32::try_from(b)
                                .ok()
                                .filter(|b| *b < 64)
                                .and_then(|b| i64::try_from((a as i128) << b).ok())
                                .map(Value::Int),
                            Shr => u32::try_from(b)
                                .ok()
                                .and_then(|b| a.checked_shr(b))
                                .map(Value::Int),
                            Eq => Some(Value::Bool(a == b)),
                            Ne => Some(Value::Bool(a != b)),
                            Lt => Some(Value::Bool(a < b)),
                            Le => Some(Value::Bool(a <= b)),
                            Gt => Some(Value::Bool(a > b)),
                            Ge => Some(Value::Bool(a >= b)),
                            _ => None,
                        };
                        if result.is_none()
                            && matches!(op, Add | Sub | Mul | Div | Mod | Pow | Shl | Shr)
                        {
                            self.arithmetic_failure = true;
                        }
                        result
                    }
                    (Value::Uint(a), Value::Uint(b)) => self.unsigned(a, b, *op, false),
                    (Value::Byte(a), Value::Byte(b)) => {
                        self.unsigned(a.into(), b.into(), *op, true)
                    }
                    (Value::Float(a), Value::Float(b)) => {
                        self.float(f64::from_bits(a), f64::from_bits(b), *op)
                    }
                    (Value::Bool(a), Value::Bool(b)) => match op {
                        BinaryOp::And => Some(Value::Bool(a && b)),
                        BinaryOp::Or => Some(Value::Bool(a || b)),
                        BinaryOp::Eq => Some(Value::Bool(a == b)),
                        BinaryOp::Ne => Some(Value::Bool(a != b)),
                        _ => None,
                    },
                    (Value::String(a), Value::String(b)) => match op {
                        BinaryOp::Concat => Some(Value::String(a + &b)),
                        BinaryOp::Eq => Some(Value::Bool(a == b)),
                        BinaryOp::Ne => Some(Value::Bool(a != b)),
                        BinaryOp::In => Some(Value::Bool(b.contains(&a))),
                        _ => None,
                    },
                    (Value::Char(a), Value::Char(b)) => match op {
                        BinaryOp::Eq => Some(Value::Bool(a == b)),
                        BinaryOp::Ne => Some(Value::Bool(a != b)),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::Call { callee, args, .. } => {
                if matches!(callee.unlocated(), Expr::Name(name) if name == "@target") {
                    return Some(Value::Tuple(vec![
                        Value::String(crate::target::OS.into()),
                        Value::String(crate::target::ARCH.into()),
                    ]));
                }
                if let Expr::Member { object, name } = callee.unlocated()
                    && let Expr::Name(owner) = object.unlocated()
                    && let Some(TypeInfo::Enum(_)) = self.checked.types.get(owner)
                {
                    return Some(Value::Enum(
                        owner.clone(),
                        name.clone(),
                        args.iter()
                            .map(|v| self.evaluate(v, env))
                            .collect::<Option<_>>()?,
                    ));
                }
                let callable = self.evaluate(callee, env)?;
                let (f, mut scope) = match &callable {
                    Value::Function(n) => (*self.functions.get(n)?, HashMap::new()),
                    Value::Closure(key, captures) => {
                        (*self.lambdas.get(key)?, captures.iter().cloned().collect())
                    }
                    _ => return None,
                };
                let mut values = vec![];
                for (p, arg) in f.params.iter().zip(args) {
                    let value = self.evaluate(arg, env)?;
                    let value = self.coerce(value, &p.ty)?;
                    values.push(value.clone());
                    scope.insert(p.name.clone(), value);
                }
                let cacheable =
                    !callable.contains_cell() && !values.iter().any(Value::contains_cell);
                let key = (callable, values);
                if cacheable && let Some(value) = self.memo.get(&key) {
                    return Some(value.clone());
                }
                match self
                    .block(&f.body, &mut scope)
                    .or_else(|| self.flow.take())?
                {
                    Flow::Next
                        if f.return_type == Type::void()
                            || f.return_type == Type::ErrorUnion(Box::new(Type::void())) =>
                    {
                        let v = self.coerce(Value::Void(vec![]), &f.return_type)?;
                        if cacheable && !v.contains_cell() {
                            self.memo.insert(key, v.clone());
                        }
                        Some(v)
                    }
                    Flow::Return(v) => {
                        let v = self.coerce(v, &f.return_type)?;
                        if cacheable && !v.contains_cell() {
                            self.memo.insert(key, v.clone());
                        }
                        Some(v)
                    }
                    Flow::Throw(message) => {
                        let Type::ErrorUnion(inner) = &f.return_type else {
                            return None;
                        };
                        let value = Value::Failure((**inner).clone(), message);
                        if cacheable {
                            self.memo.insert(key, value.clone());
                        }
                        Some(value)
                    }
                    _ => None,
                }
            }
            Expr::If { subject, arms } => {
                let flow = self.conditional(subject.as_deref(), arms, env, true)?;
                self.flow_value(flow)
            }
            Expr::Else { value, fallback } => match self.evaluate(value, env)? {
                Value::Optional(_, Some(value)) => Some(*value),
                Value::Optional(_, None) => {
                    let flow = self.value_block(fallback, env, true)?;
                    self.flow_value(flow)
                }
                _ => None,
            },
            Expr::Try(value) => match self.evaluate(value, env)? {
                Value::Success(_, value) => Some(*value),
                Value::Failure(_, message) => {
                    self.flow = Some(Flow::Throw(message));
                    None
                }
                _ => None,
            },
            Expr::Catch { value, name, body } => match self.evaluate(value, env)? {
                Value::Success(_, value) => Some(*value),
                Value::Failure(_, message) => {
                    let previous = env.insert(name.clone(), Value::String(message));
                    let flow = self.value_block(body, env, true);
                    if let Some(previous) = previous {
                        env.insert(name.clone(), previous);
                    } else {
                        env.remove(name);
                    }
                    self.flow_value(flow?)
                }
                _ => None,
            },
            _ => None,
        }
    }
    fn flow_value(&mut self, flow: Flow) -> Option<Value> {
        match flow {
            Flow::Value(value) => Some(value),
            Flow::Return(_) | Flow::Throw(_) => {
                self.flow = Some(flow);
                None
            }
            _ => None,
        }
    }
}
enum Flow {
    Next,
    Return(Value),
    Throw(String),
    Value(Value),
    Break(Option<String>),
    Continue(Option<String>),
}
enum Access {
    Field(String),
    Index(Value),
}

fn assign(value: &mut Value, path: &[Access], replacement: Value) -> Option<()> {
    let Some((first, rest)) = path.split_first() else {
        *value = replacement;
        return Some(());
    };
    match (value, first) {
        (Value::Struct(_, fields), Access::Field(name)) => assign(
            &mut fields.iter_mut().find(|(n, _)| n == name)?.1,
            rest,
            replacement,
        ),
        (Value::Array(values) | Value::Tuple(values), Access::Index(index)) => {
            let n = index_number(index)?;
            assign(values.get_mut(n)?, rest, replacement)
        }
        (Value::Map(entries), Access::Index(key)) => {
            if let Some((_, value)) = entries.iter_mut().find(|(k, _)| k.equals(key)) {
                assign(value, rest, replacement)
            } else if rest.is_empty() {
                entries.push((key.clone(), replacement));
                Some(())
            } else {
                None
            }
        }
        (Value::String(text), Access::Index(index)) if rest.is_empty() => {
            let Value::Char(replacement) = replacement else {
                return None;
            };
            let n = index_number(index)?;
            let bounds = crate::unicode::boundaries(text);
            let start = *bounds.get(n)?;
            let end = bounds.get(n + 1).copied().unwrap_or(text.len());
            text.replace_range(start..end, &replacement);
            Some(())
        }
        _ => None,
    }
}
fn index_number(value: &Value) -> Option<usize> {
    match value {
        Value::Int(n) => usize::try_from(*n).ok(),
        Value::Uint(n) => usize::try_from(*n).ok(),
        _ => None,
    }
}
fn declaration_value(pattern: &Pattern, ty: &Type, value: Value) -> Option<Value> {
    if let (Pattern::Tuple(_), Type::Tuple(types), Value::Tuple(values)) = (pattern, ty, &value)
        && types.len() != values.len()
    {
        fn regroup(ty: &Type, values: &mut impl Iterator<Item = Value>) -> Option<Value> {
            match ty {
                Type::Tuple(types) => Some(Value::Tuple(
                    types
                        .iter()
                        .map(|ty| regroup(ty, values))
                        .collect::<Option<_>>()?,
                )),
                _ => values.next(),
            }
        }
        let Value::Tuple(values) = value else {
            unreachable!()
        };
        let mut values = values.into_iter();
        let result = regroup(ty, &mut values)?;
        return values.next().is_none().then_some(result);
    }
    Some(value)
}
fn bind_declaration(
    pattern: &Pattern,
    value: Value,
    env: &mut HashMap<String, Value>,
    previous: &mut Vec<(String, Option<Value>)>,
) -> Option<()> {
    match (pattern, value) {
        (Pattern::Name(name), value) => {
            if name != "_" {
                previous.push((name.clone(), env.insert(name.clone(), value)));
            }
        }
        (Pattern::Tuple(patterns), Value::Tuple(values)) if patterns.len() == values.len() => {
            for (pattern, value) in patterns.iter().zip(values) {
                bind_declaration(pattern, value, env, previous)?;
            }
        }
        _ => return None,
    }
    Some(())
}
impl Evaluator<'_> {
    fn pattern(
        &mut self,
        pattern: &Pattern,
        value: &Value,
        env: &mut HashMap<String, Value>,
    ) -> Option<bool> {
        match (pattern, value) {
            (Pattern::Wildcard, _) => Some(true),
            (Pattern::Name(name), _) => {
                if let Some(existing) = env.get(name) {
                    let existing = match existing {
                        Value::Cell(index) => self.cells.get(*index)?,
                        existing => existing,
                    };
                    Some(existing.equals(value))
                } else {
                    env.insert(name.clone(), value.clone());
                    Some(true)
                }
            }
            (Pattern::Literal(expr), _) => Some(self.evaluate(expr, env)?.equals(value)),
            (Pattern::Tuple(patterns), Value::Tuple(values))
            | (Pattern::Array(patterns), Value::Array(values)) => {
                if patterns.len() != values.len() {
                    return Some(false);
                }
                for (pattern, value) in patterns.iter().zip(values) {
                    if !self.pattern(pattern, value, env)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            (Pattern::Struct { fields, .. }, Value::Struct(_, values)) => {
                for (name, pattern) in fields {
                    let (_, value) = values.iter().find(|(field, _)| field == name)?;
                    if !self.pattern(pattern, value, env)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            (
                Pattern::Variant {
                    name,
                    values: patterns,
                },
                Value::Enum(_, variant, values),
            ) => {
                if name.rsplit('.').next()? != variant {
                    return Some(false);
                }
                for (pattern, value) in patterns.iter().zip(values) {
                    if !self.pattern(pattern, value, env)? {
                        return Some(false);
                    }
                }
                Some(true)
            }
            _ => Some(false),
        }
    }
    fn conditional(
        &mut self,
        subject: Option<&Expr>,
        arms: &[(Vec<Pattern>, Block)],
        env: &mut HashMap<String, Value>,
        valued: bool,
    ) -> Option<Flow> {
        let subject = if let Some(subject) = subject {
            self.evaluate(subject, env)?
        } else {
            Value::Bool(true)
        };
        for (patterns, body) in arms {
            for pattern in patterns {
                let mut scope = env.clone();
                if self.pattern(pattern, &subject, &mut scope)? {
                    let result = self.value_block(body, &mut scope, valued)?;
                    for (name, value) in env.iter_mut() {
                        *value = scope.get(name)?.clone();
                    }
                    return Some(result);
                }
            }
        }
        None
    }
    fn block(&mut self, b: &Block, env: &mut HashMap<String, Value>) -> Option<Flow> {
        self.value_block(b, env, false)
    }
    fn value_block(
        &mut self,
        b: &Block,
        env: &mut HashMap<String, Value>,
        valued: bool,
    ) -> Option<Flow> {
        let mut declared = vec![];
        let mut result = Flow::Next;
        for (index, s) in b.statements.iter().enumerate() {
            *self.fuel = self.fuel.checked_sub(1)?;
            let flow = match s.unlocated() {
                Stmt::Var(v) => {
                    if v.mutex {
                        return None;
                    }
                    let value = self.evaluate(&v.value, env)?;
                    let value = self.coerce(declaration_value(&v.pattern, &v.ty, value)?, &v.ty)?;
                    bind_declaration(&v.pattern, value, env, &mut declared)?;
                    if v.mutable {
                        for name in v.binding_names() {
                            let value = env.get_mut(name)?;
                            let index = self.cells.len();
                            self.cells
                                .push(std::mem::replace(value, Value::Cell(index)));
                        }
                    }
                    Flow::Next
                }
                Stmt::Assign { target, value } => {
                    let mut path = Vec::new();
                    let name = self.place(target, env, &mut path)?;
                    let v = self.evaluate(value, env)?;
                    if name != "_" {
                        let ty = self.checked.expression_types.get(&target.id())?;
                        let v = self.coerce(v, ty)?;
                        let binding = env.get_mut(&name)?;
                        let storage = match binding {
                            Value::Cell(index) => self.cells.get_mut(*index)?,
                            value => value,
                        };
                        assign(storage, &path, v)?;
                    }
                    Flow::Next
                }
                Stmt::Return(Some(e)) => Flow::Return(self.evaluate(e, env)?),
                Stmt::Return(None) => Flow::Return(Value::Void(vec![])),
                Stmt::Throw(e) => match self.evaluate(e, env)? {
                    Value::String(message) => Flow::Throw(message),
                    _ => return None,
                },
                Stmt::Expr(value) if matches!(value.unlocated(), Expr::If { .. }) => {
                    let Expr::If { subject, arms } = value.unlocated() else {
                        unreachable!()
                    };
                    self.conditional(
                        subject.as_deref(),
                        arms,
                        env,
                        valued && index + 1 == b.statements.len(),
                    )?
                }
                Stmt::Expr(e) if valued && index + 1 == b.statements.len() => {
                    Flow::Value(self.evaluate(e, env)?)
                }
                Stmt::Expr(e) => {
                    self.evaluate(e, env)?;
                    Flow::Next
                }
                Stmt::While {
                    condition,
                    body,
                    label,
                } => loop {
                    if self.evaluate(condition, env)? != Value::Bool(true) {
                        break Flow::Next;
                    }
                    match self.block(body, env)? {
                        Flow::Break(target) if target.is_none() || &target == label => {
                            break Flow::Next;
                        }
                        Flow::Continue(target) if target.is_none() || &target == label => {}
                        Flow::Next => {}
                        flow => break flow,
                    }
                },
                Stmt::For {
                    name,
                    iterable,
                    body,
                    label,
                } => {
                    let keys = match self.evaluate(iterable, env)? {
                        Value::Array(values) => (0..values.len())
                            .map(|i| Value::Uint(i as u64))
                            .collect::<Vec<_>>(),
                        Value::Map(entries) => entries.into_iter().map(|(key, _)| key).collect(),
                        Value::String(text) => (0..crate::unicode::boundaries(&text).len())
                            .map(|i| Value::Uint(i as u64))
                            .collect(),
                        _ => return None,
                    };
                    let previous = env.remove(name);
                    let mut flow = Flow::Next;
                    for key in keys {
                        *self.fuel = self.fuel.checked_sub(1)?;
                        env.insert(name.clone(), key);
                        match self.block(body, env)? {
                            Flow::Break(target) if target.is_none() || &target == label => break,
                            Flow::Continue(target) if target.is_none() || &target == label => {}
                            Flow::Next => {}
                            result => {
                                flow = result;
                                break;
                            }
                        }
                    }
                    if let Some(previous) = previous {
                        env.insert(name.clone(), previous);
                    } else {
                        env.remove(name);
                    }
                    flow
                }
                Stmt::LabeledIf { label, value } => {
                    let Expr::If { subject, arms } = value.unlocated() else {
                        return None;
                    };
                    match self.conditional(subject.as_deref(), arms, env, false)? {
                        Flow::Break(Some(target)) if target == *label => Flow::Next,
                        flow => flow,
                    }
                }
                Stmt::Block(body) => self.block(body, env)?,
                Stmt::Break(None, label) => Flow::Break(label.clone()),
                Stmt::Break(Some(e), None) => Flow::Value(self.evaluate(e, env)?),
                Stmt::Continue(label) => Flow::Continue(label.clone()),
                _ => return None,
            };
            if !matches!(flow, Flow::Next) {
                result = flow;
                break;
            }
        }
        for (n, previous) in declared.into_iter().rev() {
            if let Some(value) = previous {
                env.insert(n, value);
            } else {
                env.remove(&n);
            }
        }
        Some(result)
    }
}
