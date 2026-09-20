//! Fuel-bounded evaluation of pure scalar expressions, functions and loops.
//! Failed/impure evaluation leaves the original program in place.
use crate::ast::*;
use std::collections::HashMap;

#[derive(Clone, PartialEq, Debug)]
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
pub fn optimize(mut module: Module) -> Module {
    let functions: HashMap<String, Function> = module
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
                fold(&mut v.value, &env, &functions);
                if let Pattern::Name(n) = &v.pattern {
                    env.remove(n);
                }
                if !v.mutable && !v.mutex {
                    if let Pattern::Name(n) = &v.pattern {
                        if let Some(value) = evaluate(&v.value, &env, &functions, &mut 100_000) {
                            env.insert(n.clone(), value);
                        }
                    }
                }
            }
            Item::Statement(Stmt::Expr(e)) => fold(e, &env, &functions),
            _ => {}
        }
    }
    module
}
fn fold(e: &mut Expr, env: &HashMap<String, Value>, functions: &HashMap<String, Function>) {
    // Only fold whole pure evaluations. Do not rewrite expressions inside a
    // short-circuited or potentially effectful expression independently.
    if let Some(v) = evaluate(e, env, functions, &mut 100_000) {
        *e = v.expr();
    } else if let Expr::Call { callee, args, .. } = e {
        if matches!(&**callee,Expr::Name(n) if n.starts_with('@')) {
            for arg in args {
                if let Some(v) = evaluate(arg, env, functions, &mut 100_000) {
                    *arg = v.expr();
                }
            }
        }
    }
}
fn evaluate(
    e: &Expr,
    env: &HashMap<String, Value>,
    functions: &HashMap<String, Function>,
    fuel: &mut usize,
) -> Option<Value> {
    *fuel = fuel.checked_sub(1)?;
    match e {
        Expr::Int(s) if !s.ends_with('u') => i64::try_from(crate::sema::integer(s).ok()?)
            .ok()
            .map(Value::Int),
        Expr::Bool(b) => Some(Value::Bool(*b)),
        Expr::String(s) => Some(Value::String(s.clone())),
        Expr::Name(n) => env.get(n).cloned(),
        Expr::Unary { op, value } => match (op, evaluate(value, env, functions, fuel)?) {
            (UnaryOp::Neg, Value::Int(v)) => v.checked_neg().map(Value::Int),
            (UnaryOp::Not, Value::Bool(b)) => Some(Value::Bool(!b)),
            (UnaryOp::BitNot, Value::Int(v)) => Some(Value::Int(!v)),
            _ => None,
        },
        Expr::Binary { left, op, right } => {
            let left = evaluate(left, env, functions, fuel)?;
            if *op == BinaryOp::And && left == Value::Bool(false) {
                return Some(left);
            }
            if *op == BinaryOp::Or && left == Value::Bool(true) {
                return Some(left);
            }
            let right = evaluate(right, env, functions, fuel)?;
            match (left, right) {
                (Value::Int(a), Value::Int(b)) => {
                    use BinaryOp::*;
                    match op {
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
                    }
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
            let f = functions.get(n)?;
            if f.return_type != Type::Named("int".into(), vec![])
                && f.return_type != Type::Named("bool".into(), vec![])
                && f.return_type != Type::Named("str".into(), vec![])
            {
                return None;
            }
            let mut scope = HashMap::new();
            for (p, arg) in f.params.iter().zip(args) {
                if !matches!(&p.ty,Type::Named(n,_) if matches!(n.as_str(),"int"|"bool"|"str")) {
                    return None;
                }
                scope.insert(p.name.clone(), evaluate(arg, env, functions, fuel)?);
            }
            match block(&f.body, &mut scope, functions, fuel)? {
                Flow::Return(v) => Some(v),
                _ => None,
            }
        }
        _ => None,
    }
}
enum Flow {
    Next,
    Return(Value),
    Break,
    Continue,
}
fn block(
    b: &Block,
    env: &mut HashMap<String, Value>,
    fs: &HashMap<String, Function>,
    fuel: &mut usize,
) -> Option<Flow> {
    let before = env.clone();
    let mut declared = vec![];
    for s in &b.statements {
        *fuel = fuel.checked_sub(1)?;
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
                let value = evaluate(&v.value, env, fs, fuel)?;
                env.insert(n.clone(), value);
                declared.push(n.clone());
                Flow::Next
            }
            Stmt::Assign { target, value } => {
                let Expr::Name(n) = target else { return None };
                if !env.contains_key(n) {
                    return None;
                }
                let v = evaluate(value, env, fs, fuel)?;
                env.insert(n.clone(), v);
                Flow::Next
            }
            Stmt::Return(Some(e)) => Flow::Return(evaluate(e, env, fs, fuel)?),
            Stmt::Expr(Expr::If { subject, arms }) => {
                let value = if let Some(e) = subject {
                    evaluate(e, env, fs, fuel)?
                } else {
                    Value::Bool(true)
                };
                let mut chosen = None;
                for (patterns, body) in arms {
                    let mut yes = false;
                    for p in patterns {
                        yes |= match p {
                            Pattern::Wildcard => true,
                            Pattern::Literal(e) => evaluate(e, env, fs, fuel)? == value,
                            Pattern::Name(n) => env.get(n)? == &value,
                            _ => return None,
                        };
                    }
                    if yes {
                        chosen = Some(body);
                        break;
                    }
                }
                block(chosen?, env, fs, fuel)?
            }
            Stmt::While {
                condition,
                body,
                label: None,
            } => {
                loop {
                    if evaluate(condition, env, fs, fuel)? != Value::Bool(true) {
                        break;
                    }
                    match block(body, env, fs, fuel)? {
                        Flow::Return(v) => return Some(Flow::Return(v)),
                        Flow::Break => break,
                        _ => {}
                    }
                }
                Flow::Next
            }
            Stmt::Block(body) => block(body, env, fs, fuel)?,
            Stmt::Break(None, None) => Flow::Break,
            Stmt::Continue(None) => Flow::Continue,
            _ => return None,
        };
        if !matches!(flow, Flow::Next) {
            return Some(flow);
        }
    }
    for n in declared {
        if let Some(value) = before.get(&n) {
            env.insert(n, value.clone());
        } else {
            env.remove(&n);
        }
    }
    Some(Flow::Next)
}
