//! Fuel-bounded, type-aware evaluation of pure expressions, functions and loops.
//! Failed/impure evaluation leaves the original program in place.
use crate::{
    ast::*,
    diagnostic::Diagnostics,
    sema::{CheckedModule, TypeInfo},
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Value {
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
    Function(String),
}
impl Value {
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
            Self::Int(n) if n < 0 => Expr::Unary {
                op: UnaryOp::Neg,
                value: Box::new(Expr::Int(n.unsigned_abs().to_string())),
            },
            Self::Int(n) => Expr::Int(n.to_string()),
            Self::Uint(n) => Expr::Int(format!("{n}u")),
            Self::Byte(n) => Expr::Cast {
                ty: Type::Named("byte".into(), vec![]),
                value: Box::new(Expr::Int(n.to_string())),
            },
            Self::Float(bits) => Expr::Float(format!("{:?}", f64::from_bits(bits))),
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
            Self::Optional(inner, Some(value)) | Self::Success(inner, value) => {
                constant_thunk(value.expr(), inner)
            }
            Self::Function(name) => Expr::Name(name),
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
                    && let Some(value) = declaration_value(v, value)
                {
                    let _ = bind_declaration(&v.pattern, value, &mut env, &mut Vec::new());
                }
            }
            Item::Statement(Stmt::Expr(e)) => {
                replacements.push((index, fold(e, &env, &functions, &checked)?))
            }
            _ => {}
        }
    }
    let mut module = checked.module;
    for (index, replacement) in replacements {
        if let Some(replacement) = replacement {
            match &mut module.items[index] {
                Item::Global(v) => v.value = replacement,
                Item::Statement(Stmt::Expr(e)) => *e = replacement,
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
fn fold(
    e: &Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, &Function>,
    checked: &CheckedModule,
) -> Result<Option<Expr>, Diagnostics> {
    // Only fold whole pure evaluations. Do not rewrite expressions inside a
    // short-circuited or potentially effectful expression independently.
    if let Some(v) = evaluate(e, env, functions, checked, &mut 100_000)? {
        return Ok(Some(materialize(v, e, checked)));
    } else if let Expr::Call {
        callee,
        args,
        generics,
    } = e
        && matches!(&**callee,Expr::Name(n) if n.starts_with('@'))
    {
        let args = args
            .iter()
            .map(|arg| {
                evaluate(arg, env, functions, checked, &mut 100_000).map(|value| {
                    value.map_or_else(|| arg.clone(), |value| materialize(value, arg, checked))
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
    let expr = value.expr();
    let ty = checked
        .expression_types
        .get(&(original as *const Expr as usize));
    // A typed constant thunk supplies context for empty arrays, optionals, and
    // nominal values even when the caller (e.g. @println) supplies no type.
    if let Some(ty) = ty
        && !matches!(ty, Type::Named(name, _) if matches!(name.as_str(), "int" | "uint" | "byte" | "float" | "str" | "char" | "bool"))
    {
        return constant_thunk(expr, ty.clone());
    }
    expr
}
fn constant_thunk(expr: Expr, ty: Type) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Lambda(Box::new(Function {
            source_path: "<constant>".into(),
            span: 0..0,
            public: false,
            name: "constant".into(),
            generics: vec![],
            params: vec![],
            return_type: ty,
            throws: false,
            body: Block {
                statements: vec![Stmt::Return(Some(expr))],
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
    let mut evaluator = Evaluator {
        functions,
        checked,
        fuel,
        memo: HashMap::new(),
        depth: 0,
        arithmetic_failure: false,
        indices: vec![],
    };
    let value = evaluator.evaluate(e, env);
    if evaluator.arithmetic_failure {
        Err(Diagnostics::one(
            "constant evaluation failed: integer overflow, division by zero, invalid shift/exponent, or non-finite float",
            0..0,
        ))
    } else {
        Ok(value)
    }
}
struct Evaluator<'a> {
    functions: &'a HashMap<String, &'a Function>,
    checked: &'a CheckedModule,
    fuel: &'a mut usize,
    memo: HashMap<(String, Vec<Value>), Value>,
    depth: usize,
    arithmetic_failure: bool,
    indices: Vec<u64>,
}
impl Evaluator<'_> {
    fn index(
        &mut self,
        index: &Expr,
        object: &Value,
        env: &HashMap<String, Value>,
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
        env: &HashMap<String, Value>,
        path: &mut Vec<Access>,
    ) -> Option<String> {
        match e {
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
        match (value, self.base_type(ty)) {
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
    fn cast(&mut self, ty: &Type, value: Value) -> Option<Value> {
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
            "str" => match value {
                Value::String(s) | Value::Char(s) => Some(Value::String(s)),
                Value::Int(n) => Some(Value::String(n.to_string())),
                Value::Uint(n) => Some(Value::String(n.to_string())),
                Value::Byte(n) => Some(Value::String(n.to_string())),
                Value::Bool(b) => Some(Value::String(b.to_string())),
                _ => None,
            },
            "char" if matches!(value, Value::Char(_)) => Some(value),
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
        if !result.is_finite() {
            self.arithmetic_failure = true;
            return None;
        }
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
            .get(&(e as *const Expr as usize))
            .map(|ty| self.base_type(ty))
    }
    fn evaluate(&mut self, e: &Expr, env: &HashMap<String, Value>) -> Option<Value> {
        if self.depth >= 512 {
            return None;
        }
        self.depth += 1;
        let result = self.expression(e, env).and_then(|value| {
            if let Some(ty) = self.expr_type(e) {
                self.coerce(value, ty)
            } else {
                Some(value)
            }
        });
        self.depth -= 1;
        result
    }
    fn expression(&mut self, e: &Expr, env: &HashMap<String, Value>) -> Option<Value> {
        let fuel = &mut *self.fuel;
        *fuel = fuel.checked_sub(1)?;
        if let Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } = e
            && let Expr::Int(n) = &**value
            && crate::sema::integer(n).ok()? == 1u64 << 63
        {
            return Some(Value::Int(i64::MIN));
        }
        match e {
            Expr::Int(s) => {
                let n = crate::sema::integer(s).ok()?;
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
                value.is_finite().then(|| Value::Float(value.to_bits()))
            }
            Expr::Bool(b) => Some(Value::Bool(*b)),
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
                if let Expr::Name(owner) = &**object
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
            Expr::Cast { ty, value } => {
                let value = self.evaluate(value, env)?;
                self.cast(ty, value)
            }
            Expr::Name(n) if n == "$" => self
                .indices
                .last()
                .and_then(|n| n.checked_sub(1))
                .map(Value::Uint),
            Expr::Name(n) => env.get(n).cloned().or_else(|| {
                self.functions
                    .contains_key(n)
                    .then(|| Value::Function(n.clone()))
            }),
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
                if let Expr::Member { object, name } = &**callee
                    && let Expr::Name(owner) = &**object
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
                let Value::Function(n) = self.evaluate(callee, env)? else {
                    return None;
                };
                let f = *self.functions.get(&n)?;
                let mut scope = HashMap::new();
                let mut values = vec![];
                for (p, arg) in f.params.iter().zip(args) {
                    let value = self.evaluate(arg, env)?;
                    let value = self.coerce(value, &p.ty)?;
                    values.push(value.clone());
                    scope.insert(p.name.clone(), value);
                }
                let key = (n.clone(), values);
                if let Some(value) = self.memo.get(&key) {
                    return Some(value.clone());
                }
                match self.block(&f.body, &mut scope)? {
                    Flow::Return(v) => {
                        let v = self.coerce(v, &f.return_type)?;
                        self.memo.insert(key, v.clone());
                        Some(v)
                    }
                    _ => None,
                }
            }
            Expr::If { subject, arms } => {
                let mut local = env.clone();
                match self.conditional(subject.as_deref(), arms, &mut local, true)? {
                    Flow::Value(value) => Some(value),
                    _ => None,
                }
            }
            Expr::Else { value, fallback } => match self.evaluate(value, env)? {
                Value::Optional(_, Some(value)) => Some(*value),
                Value::Optional(_, None) => {
                    match self.value_block(fallback, &mut env.clone(), true)? {
                        Flow::Value(value) => Some(value),
                        _ => None,
                    }
                }
                _ => None,
            },
            Expr::Try(value) | Expr::Catch { value, .. } => match self.evaluate(value, env)? {
                Value::Success(_, value) => Some(*value),
                _ => None,
            },
            _ => None,
        }
    }
}
enum Flow {
    Next,
    Return(Value),
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
fn declaration_value(v: &VarDecl, value: Value) -> Option<Value> {
    if let (Pattern::Tuple(_), Type::Tuple(types), Value::Tuple(values)) =
        (&v.pattern, &v.ty, &value)
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
        let result = regroup(&v.ty, &mut values)?;
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
            let flow = match s {
                Stmt::Var(v) => {
                    if v.mutex {
                        return None;
                    }
                    let value = self.evaluate(&v.value, env)?;
                    let value = self.coerce(declaration_value(v, value)?, &v.ty)?;
                    bind_declaration(&v.pattern, value, env, &mut declared)?;
                    Flow::Next
                }
                Stmt::Assign { target, value } => {
                    let mut path = Vec::new();
                    let name = self.place(target, env, &mut path)?;
                    let v = self.evaluate(value, env)?;
                    if name != "_" {
                        let ty = self
                            .checked
                            .expression_types
                            .get(&(target as *const Expr as usize))?;
                        let v = self.coerce(v, ty)?;
                        assign(env.get_mut(&name)?, &path, v)?;
                    }
                    Flow::Next
                }
                Stmt::Return(Some(e)) => Flow::Return(self.evaluate(e, env)?),
                Stmt::Expr(Expr::If { subject, arms }) => self.conditional(
                    subject.as_deref(),
                    arms,
                    env,
                    valued && index + 1 == b.statements.len(),
                )?,
                Stmt::Expr(e) if valued && index + 1 == b.statements.len() => {
                    Flow::Value(self.evaluate(e, env)?)
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
                Stmt::LabeledIf {
                    label,
                    value: Expr::If { subject, arms },
                } => match self.conditional(subject.as_deref(), arms, env, false)? {
                    Flow::Break(Some(target)) if target == *label => Flow::Next,
                    flow => flow,
                },
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
