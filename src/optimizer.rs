//! Proof-gated, type-aware evaluation of pure expressions, functions and loops.
//! Failed/impure evaluation leaves the original program in place.
use crate::{
    ast::{
        BinaryOp, Block, Expr, Function, Item, Module, Pattern, SourceLocation, Stmt, Type,
        UnaryOp, VarDecl,
    },
    diagnostic::Diagnostics,
    sema::{CheckedModule, TypeInfo},
};
use std::collections::{HashMap, HashSet};

#[path = "optimizer_loops.rs"]
mod loops;

#[path = "optimizer_recursion.rs"]
mod recursion;

/// Embedding is mandatory compile-time evaluation, including in debug builds.
///
/// # Errors
/// Returns diagnostics for unevaluatable paths, unreadable files, invalid embedded
/// values, or a failure to start or complete the evaluator thread.
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
        let value = evaluate(e, &HashMap::new(), &functions, &checked)
            .map_err(|error| error.at_source(source_path, span.clone()))?;
        let Some(Value::Array(bytes)) = value else {
            return Err(Diagnostics::one(
                "@embed path must be a compile-time string; runtime values, side effects, or unproven termination prevent evaluating this path", span.clone()
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
    String(Vec<String>),
    Char(String),
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Struct(String, Vec<(String, Value)>),
    Enum(String, String, Vec<Value>),
    Optional(Type, Option<Box<Value>>),
    Success(Type, Box<Value>),
    Failure(Type, Vec<String>),
    Function(String),
    Closure(usize, Vec<(String, Value)>),
    Cell(usize),
}
// Only literal text is segmented; subsequent operations preserve char-array elements.
fn string_parts(text: &str) -> Vec<String> {
    let starts = crate::unicode::boundaries(text);
    starts
        .iter()
        .enumerate()
        .map(|(i, start)| text[*start..starts.get(i + 1).copied().unwrap_or(text.len())].to_owned())
        .collect()
}

fn string_expr(mut parts: Vec<String>) -> Expr {
    fn balanced(parts: &mut [String]) -> Expr {
        if parts.len() == 1 {
            return Expr::String(std::mem::take(&mut parts[0]));
        }
        let middle = parts.len() / 2;
        let (left, right) = parts.split_at_mut(middle);
        Expr::Binary {
            left: Box::new(balanced(left)),
            op: BinaryOp::Concat,
            right: Box::new(balanced(right)),
        }
    }
    let text = parts.concat();
    if string_parts(&text) == parts {
        Expr::String(text)
    } else {
        balanced(&mut parts)
    }
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
    let precision = usize::try_from(16 - exponent).ok()?;
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
            Self::String(parts) | Self::Failure(_, parts) => parts
                .iter()
                .all(|part| crate::unicode::boundaries(part).len() == 1),
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
            // NC equality is exact IEEE-754 equality, including NaN and signed zero.
            #[allow(clippy::float_cmp)]
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
            Self::String(s) => string_expr(s),
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
                Stmt::Throw(string_expr(message)),
                Type::ErrorUnion(Box::new(inner)),
            ),
            Self::Closure(..) | Self::Cell(_) => {
                unreachable!("closure constants retain their original source")
            }
        }
    }
}
/// Fold safely computable expressions while preserving runtime effects.
///
/// # Errors
/// Returns diagnostics for reached arithmetic failures, embedding failures, or
/// a failure to start or complete the evaluator thread.
pub fn optimize(checked: CheckedModule) -> Result<Module, Diagnostics> {
    // Certified recursion uses heap continuations. Ordinary syntax traversal
    // still needs a consistent stack instead of a small test-runner thread stack.
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
    if let Some(items) = precompute_output(&checked, &functions) {
        return Ok(Module { items });
    }
    evaluate_top_level(&checked, &functions)?;
    let mut env = HashMap::new();
    let mut replacements = Vec::new();
    let mut reaches_next = true;
    for (index, item) in checked.module.items.iter().enumerate() {
        if !reaches_next {
            continue;
        }
        match item {
            Item::Global(v) => {
                reaches_next =
                    crate::flow::expression_reaches_next(&v.value, &checked.expression_types);
                replacements.push((index, fold(&v.value, &env, &functions, &checked)?));
                let constant = if !v.mutable && !v.mutex {
                    evaluate(&v.value, &env, &functions, &checked)?
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
                reaches_next =
                    crate::flow::statement_reaches_next(statement, &checked.expression_types);
                if let Stmt::Expr(e) = statement.unlocated() {
                    replacements.push((index, fold(e, &env, &functions, &checked)?));
                }
            }
            Item::Test { body, .. } => {
                reaches_next = crate::flow::block_reaches_next(body, &checked.expression_types);
            }
            _ => {}
        }
    }
    let prefix = precompute_prefix(&checked, &functions);
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
    for (index, value) in prefix {
        match &mut module.items[index] {
            Item::Global(v) => v.value = value.expect("precomputed global has a value"),
            Item::Statement(statement) => {
                *statement.unlocated_mut() = Stmt::Block(Block { statements: vec![] });
            }
            _ => unreachable!(),
        }
    }
    prune_unreachable_functions(&mut module);
    Ok(module)
}
fn prune_unreachable_functions(module: &mut Module) {
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
}

// Unlike prefix evaluation, this transaction discards all storage only after
// the entire program is known. Recorded output is never published on failure.
fn precompute_output(
    checked: &CheckedModule,
    functions: &HashMap<String, &Function>,
) -> Option<Vec<Item>> {
    if checked
        .module
        .items
        .iter()
        .any(|item| matches!(item, Item::Test { .. }))
    {
        return None;
    }
    let mut evaluator = Evaluator::new(functions, checked);
    // Reuse lexical global storage and repeat every call's effects, rather than
    // evaluating named functions against caller-local bindings or memoized state.
    evaluator.analyse_output = true;
    evaluator.recorded_output = Some(vec![]);
    let mut env = HashMap::new();
    for item in &checked.module.items {
        evaluator.analysis_globals.clone_from(&env);
        let flow = match item {
            Item::Global(v) => evaluator
                .declaration(v, &mut env, &mut Vec::new())
                .map(|()| Flow::Next),
            Item::Statement(statement) => {
                evaluator.statements(std::slice::from_ref(statement), &mut env, false)
            }
            _ => continue,
        };
        if !matches!(flow, Some(Flow::Next)) || evaluator.failure().is_some() {
            return None;
        }
    }
    evaluator.recorded_output
}

fn precomputed_print(
    evaluator: &mut Evaluator<'_>,
    callee: &Expr,
    args: &[Expr],
    env: &mut HashMap<String, Value>,
) -> Option<Value> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(evaluator.evaluate(arg, env)?);
    }
    if evaluator.recorded_output.is_some() {
        let mut bytes = String::new();
        for (arg, value) in args.iter().zip(&values) {
            // Preserve argument snapshots and active payload types. Only output
            // flattens character boundaries; ordinary strings do not.
            bytes.push_str(&evaluator.string(value, evaluator.expr_type(arg)?)?.concat());
        }
        let mut output = Expr::Call {
            callee: Box::new(callee.clone()),
            args: vec![Expr::String(bytes)],
            generics: vec![],
        };
        if let Some(location) = callee.location() {
            output = output.located(location.clone());
        }
        evaluator
            .recorded_output
            .as_mut()?
            .push(Item::Statement(Stmt::Expr(output)));
    }
    Some(Value::Void(vec![]))
}

// Only the initial, call-free execution region can move into initializers:
// nothing has observed these globals yet. Calls and closure creation are barriers
// even if individually pure, since they can observe or export shared storage.
fn prefix_items(checked: &CheckedModule) -> Vec<usize> {
    let mut names = HashSet::new();
    let mut indices = Vec::new();
    for (index, item) in checked.module.items.iter().enumerate() {
        match item {
            Item::Global(v) => {
                let Pattern::Name(name) = &v.pattern else {
                    break;
                };
                if v.mutex || name == "_" || !names.insert(name.clone()) {
                    break;
                }
            }
            Item::Statement(_) => {}
            Item::Test { .. } => break,
            _ => continue,
        }
        let mut barrier = false;
        crate::visit::item(item, &mut |e| {
            barrier |= matches!(e, Expr::Call { .. } | Expr::Lambda(_) | Expr::Async(_));
        });
        if barrier {
            break;
        }
        indices.push(index);
    }
    indices
}

fn precompute_prefix(
    checked: &CheckedModule,
    functions: &HashMap<String, &Function>,
) -> Vec<(usize, Option<Expr>)> {
    let indices = prefix_items(checked);
    if !indices
        .iter()
        .any(|&index| matches!(checked.module.items[index], Item::Statement(_)))
    {
        return vec![];
    }
    let mut evaluator = Evaluator::new(functions, checked);
    let mut env = HashMap::new();
    for &index in &indices {
        let flow = match &checked.module.items[index] {
            Item::Global(v) => evaluator
                .declaration(v, &mut env, &mut Vec::new())
                .map(|()| Flow::Next),
            Item::Statement(statement) => {
                evaluator.statements(std::slice::from_ref(statement), &mut env, false)
            }
            _ => unreachable!(),
        };
        // Evaluation is transactional: partial writes and failures must never become runtime initializers.
        if !matches!(flow, Some(Flow::Next)) {
            return vec![];
        }
    }
    indices
        .into_iter()
        .map(|index| {
            let value = match &checked.module.items[index] {
                Item::Global(v) => {
                    let value = env.get(v.binding_names()[0])?;
                    let value = match value {
                        Value::Cell(cell) => evaluator.cells.get(*cell)?,
                        value => value,
                    };
                    if !value.materializable() {
                        return None;
                    }
                    let mut initializer = v.value.clone();
                    *initializer.unlocated_mut() = materialize(value.clone(), &v.value, checked);
                    Some(initializer)
                }
                _ => None,
            };
            Some((index, value))
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

// Analyse known top-level execution without replacing persistent state or
// executing output effects. Unknown execution keeps runtime code.
fn evaluate_top_level(
    checked: &CheckedModule,
    functions: &HashMap<String, &Function>,
) -> Result<(), Diagnostics> {
    let mut env = HashMap::new();
    let mut evaluator = Evaluator::new(functions, checked);
    evaluator.analyse_output = true;
    for item in &checked.module.items {
        if !matches!(
            item,
            Item::Global(_) | Item::Statement(_) | Item::Test { .. }
        ) {
            continue;
        }
        evaluator.memo.clear();
        evaluator.analysis_globals.clone_from(&env);
        let flow = match item {
            Item::Global(v) => evaluator
                .declaration(v, &mut env, &mut Vec::new())
                .map(|()| Flow::Next),
            Item::Statement(statement) => {
                evaluator.statements(std::slice::from_ref(statement), &mut env, false)
            }
            Item::Test { body, .. } => evaluator.block(body, &mut env),
            _ => Some(Flow::Next),
        };
        if let Some(error) = evaluator.failure() {
            return Err(error);
        }
        if !matches!(flow, Some(Flow::Next)) {
            break;
        }
    }
    Ok(())
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
        crate::visit::item(item, &mut |e| {
            if preserved.contains(&e.id())
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
            if let Ok(Some(value)) = evaluate(e, &HashMap::new(), functions, checked)
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
    if let Some(v) = evaluate(e, env, functions, checked)?.filter(Value::materializable) {
        return Ok(Some(materialize(v, e, checked)));
    } else if let Expr::Call {
        callee,
        args,
        generics,
    } = e.unlocated()
        && matches!(callee.unlocated(),Expr::Name(n) if n.starts_with('@'))
    {
        let mut reached = true;
        let mut folded_args = Vec::new();
        for arg in args {
            let value = match evaluate(arg, env, functions, checked) {
                Ok(value) => value,
                Err(error) if reached => return Err(error),
                Err(_) => None,
            };
            // An unknown earlier argument may fail or never return. Later
            // successes can fold, but their failures are not necessarily reached.
            reached &= value.is_some();
            folded_args.push(
                value
                    .filter(Value::materializable)
                    .map_or_else(|| arg.clone(), |value| materialize(value, arg, checked)),
            );
        }
        let args = folded_args;
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
) -> Result<Option<Value>, Diagnostics> {
    let mut evaluator = Evaluator::new(functions, checked);
    let value = evaluator.evaluate(e, &mut env.clone());
    if let Some(error) = evaluator.failure() {
        Err(error)
    } else {
        // Cell identities are private to this evaluation, never reusable constants.
        Ok(value.filter(|value| !value.contains_cell()))
    }
}
struct Evaluator<'a> {
    functions: &'a HashMap<String, &'a Function>,
    checked: &'a CheckedModule,
    lambdas: HashMap<usize, &'a Function>,
    expressions: HashMap<usize, &'a Expr>,
    embed_error: Option<Diagnostics>,
    memo: HashMap<(Value, Vec<Value>), Value>,
    cells: Vec<Value>,

    analyse_output: bool,
    recorded_output: Option<Vec<Item>>,
    analysis_globals: HashMap<String, Value>,
    arithmetic_failure: bool,
    arithmetic_location: Option<SourceLocation>,
    // Value expressions can exit enclosing statements through returns, throws, or jumps.
    flow: Option<Flow>,
    indices: Vec<u64>,
}
impl<'module> Evaluator<'module> {
    fn new(
        functions: &'module HashMap<String, &'module Function>,
        checked: &'module CheckedModule,
    ) -> Self {
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
        Self {
            functions,
            checked,
            lambdas,
            expressions,
            embed_error: None,
            memo: HashMap::new(),
            cells: Vec::new(),

            analyse_output: false,
            recorded_output: None,
            analysis_globals: HashMap::new(),
            arithmetic_failure: false,
            arithmetic_location: None,
            flow: None,
            indices: vec![],
        }
    }
    fn failure(&mut self) -> Option<Diagnostics> {
        if let Some(error) = self.embed_error.take() {
            return Some(error);
        }
        if !self.arithmetic_failure {
            return None;
        }
        let error = Diagnostics::one(
            "constant evaluation failed: integer overflow, division by zero, invalid shift/exponent, or numeric cast out of range",
            0..0,
        );
        Some(if let Some(location) = self.arithmetic_location.take() {
            error.at_source(&location.path, location.span)
        } else {
            error
        })
    }
    fn declaration(
        &mut self,
        v: &VarDecl,
        env: &mut HashMap<String, Value>,
        declared: &mut Vec<(String, Option<Value>)>,
    ) -> Option<()> {
        if v.mutex {
            return None;
        }
        let value = self.evaluate(&v.value, env)?;
        let value = self.coerce(declaration_value(&v.pattern, &v.ty, value)?, &v.ty)?;
        bind_declaration(&v.pattern, value, env, declared)?;
        if v.mutable {
            for name in v.binding_names() {
                let value = env.get_mut(name)?;
                let index = self.cells.len();
                self.cells
                    .push(std::mem::replace(value, Value::Cell(index)));
            }
        }
        Some(())
    }
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
            Value::String(text) => text.len(),
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
                let index = if crate::visit::uses_index_length(index) {
                    let object = self.read_place(&root, env, path)?.clone();
                    self.index(index, &object, env)?
                } else {
                    self.evaluate(index, env)?
                };
                path.push(Access::Index(index));
                Some(root)
            }
            _ => None,
        }
    }
    // Resolve storage using previously evaluated keys/indices, never by
    // executing the assignment's parent expression a second time.
    fn read_place<'b>(
        &'b self,
        root: &str,
        env: &'b HashMap<String, Value>,
        path: &[Access],
    ) -> Option<&'b Value> {
        let storage = match env.get(root)? {
            Value::Cell(index) => self.cells.get(*index)?,
            value => value,
        };
        place_value(storage, path)
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
    fn string(&self, value: &Value, ty: &Type) -> Option<Vec<String>> {
        let ty = self.base_type(ty);
        let sequence = |values: &[Value], types: &[Type], quoted: bool| {
            let mut result = Vec::new();
            for (i, (value, ty)) in values.iter().zip(types).enumerate() {
                if i != 0 {
                    result.extend(string_parts(", "));
                }
                let quote = quoted && *ty == Type::Named("str".into(), vec![]);
                if quote {
                    result.extend(string_parts("\""));
                }
                result.extend(self.string(value, ty)?);
                if quote {
                    result.extend(string_parts("\""));
                }
            }
            Some(result)
        };
        let wrap = |prefix: &str, mut parts: Vec<String>, suffix: &str| {
            let mut result = string_parts(prefix);
            result.append(&mut parts);
            result.extend(string_parts(suffix));
            result
        };
        Some(match (value, ty) {
            (Value::String(s), _) => s.clone(),
            (Value::Char(s), _) => vec![s.clone()],
            (Value::Int(n), _) => string_parts(&n.to_string()),
            (Value::Uint(n), _) => string_parts(&n.to_string()),
            (Value::Byte(n), _) => string_parts(&n.to_string()),
            (Value::Float(bits), _) => string_parts(&float_string(f64::from_bits(*bits))?),
            (Value::Bool(b), _) => string_parts(&b.to_string()),
            (Value::Optional(_, None), _) => string_parts("none"),
            (Value::Failure(_, message), _) => wrap("error: ", message.clone(), ""),
            (Value::Success(inner, value), _) => self.string(value, inner)?,
            (Value::Optional(_, Some(value)), Type::Optional(inner)) => {
                self.string(value, inner)?
            }
            (Value::Array(values), Type::Array(inner, _)) => {
                let types = vec![(**inner).clone(); values.len()];
                wrap("[", sequence(values, &types, true)?, "]")
            }
            (Value::Tuple(values), Type::Tuple(types)) => {
                wrap("(", sequence(values, types, false)?, ")")
            }
            (Value::Map(entries), Type::Map(key, val)) => {
                let mut parts = Vec::new();
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i != 0 {
                        parts.extend(string_parts(", "));
                    }
                    parts.extend(self.string(k, key)?);
                    parts.extend(string_parts(": "));
                    parts.extend(self.string(v, val)?);
                }
                wrap("[", parts, "]")
            }
            (Value::Struct(name, values), _) => {
                let TypeInfo::Struct(declaration) = self.checked.types.get(name)? else {
                    return None;
                };
                let mut parts = Vec::new();
                for (i, field) in declaration.fields.iter().enumerate() {
                    if i != 0 {
                        parts.extend(string_parts(", "));
                    }
                    let (_, value) = values.iter().find(|(name, _)| *name == field.name)?;
                    parts.extend(string_parts(&format!(".{} = ", field.name)));
                    parts.extend(self.string(value, &field.ty)?);
                }
                wrap(&format!("{name}{{"), parts, "}")
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
                    string_parts(&format!("{name}.{variant}"))
                } else {
                    wrap(
                        &format!("{name}.{variant}("),
                        sequence(values, types, true)?,
                        ")",
                    )
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
                    Value::String(s) => s.concat().into_bytes(),
                    Value::Char(s) => s.into_bytes(),
                    Value::Array(values) => return Some(Value::Array(values)),
                    _ => return None,
                };
                return Some(Value::Array(bytes.into_iter().map(Value::Byte).collect()));
            }
            if **inner == Type::Named("char".into(), vec![])
                && let Value::String(s) = value
            {
                return Some(Value::Array(s.into_iter().map(Value::Char).collect()));
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
            Value::Int(n) => Some(i128::from(*n)),
            Value::Uint(n) => Some(i128::from(*n)),
            Value::Byte(n) => Some(i128::from(*n)),
            Value::Bool(b) => Some(i128::from(*b)),
            Value::Float(bits) => {
                let n = f64::from_bits(*bits);
                let fits = match name.as_str() {
                    "byte" => (0.0..=255.0).contains(&n),
                    "uint" => (0.0..18_446_744_073_709_551_616.0).contains(&n),
                    _ => (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&n),
                };
                (n.is_finite() && fits).then(|| {
                    // NC truncates toward zero; the finite/range checks guard this cast.
                    #[allow(clippy::cast_possible_truncation)]
                    let integer = n.trunc() as i128;
                    integer
                })
            }
            _ => None,
        };
        let result = match name.as_str() {
            "int" => integer.and_then(|n| i64::try_from(n).ok()).map(Value::Int),
            "uint" => integer.and_then(|n| u64::try_from(n).ok()).map(Value::Uint),
            "byte" => integer.and_then(|n| u8::try_from(n).ok()).map(Value::Byte),
            "float" => match value {
                Value::Float(_) => Some(value),
                // NC integer-to-float conversion intentionally rounds to IEEE-754 precision.
                #[allow(clippy::cast_precision_loss)]
                Value::Int(n) => Some(Value::Float((n as f64).to_bits())),
                // NC integer-to-float conversion intentionally rounds to IEEE-754 precision.
                #[allow(clippy::cast_precision_loss)]
                Value::Uint(n) => Some(Value::Float((n as f64).to_bits())),
                Value::Byte(n) => Some(Value::Float(f64::from(n).to_bits())),
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
        use BinaryOp::{
            Add, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mod, Mul, Ne, Pow, Shl, Shr, Sub,
        };
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
                .and_then(|b| u64::try_from(u128::from(a) << b).ok()),
            Shr => u32::try_from(b)
                .ok()
                .filter(|b| *b < if byte { 8 } else { 64 })
                .and_then(|b| a.checked_shr(b)),
            _ => return None,
        }
        .filter(|n| !byte || u8::try_from(*n).is_ok());
        if result.is_none() {
            self.arithmetic_failure = true;
        }
        result.and_then(|n| {
            if byte {
                u8::try_from(n).ok().map(Value::Byte)
            } else {
                Some(Value::Uint(n))
            }
        })
    }
    fn float(a: f64, b: f64, op: BinaryOp) -> Option<Value> {
        use BinaryOp::{Add, Div, Eq, Ge, Gt, Le, Lt, Mod, Mul, Ne, Pow, Sub};
        let comparison = match op {
            // NC equality is exact IEEE-754 equality, not an approximate comparison.
            #[allow(clippy::float_cmp)]
            Eq => Some(a == b),
            // IEEE-754 inequality must also preserve NaN and signed-zero behavior.
            #[allow(clippy::float_cmp)]
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
        let result = self.expression(e, env).and_then(|value| {
            if let Some(ty) = self.checked.expression_types.get(&e.id()) {
                self.coerce(value, ty)
            } else {
                Some(value)
            }
        });

        if self.arithmetic_failure && self.arithmetic_location.is_none() {
            self.arithmetic_location = e.location().cloned();
        }
        result
    }
    fn expression(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        let e = e.unlocated();
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
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Bytes(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::None => self.literal_value(e),
            Expr::Name(n) if n == "$" => self.literal_value(e),
            Expr::Lambda(_) => self.lambda_value(e, env),
            Expr::Embed { .. } => self.embed_value(e, env),
            Expr::Array(_) | Expr::Tuple(_) | Expr::Map(_) | Expr::StructInit { .. } => {
                self.container_value(e, env)
            }
            Expr::Member { object, name } => self.member_value(object, name, env),
            Expr::Index { object, index } => self.index_value(object, index, env),
            Expr::Cast { ty, value, .. } => self.cast_value(ty, value, env),
            Expr::Name(n) => self.name_value(e, n, env),
            Expr::Unary { op, value } => self.unary_value(*op, value, env),
            Expr::Binary { left, op, right } => self.binary_value(left, *op, right, env),
            Expr::Call { callee, args, .. } => self.call_value(callee, args, env),
            Expr::If { .. } | Expr::Else { .. } | Expr::Try(_) | Expr::Catch { .. } => {
                self.branch_value(e, env)
            }
            _ => None,
        }
    }
    fn literal_value(&self, e: &Expr) -> Option<Value> {
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
            Expr::Bytes(bytes) => Some(Value::Array(
                bytes.iter().copied().map(Value::Byte).collect(),
            )),
            Expr::String(s) => Some(Value::String(string_parts(s))),
            Expr::Char(s) => Some(Value::Char(s.clone())),
            Expr::None => {
                let Type::Optional(inner) = self.expr_type(e)? else {
                    return None;
                };
                Some(Value::Optional((**inner).clone(), None))
            }
            Expr::Name(n) if n == "$" => self
                .indices
                .last()
                .and_then(|n| n.checked_sub(1))
                .map(Value::Uint),
            _ => None,
        }
    }
    fn container_value(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        match e {
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
            _ => None,
        }
    }
    fn branch_value(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        match e {
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
    fn lambda_value(&mut self, e: &Expr, env: &HashMap<String, Value>) -> Option<Value> {
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
                        value @ Value::Cell(_) => Some((capture.name.clone(), value.clone())),
                        _ => None,
                    };
                }
                let value = env
                    .get(&capture.name)
                    .cloned()
                    .or_else(|| self.constant_binding(capture.initializer?, &capture.name))?;
                Some((capture.name.clone(), value))
            })
            .collect::<Option<_>>()?;
        Some(Value::Closure(key, values))
    }
    fn embed_value(&mut self, e: &Expr, env: &mut HashMap<String, Value>) -> Option<Value> {
        let Expr::Embed {
            path,
            source_path,
            span,
        } = e
        else {
            return None;
        };

        let Value::String(path) = self.evaluate(path, env)? else {
            return None;
        };
        match embedded_bytes(&path.concat(), source_path) {
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
    fn member_value(
        &mut self,
        object: &Expr,
        name: &str,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        if let Expr::Name(owner) = object.unlocated()
            && let Some(TypeInfo::Enum(_)) = self.checked.types.get(owner)
        {
            return Some(Value::Enum(owner.clone(), name.to_owned(), vec![]));
        }
        match self.evaluate(object, env)? {
            Value::Struct(_, fields) => fields
                .into_iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value),
            Value::Array(values) if name == "len" => Some(Value::Uint(values.len() as u64)),
            Value::Map(values) if name == "len" => Some(Value::Uint(values.len() as u64)),
            Value::String(value) if name == "len" => Some(Value::Uint(value.len() as u64)),
            _ => None,
        }
    }
    fn index_value(
        &mut self,
        object: &Expr,
        index: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
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
            Value::String(value) => value.get(n).cloned().map(Value::Char),
            _ => None,
        }
    }
    fn cast_value(
        &mut self,
        ty: &Type,
        value: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        let from = self.expr_type(value)?.clone();
        let value = self.evaluate(value, env)?;
        if self.base_type(ty) == self.base_type(&from) {
            self.coerce(value, ty)
        } else {
            self.cast(ty, &from, value)
        }
    }
    fn name_value(&mut self, e: &Expr, n: &str, env: &HashMap<String, Value>) -> Option<Value> {
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
            .then(|| Value::Function(n.to_owned()))
    }
    fn call_value(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        if self.analyse_output
            && matches!(callee.unlocated(), Expr::Name(name) if name == "@print" || name == "@println")
        {
            return precomputed_print(self, callee, args, env);
        }
        if matches!(callee.unlocated(), Expr::Name(name) if name == "@target") {
            return Some(Value::Tuple(vec![
                Value::String(string_parts(crate::target::OS)),
                Value::String(string_parts(crate::target::ARCH)),
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
            Value::Function(n) => (
                *self.functions.get(n)?,
                if self.analyse_output {
                    self.analysis_globals.clone()
                } else {
                    HashMap::new()
                },
            ),
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
        // Resolve callable arguments without executing any candidate body. Unknown
        // edges and uncertified cycles must be rejected before entry, not by fuel.
        let ranks = self.recursion_graph_proven(f, &scope)?;
        if let Some(rank) = ranks.get(&Self::recursion_identity(f)) {
            return self.recursion_heap(f, &callable, &values, &scope, *rank, &ranks);
        }
        // Sequential analysis observes global mutations and must repeat call effects.
        let cacheable = !self.analyse_output
            && !callable.contains_cell()
            && !values.iter().any(Value::contains_cell);
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
    fn recursion_identity(function: &Function) -> usize {
        std::ptr::from_ref(function) as usize
    }

    /// Abstract values are sets of callable identities, flattened through
    /// containers. Merging alternatives is conservative; no candidate is run.
    fn recursion_targets(&self, value: &Value) -> HashSet<usize> {
        let mut targets = HashSet::new();
        let mut pending = vec![value];
        let mut cells = HashSet::new();
        while let Some(value) = pending.pop() {
            match value {
                Value::Function(name) => {
                    targets.insert(Self::recursion_identity(self.functions[name]));
                }
                Value::Closure(key, _) => {
                    targets.insert(Self::recursion_identity(self.lambdas[key]));
                }
                Value::Cell(index) if cells.insert(*index) => {
                    pending.push(&self.cells[*index]);
                }
                Value::Array(values) | Value::Tuple(values) | Value::Enum(_, _, values) => {
                    pending.extend(values);
                }
                Value::Struct(_, fields) => {
                    pending.extend(fields.iter().map(|(_, value)| value));
                }
                Value::Map(entries) => {
                    pending.extend(entries.iter().flat_map(|(key, value)| [key, value]));
                }

                Value::Optional(_, Some(value)) | Value::Success(_, value) => pending.push(value),
                _ => {}
            }
        }
        targets
    }

    fn recursion_merge(
        scope: &mut HashMap<String, HashSet<usize>>,
        name: &str,
        targets: &HashSet<usize>,
    ) -> bool {
        let entry = scope.entry(name.to_owned()).or_default();
        let before = entry.len();
        entry.extend(targets);
        entry.len() != before
    }

    fn recursion_seed(
        &self,
        values: &HashMap<String, Value>,
        scopes: &mut HashMap<usize, HashMap<String, HashSet<usize>>>,
    ) {
        let mut pending: Vec<_> = values.values().collect();
        let mut cells = HashSet::new();
        while let Some(value) = pending.pop() {
            match value {
                Value::Closure(key, captures) => {
                    let scope = scopes
                        .entry(Self::recursion_identity(self.lambdas[key]))
                        .or_default();
                    for (name, value) in captures {
                        Self::recursion_merge(scope, name, &self.recursion_targets(value));
                        pending.push(value);
                    }
                }
                Value::Cell(index) if cells.insert(*index) => pending.push(&self.cells[*index]),
                Value::Array(values) | Value::Tuple(values) | Value::Enum(_, _, values) => {
                    pending.extend(values);
                }
                Value::Struct(_, fields) => pending.extend(fields.iter().map(|(_, value)| value)),
                Value::Map(entries) => {
                    pending.extend(entries.iter().flat_map(|(key, value)| [key, value]));
                }
                Value::Optional(_, Some(value)) | Value::Success(_, value) => pending.push(value),
                _ => {}
            }
        }
    }

    /// Collect only this callable's statements, not deferred lambda bodies.
    fn recursion_statements(function: &Function) -> Vec<&Stmt> {
        Self::recursion_block_statements(&function.body)
    }

    fn recursion_block_statements(body: &Block) -> Vec<&Stmt> {
        let mut pending = vec![body];
        let mut visited = HashSet::new();
        let mut statements = Vec::new();
        while let Some(block) = pending.pop() {
            if !visited.insert(std::ptr::from_ref(block)) {
                continue;
            }
            for statement in &block.statements {
                statements.push(statement.unlocated());
                match statement.unlocated() {
                    Stmt::Block(body)
                    | Stmt::For { body, .. }
                    | Stmt::While { body, .. }
                    | Stmt::Lock { body, .. } => pending.push(body),
                    _ => {}
                }
            }
            let mut deferred = HashSet::new();
            crate::visit::block(block, &mut |expression| {
                if deferred.contains(&expression.id()) {
                    return;
                }
                match expression {
                    Expr::Lambda(function) => crate::visit::block(&function.body, &mut |value| {
                        deferred.insert(value.id());
                    }),
                    Expr::If { arms, .. } => pending.extend(arms.iter().map(|(_, body)| body)),
                    Expr::Else { fallback, .. } => pending.push(fallback),
                    Expr::Catch { body, .. } => pending.push(body),
                    _ => {}
                }
            });
        }
        statements
    }

    fn recursion_expressions<'a>(&'a self, function: &'a Function) -> Vec<&'a Expr> {
        let mut expressions = Vec::new();
        let mut deferred = HashSet::new();
        crate::visit::block(&function.body, &mut |expression| {
            if deferred.contains(&expression.id()) {
                return;
            }
            expressions.push(expression);
            if let Expr::Lambda(function) = expression {
                crate::visit::block(&function.body, &mut |value| {
                    deferred.insert(value.id());
                });
            }
        });
        let mut pending = expressions;
        let mut expressions = Vec::new();
        let mut seen = HashSet::new();
        while let Some(expression) = pending.pop() {
            if !seen.insert(expression.id()) {
                continue;
            }
            expressions.push(expression.unlocated());
            if let Expr::Name(_) = expression.unlocated()
                && let Some(Some(key)) = self.checked.constant_sources.get(&expression.id())
                && let Some(initializer) = self.expressions.get(key)
            {
                pending.push(initializer);
            }
            Self::recursion_children(expression, &mut pending);
        }
        expressions
    }

    fn recursion_children<'a>(expression: &'a Expr, pending: &mut Vec<&'a Expr>) {
        let mut blocks = Vec::new();
        match expression.unlocated() {
            Expr::Embed { path, .. } => pending.push(path),
            Expr::Cast { value, .. }
            | Expr::Unary { value, .. }
            | Expr::Try(value)
            | Expr::Async(value)
            | Expr::Await(value) => pending.push(value),
            Expr::Binary { left, right, .. } => pending.extend([&**left, &**right]),
            Expr::Call { callee, args, .. } => {
                pending.push(callee);
                pending.extend(args);
            }
            Expr::Index { object, index } => pending.extend([&**object, &**index]),
            Expr::Member { object, .. } => pending.push(object),
            Expr::Array(values) | Expr::Tuple(values) => pending.extend(values),
            Expr::Map(entries) => {
                pending.extend(entries.iter().flat_map(|(key, value)| [key, value]));
            }
            Expr::StructInit { fields, .. } => {
                pending.extend(fields.iter().map(|(_, value)| value));
            }
            Expr::If { subject, arms } => {
                pending.extend(subject.as_deref());
                blocks.extend(arms.iter().map(|(_, body)| body));
            }
            Expr::Else { value, fallback } => {
                pending.push(value);
                blocks.push(fallback);
            }
            Expr::Catch { value, body, .. } => {
                pending.push(value);
                blocks.push(body);
            }
            _ => {}
        }
        for block in blocks {
            let mut deferred = HashSet::new();
            crate::visit::block(block, &mut |value| {
                if deferred.contains(&value.id()) {
                    return;
                }
                pending.push(value);
                if let Expr::Lambda(function) = value {
                    crate::visit::block(&function.body, &mut |value| {
                        deferred.insert(value.id());
                    });
                }
            });
        }
    }

    fn recursion_abstract(
        &self,
        expression: &Expr,
        scope: &HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
        visiting: &mut HashSet<usize>,
    ) -> HashSet<usize> {
        if !visiting.insert(expression.id()) {
            return HashSet::from([usize::MAX]);
        }
        let result = self.recursion_abstract_inner(expression, scope, returns, visiting);
        visiting.remove(&expression.id());
        result
    }

    fn recursion_abstract_inner(
        &self,
        expression: &Expr,
        scope: &HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
        visiting: &mut HashSet<usize>,
    ) -> HashSet<usize> {
        let mut result = HashSet::new();
        match expression.unlocated() {
            Expr::Name(name) => {
                // A semantically resolved function symbol is not a caller-local
                // shadow. Binding alternatives, including lexical initializers,
                // are merged rather than letting a same-named scope hide an edge.
                if !self.checked.constant_sources.contains_key(&expression.id())
                    && let Some(function) = self.functions.get(name)
                {
                    return HashSet::from([Self::recursion_identity(function)]);
                }
                if let Some(targets) = scope.get(name) {
                    result.extend(targets);
                }
                if let Some(Some(key)) = self.checked.constant_sources.get(&expression.id())
                    && let Some(value) = self.expressions.get(key)
                {
                    result.extend(self.recursion_abstract(value, scope, returns, visiting));
                }
                if !scope.contains_key(name)
                    && matches!(
                        self.checked.constant_sources.get(&expression.id()),
                        Some(None)
                    )
                    && self.recursion_callable_type(expression)
                {
                    result.insert(usize::MAX);
                }
            }
            Expr::Lambda(function) => {
                result.insert(Self::recursion_identity(function));
            }
            Expr::Call { callee, args, .. } => {
                if matches!(callee.unlocated(), Expr::Member { .. })
                    && self.recursion_leaf_call(callee)
                {
                    for arg in args {
                        result.extend(self.recursion_abstract(arg, scope, returns, visiting));
                    }
                }
                for target in self.recursion_abstract(callee, scope, returns, visiting) {
                    if target == usize::MAX {
                        result.insert(target);
                    }
                    if let Some(values) = returns.get(&target) {
                        result.extend(values);
                    }
                }
            }
            Expr::Array(values) | Expr::Tuple(values) => {
                for value in values {
                    result.extend(self.recursion_abstract(value, scope, returns, visiting));
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    result.extend(self.recursion_abstract(value, scope, returns, visiting));
                }
            }
            Expr::Map(entries) => {
                for (key, value) in entries {
                    result.extend(self.recursion_abstract(key, scope, returns, visiting));
                    result.extend(self.recursion_abstract(value, scope, returns, visiting));
                }
            }
            Expr::Index { object, .. }
            | Expr::Member { object, .. }
            | Expr::Cast { value: object, .. }
            | Expr::Try(object) => {
                result.extend(self.recursion_abstract(object, scope, returns, visiting));
            }
            Expr::Binary {
                left,
                op: BinaryOp::Concat,
                right,
            } => {
                result.extend(self.recursion_abstract(left, scope, returns, visiting));
                result.extend(self.recursion_abstract(right, scope, returns, visiting));
            }
            Expr::If { arms, .. } => {
                for (_, body) in arms {
                    result.extend(self.recursion_block_targets(body, scope, returns, visiting));
                }
            }
            Expr::Else { value, fallback }
            | Expr::Catch {
                value,
                body: fallback,
                ..
            } => {
                result.extend(self.recursion_abstract(value, scope, returns, visiting));
                result.extend(self.recursion_block_targets(fallback, scope, returns, visiting));
            }
            Expr::None => {}
            _ if self.recursion_callable_type(expression) => {
                result.insert(usize::MAX);
            }
            _ => {}
        }
        result
    }

    fn recursion_block_targets(
        &self,
        body: &Block,
        scope: &HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
        visiting: &mut HashSet<usize>,
    ) -> HashSet<usize> {
        let mut targets = HashSet::new();
        for statement in Self::recursion_block_statements(body) {
            if let Stmt::Expr(value) | Stmt::Return(Some(value)) | Stmt::Break(Some(value), _) =
                statement
            {
                targets.extend(self.recursion_abstract(value, scope, returns, visiting));
            }
        }
        targets
    }

    fn recursion_pattern_names(pattern: &Pattern) -> Vec<&str> {
        let mut pending = vec![pattern];
        let mut names = Vec::new();
        while let Some(pattern) = pending.pop() {
            match pattern {
                Pattern::Name(name) => names.push(name.as_str()),
                Pattern::Tuple(patterns)
                | Pattern::Array(patterns)
                | Pattern::Variant {
                    values: patterns, ..
                } => pending.extend(patterns),
                Pattern::Struct { fields, .. } => {
                    pending.extend(fields.iter().map(|(_, pattern)| pattern));
                }
                _ => {}
            }
        }
        names
    }

    fn recursion_callable_type(&self, expression: &Expr) -> bool {
        let mut pending: Vec<_> = self.expr_type(expression).into_iter().collect();
        let mut named = HashSet::new();
        while let Some(ty) = pending.pop() {
            match ty {
                Type::Function(_, _) => return true,
                Type::Array(inner, _)
                | Type::Optional(inner)
                | Type::ErrorUnion(inner)
                | Type::Future(inner) => pending.push(inner),
                Type::Map(key, value) => pending.extend([&**key, &**value]),
                Type::Tuple(types) => pending.extend(types),
                Type::Named(name, _) if named.insert(name) => match self.checked.types.get(name) {
                    Some(TypeInfo::Alias(base)) => pending.push(base),
                    Some(TypeInfo::Struct(declaration)) => {
                        pending.extend(declaration.fields.iter().map(|field| &field.ty));
                    }
                    Some(TypeInfo::Enum(declaration)) => pending.extend(
                        declaration
                            .variants
                            .iter()
                            .flat_map(|variant| &variant.values),
                    ),
                    _ => {}
                },
                Type::Named(..) => {}
            }
        }
        false
    }

    fn recursion_leaf_call(&self, callee: &Expr) -> bool {
        match callee.unlocated() {
            Expr::Name(name) => name.starts_with('@'),
            Expr::Member { object, .. } => {
                matches!(object.unlocated(), Expr::Name(owner)
                    if matches!(self.checked.types.get(owner), Some(TypeInfo::Enum(_))))
            }
            _ => false,
        }
    }

    /// Monotone callable-flow analysis reaches a finite fixed point. Unknown
    /// targets never become permission to execute; only the reachable graph is
    /// checked, including callbacks returned by factories and stored in containers.
    fn recursion_graph_proven(
        &self,
        root: &Function,
        values: &HashMap<String, Value>,
    ) -> Option<HashMap<usize, usize>> {
        let mut functions: HashMap<_, _> = self
            .functions
            .values()
            .chain(self.lambdas.values())
            .map(|function| (Self::recursion_identity(function), *function))
            .collect();
        let root_id = Self::recursion_identity(root);
        functions.insert(root_id, root);
        let mut scopes = HashMap::new();
        self.recursion_seed(values, &mut scopes);
        let mut root_scope = HashMap::new();
        for (name, value) in values {
            root_scope.insert(name.clone(), self.recursion_targets(value));
        }
        scopes.insert(root_id, root_scope);
        let mut returns = HashMap::<usize, HashSet<usize>>::new();
        let mut graph = HashMap::<usize, HashSet<usize>>::new();
        let mut active = HashSet::from([root_id]);
        loop {
            let mut changed = false;
            for id in active.clone() {
                let function = functions[&id];
                let mut scope = scopes.get(&id).cloned().unwrap_or_default();
                changed |= self.recursion_bindings(function, &mut scope, &returns);
                scopes.insert(id, scope.clone());
                for expression in self.recursion_expressions(function) {
                    if let Expr::Lambda(child) = expression {
                        let child_scope =
                            scopes.entry(Self::recursion_identity(child)).or_default();
                        for (name, targets) in &scope {
                            changed |= Self::recursion_merge(child_scope, name, targets);
                        }
                    }
                    if let Expr::Call { callee, args, .. } = expression {
                        if self.recursion_leaf_call(callee) {
                            continue;
                        }
                        let targets =
                            self.recursion_abstract(callee, &scope, &returns, &mut HashSet::new());
                        for target in targets {
                            if target == usize::MAX {
                                return None;
                            }
                            changed |= graph.entry(id).or_default().insert(target);
                            changed |= active.insert(target);
                            let child = functions.get(&target)?;
                            let child_scope = scopes.entry(target).or_default();
                            for (param, arg) in child.params.iter().zip(args) {
                                let targets = self.recursion_abstract(
                                    arg,
                                    &scope,
                                    &returns,
                                    &mut HashSet::new(),
                                );
                                changed |=
                                    Self::recursion_merge(child_scope, &param.name, &targets);
                            }
                        }
                    }
                }
                for statement in Self::recursion_statements(function) {
                    if let Stmt::Return(Some(value)) = statement {
                        let targets =
                            self.recursion_abstract(value, &scope, &returns, &mut HashSet::new());
                        let entry = returns.entry(id).or_default();
                        let before = entry.len();
                        entry.extend(targets);
                        changed |= entry.len() != before;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.recursion_certify_graph(&functions, &scopes, &returns, &graph, &active)
    }

    fn recursion_bindings(
        &self,
        function: &Function,
        scope: &mut HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
    ) -> bool {
        let mut changed = self.recursion_scoped_callable_barriers(function, scope);
        for expression in self.recursion_expressions(function) {
            if let Expr::If {
                subject: Some(subject),
                arms,
            } = expression
            {
                let targets = self.recursion_abstract(subject, scope, returns, &mut HashSet::new());
                for (patterns, _) in arms {
                    for pattern in patterns {
                        for name in Self::recursion_pattern_names(pattern) {
                            changed |= Self::recursion_merge(scope, name, &targets);
                        }
                    }
                }
            }
        }
        for statement in Self::recursion_statements(function) {
            match statement {
                Stmt::Var(declaration) => {
                    let targets = self.recursion_abstract(
                        &declaration.value,
                        scope,
                        returns,
                        &mut HashSet::new(),
                    );
                    for name in declaration.binding_names() {
                        changed |= Self::recursion_merge(scope, name, &targets);
                    }
                }
                Stmt::Assign { target, value } => {
                    let mut root = target.unlocated();
                    while let Expr::Index { object, .. } | Expr::Member { object, .. } = root {
                        root = object.unlocated();
                    }
                    if let Expr::Name(name) = root {
                        let targets =
                            self.recursion_abstract(value, scope, returns, &mut HashSet::new());
                        changed |= Self::recursion_merge(scope, name, &targets);
                    }
                }
                _ => {}
            }
        }
        changed
    }

    fn recursion_calls_known(
        &self,
        function: &Function,
        scope: &HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
    ) -> bool {
        for statement in Self::recursion_statements(function) {
            if let Stmt::Assign { target, value } = statement
                && self.recursion_callable_write(function, target, value, scope, returns)
            {
                return false;
            }
        }
        self.recursion_expressions(function)
            .iter()
            .all(|expression| {
                let Expr::Call { callee, .. } = expression else {
                    return true;
                };
                if self.recursion_leaf_call(callee) {
                    return true;
                }
                let targets = self.recursion_abstract(callee, scope, returns, &mut HashSet::new());
                !targets.is_empty() && !targets.contains(&usize::MAX)
            })
    }

    fn recursion_callable_write(
        &self,
        function: &Function,
        target: &Expr,
        value: &Expr,
        scope: &HashMap<String, HashSet<usize>>,
        returns: &HashMap<usize, HashSet<usize>>,
    ) -> bool {
        let mut root = target.unlocated();
        while let Expr::Index { object, .. } | Expr::Member { object, .. } = root {
            root = object.unlocated();
        }
        let Expr::Name(name) = root else {
            return true;
        };
        if !self.recursion_callable_type(root)
            && !self.recursion_callable_type(value)
            && self
                .recursion_abstract(value, scope, returns, &mut HashSet::new())
                .is_empty()
        {
            return false;
        }
        // Callable writes through shared captures/globals need cross-call storage
        // provenance. Until that is represented, reject rather than miss an edge.
        if self.checked.shared_accesses.contains(&root.id())
            || self.lambdas.iter().any(|(key, lambda)| {
                std::ptr::eq(*lambda, function)
                    && self.checked.captures.get(key).is_some_and(|captures| {
                        captures
                            .iter()
                            .any(|capture| capture.mutable && capture.name == *name)
                    })
            })
        {
            return true;
        }
        !function.params.iter().any(|param| param.name == *name)
            && !Self::recursion_statements(function).iter().any(|statement| {
                matches!(statement, Stmt::Var(declaration) if declaration.binding_names().contains(&name.as_str()))
            })
    }

    fn recursion_reaches(
        graph: &HashMap<usize, HashSet<usize>>,
        start: usize,
        target: usize,
    ) -> bool {
        let mut pending = vec![start];
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if id == target {
                return true;
            }
            if visited.insert(id)
                && let Some(edges) = graph.get(&id)
            {
                pending.extend(edges);
            }
        }
        false
    }

    fn recursion_return(block: &Block) -> Option<&Expr> {
        let [statement] = block.statements.as_slice() else {
            return None;
        };
        match statement.unlocated() {
            Stmt::Return(Some(value)) => Some(value),
            _ => None,
        }
    }

    fn recursion_rank_literal(function: &Function, rank: usize, expression: &Expr) -> Option<u64> {
        let floating = matches!(&function.params[rank].ty, Type::Named(name, _) if name == "float");
        if matches!(expression.unlocated(), Expr::Float(_)) != floating
            || !matches!(expression.unlocated(), Expr::Int(_) | Expr::Float(_))
        {
            return None;
        }
        Self::recursion_integer(expression)
    }

    fn recursion_integer(expression: &Expr) -> Option<u64> {
        match expression.unlocated() {
            Expr::Int(text) => crate::lexer::integer(text).ok(),
            Expr::Float(text) => {
                let value: f64 = text.parse().ok()?;
                if (0.0..=9_007_199_254_740_992.0).contains(&value)
                    && value.fract().abs().to_bits() == 0
                {
                    value.to_string().parse().ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Integer-valued floats through 2^53 form an exact subtraction domain:
    /// every admitted decrement is positive, integral, and cannot round away.
    fn recursion_domain(value: &Value) -> bool {
        match value {
            Value::Int(value) => *value >= 0,
            Value::Uint(_) | Value::Byte(_) => true,
            Value::Float(bits) => {
                let value = f64::from_bits(*bits);
                (0.0..=9_007_199_254_740_992.0).contains(&value)
                    && value.fract().abs().to_bits() == 0
            }
            _ => false,
        }
    }

    fn recursion_call_free(expression: &Expr) -> bool {
        let mut pending = vec![expression];
        while let Some(expression) = pending.pop() {
            match expression.unlocated() {
                Expr::Call { .. } => return false,
                Expr::Unary { value, .. } => pending.push(value),
                Expr::Binary { left, right, .. } => pending.extend([&**left, &**right]),
                _ => {}
            }
        }
        true
    }

    fn recursion_result(&mut self, expression: &Expr, value: Option<Value>) -> Option<Value> {
        if self.arithmetic_failure && self.arithmetic_location.is_none() {
            self.arithmetic_location = expression.location().cloned();
        }
        let value = value?;
        if let Some(ty) = self.checked.expression_types.get(&expression.id()) {
            self.coerce(value, ty)
        } else {
            Some(value)
        }
    }

    fn recursion_binary(&mut self, expression: &Expr, left: Value, right: Value) -> Option<Value> {
        let Expr::Binary { op, .. } = expression.unlocated() else {
            return None;
        };
        let value = if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && !matches!(left, Value::Float(_))
        {
            let equal = left.equals(&right);
            Some(Value::Bool(if *op == BinaryOp::Eq {
                equal
            } else {
                !equal
            }))
        } else {
            self.binary_operands(left, *op, right)
        };
        self.recursion_result(expression, value)
    }

    fn recursion_unary(&mut self, expression: &Expr, value: Value) -> Option<Value> {
        let Expr::Unary { op, .. } = expression.unlocated() else {
            return None;
        };
        // Reuse the ordinary unary semantics without re-evaluating its operand.
        let operand = Expr::Name("recursion_operand".into());
        let mut scope = HashMap::from([("recursion_operand".into(), value)]);
        let value = self.unary_value(*op, &operand, &mut scope);
        self.recursion_result(expression, value)
    }

    /// Only certified expression grammar enters this machine. Recursive calls
    /// push heap tasks and scopes, never evaluator/Rust stack frames. Leaves use
    /// the normal evaluator for typing, arithmetic diagnostics and source spans.
    fn recursion_heap(
        &mut self,
        function: &Function,
        callable: &Value,
        arguments: &[Value],
        scope: &HashMap<String, Value>,
        rank: usize,
        ranks: &HashMap<usize, usize>,
    ) -> Option<Value> {
        enum Task<'a> {
            Eval(&'a Expr, usize),
            Left(&'a Expr, usize),
            Right(&'a Expr, Value),
            Unary(&'a Expr),
            Leave(usize),
        }
        // Each frame owns the resolved callable and argument snapshots. Shared
        // callable captures remain live cells, and never become memo keys.
        let mut scope = scope.clone();
        let body = self.recursion_selected_body(function, &mut scope, rank)?;
        let mut frames = vec![(scope, arguments.to_vec(), callable.clone(), function)];
        let mut tasks = vec![Task::Leave(0), Task::Eval(body, 0)];
        let mut values = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Eval(expression, frame) => {
                    if Self::recursion_call_free(expression) {
                        values.push(self.evaluate(expression, &mut frames[frame].0)?);
                        continue;
                    }
                    match expression.unlocated() {
                        Expr::Binary { left, .. } => {
                            tasks.extend([Task::Left(expression, frame), Task::Eval(left, frame)]);
                        }
                        Expr::Unary { value, .. } => {
                            tasks.extend([Task::Unary(expression), Task::Eval(value, frame)]);
                        }
                        Expr::Call { callee, args, .. } => {
                            let callable = self.evaluate(callee, &mut frames[frame].0)?;
                            let (child, mut scope) = match &callable {
                                Value::Function(name) => {
                                    (*self.functions.get(name)?, HashMap::new())
                                }
                                Value::Closure(key, captures) => {
                                    (*self.lambdas.get(key)?, captures.iter().cloned().collect())
                                }
                                _ => return None,
                            };
                            let mut arguments = Vec::new();
                            for (arg, param) in args.iter().zip(&child.params) {
                                let value = self.evaluate(arg, &mut frames[frame].0)?;
                                let value = self.coerce(value, &param.ty)?;
                                scope.insert(param.name.clone(), value.clone());
                                arguments.push(value);
                            }
                            let rank = *ranks.get(&Self::recursion_identity(child))?;
                            let body = self.recursion_selected_body(child, &mut scope, rank)?;
                            let key = (callable.clone(), arguments.clone());
                            let cacheable =
                                self.recursion_heap_cacheable(child, &callable, &arguments);
                            if cacheable && let Some(value) = self.memo.get(&key) {
                                values.push(value.clone());
                            } else {
                                let next = frames.len();
                                frames.push((scope, arguments, callable, child));
                                tasks.extend([Task::Leave(next), Task::Eval(body, next)]);
                            }
                        }
                        _ => return None,
                    }
                }
                Task::Left(expression, frame) => {
                    let left = values.pop()?;
                    let Expr::Binary { op, right, .. } = expression.unlocated() else {
                        return None;
                    };
                    if (*op == BinaryOp::And && left == Value::Bool(false))
                        || (*op == BinaryOp::Or && left == Value::Bool(true))
                    {
                        values.push(self.recursion_result(expression, Some(left))?);
                    } else {
                        tasks.extend([Task::Right(expression, left), Task::Eval(right, frame)]);
                    }
                }
                Task::Right(expression, left) => {
                    let right = values.pop()?;
                    values.push(self.recursion_binary(expression, left, right)?);
                }
                Task::Unary(expression) => {
                    let value = values.pop()?;
                    values.push(self.recursion_unary(expression, value)?);
                }
                Task::Leave(frame) => {
                    debug_assert_eq!(frame + 1, frames.len());
                    let (_, arguments, callable, function) = frames.pop()?;
                    let value = self.coerce(values.pop()?, &function.return_type)?;
                    if self.recursion_heap_cacheable(function, &callable, &arguments)
                        && !value.contains_cell()
                    {
                        self.memo
                            .insert((callable.clone(), arguments), value.clone());
                    }
                    values.push(value);
                }
            }
        }
        values.pop()
    }

    fn unary_value(
        &mut self,
        op: UnaryOp,
        value: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        match (op, self.evaluate(value, env)?) {
            (UnaryOp::Neg, Value::Int(v)) => {
                let result = v.checked_neg();
                if result.is_none() {
                    self.arithmetic_failure = true;
                }
                result.map(Value::Int)
            }
            (UnaryOp::Neg, Value::Float(v)) => Some(Value::Float((-f64::from_bits(v)).to_bits())),
            (UnaryOp::Not, Value::Bool(b)) => Some(Value::Bool(!b)),
            (UnaryOp::BitNot, Value::Int(v)) => Some(Value::Int(!v)),
            (UnaryOp::BitNot, Value::Uint(v)) => Some(Value::Uint(!v)),
            (UnaryOp::BitNot, Value::Byte(v)) => Some(Value::Byte(!v)),
            _ => None,
        }
    }
    fn signed(&mut self, a: i64, b: i64, op: BinaryOp) -> Option<Value> {
        use BinaryOp::{
            Add, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mod, Mul, Ne, Pow, Shl, Shr, Sub,
        };
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
                .and_then(|b| i64::try_from(i128::from(a) << b).ok())
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
        if result.is_none() && matches!(op, Add | Sub | Mul | Div | Mod | Pow | Shl | Shr) {
            self.arithmetic_failure = true;
        }
        result
    }
    fn binary_value(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Option<Value> {
        let left = self.evaluate(left, env)?;
        if op == BinaryOp::And && left == Value::Bool(false) {
            return Some(left);
        }
        if op == BinaryOp::Or && left == Value::Bool(true) {
            return Some(left);
        }
        let right = self.evaluate(right, env)?;
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && !matches!(left, Value::Float(_)) {
            let equal = left.equals(&right);
            return Some(Value::Bool(if op == BinaryOp::Eq { equal } else { !equal }));
        }
        self.binary_operands(left, op, right)
    }
    fn binary_operands(&mut self, left: Value, op: BinaryOp, right: Value) -> Option<Value> {
        match (left, right) {
            (Value::Map(mut a), Value::Map(b)) if op == BinaryOp::Concat => {
                for (key, value) in b {
                    if let Some((_, existing)) = a.iter_mut().find(|(other, _)| other.equals(&key))
                    {
                        *existing = value;
                    } else {
                        a.push((key, value));
                    }
                }
                Some(Value::Map(a))
            }
            (Value::Array(mut a), Value::Array(b)) if op == BinaryOp::Concat => {
                a.extend(b);
                Some(Value::Array(a))
            }
            (value, Value::Array(values)) if op == BinaryOp::In => Some(Value::Bool(
                values.iter().any(|element| element.equals(&value)),
            )),
            (value, Value::Map(values)) if op == BinaryOp::In => Some(Value::Bool(
                values.iter().any(|(key, _)| key.equals(&value)),
            )),
            (Value::Int(a), Value::Int(b)) => self.signed(a, b, op),
            (Value::Uint(a), Value::Uint(b)) => self.unsigned(a, b, op, false),
            (Value::Byte(a), Value::Byte(b)) => self.unsigned(a.into(), b.into(), op, true),
            (Value::Float(a), Value::Float(b)) => {
                Self::float(f64::from_bits(a), f64::from_bits(b), op)
            }
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinaryOp::And => Some(Value::Bool(a && b)),
                BinaryOp::Or => Some(Value::Bool(a || b)),
                BinaryOp::Eq => Some(Value::Bool(a == b)),
                BinaryOp::Ne => Some(Value::Bool(a != b)),
                _ => None,
            },
            (Value::String(a), Value::String(b)) => match op {
                BinaryOp::Concat => {
                    let mut a = a;
                    a.extend(b);
                    Some(Value::String(a))
                }
                BinaryOp::Eq => Some(Value::Bool(a == b)),
                BinaryOp::Ne => Some(Value::Bool(a != b)),
                BinaryOp::In => Some(Value::Bool(
                    a.is_empty() || b.windows(a.len()).any(|part| part == a),
                )),
                _ => None,
            },
            (Value::Char(a), Value::String(b)) if op == BinaryOp::In => {
                Some(Value::Bool(b.contains(&a)))
            }
            (Value::Char(a), Value::Char(b)) => match op {
                BinaryOp::Eq => Some(Value::Bool(a == b)),
                BinaryOp::Ne => Some(Value::Bool(a != b)),
                _ => None,
            },
            _ => None,
        }
    }
    fn flow_value(&mut self, flow: Flow) -> Option<Value> {
        match flow {
            Flow::Value(value) => Some(value),
            Flow::Return(_) | Flow::Throw(_) | Flow::Break(_) | Flow::Continue(_) => {
                self.flow = Some(flow);
                None
            }
            Flow::Next => None,
        }
    }
}
enum Flow {
    Next,
    Return(Value),
    Throw(Vec<String>),
    Value(Value),
    Break(Option<String>),
    Continue(Option<String>),
}
enum Access {
    Field(String),
    Index(Value),
}

fn place_value<'a>(mut value: &'a Value, path: &[Access]) -> Option<&'a Value> {
    for access in path {
        value = match (value, access) {
            (Value::Struct(_, fields), Access::Field(name)) => {
                &fields.iter().find(|(field, _)| field == name)?.1
            }
            (Value::Array(values) | Value::Tuple(values), Access::Index(index)) => {
                values.get(index_number(index)?)?
            }
            (Value::Map(entries), Access::Index(key)) => {
                &entries.iter().find(|(stored, _)| stored.equals(key))?.1
            }
            _ => return None,
        };
    }
    Some(value)
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
            *text.get_mut(n)? = replacement;
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
        self.statements(&b.statements, env, valued)
    }
    fn statements(
        &mut self,
        statements: &[Stmt],
        env: &mut HashMap<String, Value>,
        valued: bool,
    ) -> Option<Flow> {
        let mut declared = vec![];
        let mut result = Flow::Next;
        for (index, s) in statements.iter().enumerate() {
            let flow = self
                .statement_flow(
                    s,
                    env,
                    &mut declared,
                    valued && index + 1 == statements.len(),
                )
                .or_else(|| self.flow.take())?;
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
    fn statement_flow(
        &mut self,
        statement: &Stmt,
        env: &mut HashMap<String, Value>,
        declared: &mut Vec<(String, Option<Value>)>,
        valued: bool,
    ) -> Option<Flow> {
        Some(match statement.unlocated() {
            Stmt::Var(v) => {
                self.declaration(v, env, declared)?;
                Flow::Next
            }
            Stmt::Assign { target, value } => self.assignment(target, value, env)?,
            Stmt::Assert(e) if self.analyse_output => {
                // Assertions remain runtime checks; only proven success permits
                // analysis of subsequent statements with the current state.
                if self.evaluate(e, env)? != Value::Bool(true) {
                    return None;
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
                self.conditional(subject.as_deref(), arms, env, valued)?
            }
            Stmt::Expr(e) if valued => Flow::Value(self.evaluate(e, env)?),
            Stmt::Expr(e) => {
                self.evaluate(e, env)?;
                Flow::Next
            }
            Stmt::While {
                condition,
                body,
                label,
            } => self.while_loop(condition, body, label.as_deref(), env)?,
            Stmt::For {
                name,
                iterable,
                body,
                label,
            } => self.for_loop(name, iterable, body, label.as_deref(), env)?,
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
        })
    }
    fn assignment(
        &mut self,
        target: &Expr,
        value: &Expr,
        env: &mut HashMap<String, Value>,
    ) -> Option<Flow> {
        let v = self.evaluate(value, env)?;
        let mut path = Vec::new();
        let name = self.place(target, env, &mut path)?;
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
        Some(Flow::Next)
    }
    fn while_loop(
        &mut self,
        condition: &Expr,
        body: &Block,
        label: Option<&str>,
        env: &mut HashMap<String, Value>,
    ) -> Option<Flow> {
        if crate::flow::infinite_loop(condition, body, label, &self.checked.expression_types) {
            return None;
        }
        if matches!(condition.unlocated(), Expr::Bool(false)) {
            return Some(Flow::Next);
        }
        // Neither preprocessing nor certification executes candidate helpers.
        if let Some(certificate) = crate::termination::counted_loop(condition, body) {
            if !self.nested_loops_proven(body, None, env) {
                return None;
            }
            let (start, min, max) = self.loop_integer(env.get(certificate.counter)?)?;
            let bound = self.evaluate(certificate.bound, env)?;
            let (bound, _, _) = self.loop_integer(&bound)?;
            if !certificate.terminates(start, bound, min, max) {
                return None;
            }
        } else if !self.helper_loop_proven(condition, body, label, env) {
            return None;
        }
        Some(loop {
            if self.evaluate(condition, env)? != Value::Bool(true) {
                break Flow::Next;
            }
            match self.block(body, env)? {
                Flow::Break(target) if target.is_none() || target.as_deref() == label => {
                    break Flow::Next;
                }
                Flow::Continue(target) if target.is_none() || target.as_deref() == label => {}
                Flow::Next => {}
                flow => break flow,
            }
        })
    }
    fn loop_integer(&self, value: &Value) -> Option<(i128, i128, i128)> {
        match value {
            Value::Cell(index) => self.loop_integer(self.cells.get(*index)?),
            Value::Int(value) => Some((
                i128::from(*value),
                i128::from(i64::MIN),
                i128::from(i64::MAX),
            )),
            Value::Uint(value) => Some((i128::from(*value), 0, i128::from(u64::MAX))),
            Value::Byte(value) => Some((i128::from(*value), 0, i128::from(u8::MAX))),
            _ => None,
        }
    }
    fn for_loop(
        &mut self,
        name: &str,
        iterable: &Expr,
        body: &Block,
        label: Option<&str>,
        env: &mut HashMap<String, Value>,
    ) -> Option<Flow> {
        if !self.nested_loops_proven(body, Some((name, iterable)), env) {
            return None;
        }
        let traversal = match self.evaluate(iterable, env)? {
            Value::Array(elements) => (0..elements.len())
                .map(|i| Value::Uint(i as u64))
                .collect::<Vec<_>>(),
            Value::Map(entries) => entries.into_iter().map(|(key, _)| key).collect(),
            Value::String(text) => (0..text.len()).map(|i| Value::Uint(i as u64)).collect(),
            _ => return None,
        };
        let previous = env.remove(name);
        let mut flow = Flow::Next;
        for key in traversal {
            env.insert(name.to_owned(), key);
            match self.block(body, env)? {
                Flow::Break(target) if target.is_none() || target.as_deref() == label => break,
                Flow::Continue(target) if target.is_none() || target.as_deref() == label => {}
                Flow::Next => {}
                result => {
                    flow = result;
                    break;
                }
            }
        }
        if let Some(previous) = previous {
            env.insert(name.to_owned(), previous);
        } else {
            env.remove(name);
        }
        Some(flow)
    }
}
