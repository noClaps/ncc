//! Fuel-bounded evaluation of pure scalar expressions, functions and loops.
//! Failed/impure evaluation leaves the original program in place.
use crate::{ast::*, diagnostic::Diagnostics};
use std::collections::{HashMap, HashSet};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Value {
    Int(i64),
    Bool(bool),
    String(String),
}
impl Value {
    fn expr(self) -> Expr {
        match self {
            Self::Int(n) if n < 0 => Expr::Unary {
                op: UnaryOp::Neg,
                value: Box::new(Expr::Int(n.unsigned_abs().to_string())),
            },
            Self::Int(n) => Expr::Int(n.to_string()),
            Self::Bool(b) => Expr::Bool(b),
            Self::String(s) => Expr::String(s),
        }
    }
}
pub fn optimize(mut module: Module) -> Result<Module, Diagnostics> {
    let mut functions: HashMap<String, Function> = module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(f) = item {
                Some((f.name.clone(), f.clone()))
            } else {
                None
            }
        })
        .collect();
    let mut env = HashMap::new();
    for item in &mut module.items {
        match item {
            Item::Global(v) => {
                fold(&mut v.value, &env, &functions)?;
                if let Pattern::Name(n) = &v.pattern {
                    env.remove(n);
                    functions.remove(n);
                }
                if !v.mutable
                    && !v.mutex
                    && let Pattern::Name(n) = &v.pattern
                    && let Some(value) = evaluate(&v.value, &env, &functions, &mut 100_000)?
                {
                    env.insert(n.clone(), value);
                }
            }
            Item::Statement(Stmt::Expr(e)) => fold(e, &env, &functions)?,
            _ => {}
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
    e: &mut Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, Function>,
) -> Result<(), Diagnostics> {
    // Only fold whole pure evaluations. Do not rewrite expressions inside a
    // short-circuited or potentially effectful expression independently.
    if let Some(v) = evaluate(e, env, functions, &mut 100_000)? {
        *e = v.expr();
    } else if let Expr::Call { callee, args, .. } = e
        && matches!(&**callee,Expr::Name(n) if n.starts_with('@'))
    {
        for arg in args {
            if let Some(v) = evaluate(arg, env, functions, &mut 100_000)? {
                *arg = v.expr();
            }
        }
    }
    Ok(())
}
fn evaluate(
    e: &Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, Function>,
    fuel: &mut usize,
) -> Result<Option<Value>, Diagnostics> {
    let mut evaluator = Evaluator {
        functions,
        fuel,
        memo: HashMap::new(),
        depth: 0,
        arithmetic_failure: false,
    };
    let value = evaluator.evaluate(e, env);
    if evaluator.arithmetic_failure {
        Err(Diagnostics::one(
            "constant evaluation failed: integer overflow, division by zero, or invalid exponent",
            0..0,
        ))
    } else {
        Ok(value)
    }
}
struct Evaluator<'a> {
    functions: &'a HashMap<String, Function>,
    fuel: &'a mut usize,
    memo: HashMap<(String, Vec<Value>), Value>,
    depth: usize,
    arithmetic_failure: bool,
}
impl Evaluator<'_> {
    fn evaluate(&mut self, e: &Expr, env: &HashMap<String, Value>) -> Option<Value> {
        if self.depth >= 512 {
            return None;
        }
        self.depth += 1;
        let result = self.expression(e, env);
        self.depth -= 1;
        result
    }
    fn expression(&mut self, e: &Expr, env: &HashMap<String, Value>) -> Option<Value> {
        let fuel = &mut *self.fuel;
        *fuel = fuel.checked_sub(1)?;
        match e {
            Expr::Int(s) if !s.ends_with('u') => i64::try_from(crate::sema::integer(s).ok()?)
                .ok()
                .map(Value::Int),
            Expr::Bool(b) => Some(Value::Bool(*b)),
            Expr::String(s) => Some(Value::String(s.clone())),
            Expr::Name(n) => env.get(n).cloned(),
            Expr::Unary { op, value } => match (op, self.evaluate(value, env)?) {
                (UnaryOp::Neg, Value::Int(v)) => v.checked_neg().map(Value::Int),
                (UnaryOp::Not, Value::Bool(b)) => Some(Value::Bool(!b)),
                (UnaryOp::BitNot, Value::Int(v)) => Some(Value::Int(!v)),
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
                match (left, right) {
                    (Value::Int(a), Value::Int(b)) => {
                        use BinaryOp::*;
                        let result = match op {
                            Add => a.checked_add(b).map(Value::Int),
                            Sub => a.checked_sub(b).map(Value::Int),
                            Mul => a.checked_mul(b).map(Value::Int),
                            Div => a.checked_div(b).map(Value::Int),
                            Mod => a.checked_rem(b).map(Value::Int),
                            Pow => a.checked_pow(u32::try_from(b).ok()?).map(Value::Int),
                            Eq => Some(Value::Bool(a == b)),
                            Ne => Some(Value::Bool(a != b)),
                            Lt => Some(Value::Bool(a < b)),
                            Le => Some(Value::Bool(a <= b)),
                            Gt => Some(Value::Bool(a > b)),
                            Ge => Some(Value::Bool(a >= b)),
                            _ => None,
                        };
                        if result.is_none() && matches!(op, Add | Sub | Mul | Div | Mod | Pow) {
                            self.arithmetic_failure = true;
                        }
                        result
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
                        _ => None,
                    },
                    _ => None,
                }
            }
            Expr::Call { callee, args, .. } => {
                let Expr::Name(n) = &**callee else {
                    return None;
                };
                let f = self.functions.get(n)?.clone();
                if f.return_type != Type::Named("int".into(), vec![])
                    && f.return_type != Type::Named("bool".into(), vec![])
                    && f.return_type != Type::Named("str".into(), vec![])
                {
                    return None;
                }
                let mut scope = HashMap::new();
                let mut values = vec![];
                for (p, arg) in f.params.iter().zip(args) {
                    if !matches!(&p.ty,Type::Named(n,_) if matches!(n.as_str(),"int"|"bool"|"str"))
                    {
                        return None;
                    }
                    let value = self.evaluate(arg, env)?;
                    values.push(value.clone());
                    scope.insert(p.name.clone(), value);
                }
                let key = (n.clone(), values);
                if let Some(value) = self.memo.get(&key) {
                    return Some(value.clone());
                }
                match self.block(&f.body, &mut scope)? {
                    Flow::Return(v) => {
                        self.memo.insert(key, v.clone());
                        Some(v)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
enum Flow {
    Next,
    Return(Value),
    Break,
    Continue,
}
impl Evaluator<'_> {
    fn block(&mut self, b: &Block, env: &mut HashMap<String, Value>) -> Option<Flow> {
        let mut declared = vec![];
        let mut result = Flow::Next;
        for s in &b.statements {
            *self.fuel = self.fuel.checked_sub(1)?;
            let flow = match s {
                Stmt::Var(v) => {
                    if v.mutex
                        || !matches!(&v.ty,Type::Named(n,_) if matches!(n.as_str(),"int"|"bool"|"str"))
                    {
                        return None;
                    }
                    let Pattern::Name(n) = &v.pattern else {
                        return None;
                    };
                    let value = self.evaluate(&v.value, env)?;
                    let previous = env.insert(n.clone(), value);
                    declared.push((n.clone(), previous));
                    Flow::Next
                }
                Stmt::Assign { target, value } => {
                    let Expr::Name(n) = target else { return None };
                    if !env.contains_key(n) {
                        return None;
                    }
                    let v = self.evaluate(value, env)?;
                    env.insert(n.clone(), v);
                    Flow::Next
                }
                Stmt::Return(Some(e)) => Flow::Return(self.evaluate(e, env)?),
                Stmt::Expr(Expr::If { subject, arms }) => {
                    let value = if let Some(e) = subject {
                        self.evaluate(e, env)?
                    } else {
                        Value::Bool(true)
                    };
                    let mut chosen = None;
                    for (patterns, body) in arms {
                        let mut yes = false;
                        for p in patterns {
                            yes |= match p {
                                Pattern::Wildcard => true,
                                Pattern::Literal(e) => self.evaluate(e, env)? == value,
                                Pattern::Name(n) => env.get(n)? == &value,
                                _ => return None,
                            };
                        }
                        if yes {
                            chosen = Some(body);
                            break;
                        }
                    }
                    self.block(chosen?, env)?
                }
                Stmt::While {
                    condition,
                    body,
                    label: None,
                } => loop {
                    if self.evaluate(condition, env)? != Value::Bool(true) {
                        break Flow::Next;
                    }
                    match self.block(body, env)? {
                        Flow::Return(v) => break Flow::Return(v),
                        Flow::Break => break Flow::Next,
                        _ => {}
                    }
                },
                Stmt::Block(body) => self.block(body, env)?,
                Stmt::Break(None, None) => Flow::Break,
                Stmt::Continue(None) => Flow::Continue,
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
