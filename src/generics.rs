//! Monomorphize explicit generic function applications before checking bodies.
use crate::{ast::*, diagnostic::Diagnostics};
use std::collections::HashMap;

pub fn specialize(mut module: Module) -> Result<Module, Diagnostics> {
    let templates = module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(f) = item {
                if !f.generics.is_empty() {
                    Some((f.name.clone(), f.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    let mut pass = Pass {
        templates,
        instances: HashMap::new(),
        generated: vec![],
    };
    module
        .items
        .retain(|i| !matches!(i, Item::Function(f) if !f.generics.is_empty()));
    for item in &mut module.items {
        match item {
            Item::Function(f) => pass.block(&mut f.body, &HashMap::new())?,
            Item::Global(v) => pass.expr(&mut v.value, &HashMap::new())?,
            Item::Statement(s) => pass.statement(s, &HashMap::new())?,
            Item::Test { body, .. } => pass.block(body, &HashMap::new())?,
            _ => {}
        }
    }
    module
        .items
        .extend(pass.generated.into_iter().map(Item::Function));
    Ok(module)
}
struct Pass {
    templates: HashMap<String, Function>,
    instances: HashMap<(String, Vec<Type>), String>,
    generated: Vec<Function>,
}
fn substitute(ty: &mut Type, bindings: &HashMap<String, Type>) {
    match ty {
        Type::Named(n, args) => {
            if let Some(t) = bindings.get(n) {
                *ty = t.clone();
            } else {
                for t in args {
                    substitute(t, bindings);
                }
            }
        }
        Type::Array(t, _) | Type::Optional(t) | Type::ErrorUnion(t) | Type::Future(t) => {
            substitute(t, bindings)
        }
        Type::Map(k, v) => {
            substitute(k, bindings);
            substitute(v, bindings);
        }
        Type::Tuple(ts) => {
            for t in ts {
                substitute(t, bindings);
            }
        }
        Type::Function(ts, r) => {
            for t in ts {
                substitute(t, bindings);
            }
            substitute(r, bindings);
        }
    }
}
impl Pass {
    fn instance(&mut self, name: &str, arguments: &[Type]) -> Result<String, Diagnostics> {
        let key = (name.into(), arguments.to_vec());
        if let Some(name) = self.instances.get(&key) {
            return Ok(name.clone());
        }
        let Some(mut f) = self.templates.get(name).cloned() else {
            return Err(Diagnostics::one(
                format!("`{name}` is not a generic function"),
                0..0,
            ));
        };
        if f.generics.len() != arguments.len() {
            return Err(Diagnostics::one(
                format!("incorrect number of type arguments for `{name}`"),
                0..0,
            ));
        }
        if self.instances.len() >= 256 {
            return Err(Diagnostics::one(
                "generic specialization limit exceeded (possibly infinitely expanding recursion)",
                0..0,
            ));
        }
        let instance = format!("specialized_{}_{}", self.instances.len(), name);
        self.instances.insert(key, instance.clone());
        let bindings = f
            .generics
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect();
        f.name = instance.clone();
        f.generics.clear();
        for p in &mut f.params {
            substitute(&mut p.ty, &bindings);
        }
        substitute(&mut f.return_type, &bindings);
        self.block(&mut f.body, &bindings)?;
        self.generated.push(f);
        Ok(instance)
    }
    fn block(
        &mut self,
        b: &mut Block,
        bindings: &HashMap<String, Type>,
    ) -> Result<(), Diagnostics> {
        for s in &mut b.statements {
            self.statement(s, bindings)?;
        }
        Ok(())
    }
    fn statement(&mut self, s: &mut Stmt, b: &HashMap<String, Type>) -> Result<(), Diagnostics> {
        match s {
            Stmt::Var(v) => {
                substitute(&mut v.ty, b);
                self.expr(&mut v.value, b)?;
            }
            Stmt::Block(body) | Stmt::Lock { body, .. } => self.block(body, b)?,
            Stmt::Expr(e) | Stmt::Assert(e) | Stmt::Throw(e) => self.expr(e, b)?,
            Stmt::Return(e) | Stmt::Break(e, _) => {
                if let Some(e) = e {
                    self.expr(e, b)?;
                }
            }
            Stmt::Assign { target, value } => {
                self.expr(target, b)?;
                self.expr(value, b)?;
            }
            Stmt::While {
                condition, body, ..
            } => {
                self.expr(condition, b)?;
                self.block(body, b)?;
            }
            Stmt::For { iterable, body, .. } => {
                self.expr(iterable, b)?;
                self.block(body, b)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn expr(&mut self, e: &mut Expr, b: &HashMap<String, Type>) -> Result<(), Diagnostics> {
        match e {
            Expr::Call {
                callee,
                args,
                generics,
            } => {
                for a in args {
                    self.expr(a, b)?;
                }
                for t in generics.iter_mut() {
                    substitute(t, b);
                }
                if !generics.is_empty() {
                    let Expr::Name(name) = &**callee else {
                        return Err(Diagnostics::one(
                            "generic call requires a named function",
                            0..0,
                        ));
                    };
                    let name = self.instance(name, generics)?;
                    **callee = Expr::Name(name);
                    generics.clear();
                } else if let Expr::Name(name) = &**callee {
                    if self.templates.contains_key(name) {
                        return Err(Diagnostics::one(
                            format!("generic function `{name}` requires explicit type arguments"),
                            0..0,
                        ));
                    }
                }
            }
            Expr::Cast { ty, value } => {
                substitute(ty, b);
                self.expr(value, b)?;
            }
            Expr::Unary { value, .. }
            | Expr::Async(value)
            | Expr::Await(value)
            | Expr::Try(value) => self.expr(value, b)?,
            Expr::Binary { left, right, .. } => {
                self.expr(left, b)?;
                self.expr(right, b)?;
            }
            Expr::Index { object, index } => {
                self.expr(object, b)?;
                self.expr(index, b)?;
            }
            Expr::Member { object, .. } => self.expr(object, b)?,
            Expr::Array(xs) | Expr::Tuple(xs) => {
                for x in xs {
                    self.expr(x, b)?;
                }
            }
            Expr::Map(xs) => {
                for (k, v) in xs {
                    self.expr(k, b)?;
                    self.expr(v, b)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, v) in fields {
                    self.expr(v, b)?;
                }
            }
            Expr::If { subject, arms } => {
                if let Some(s) = subject {
                    self.expr(s, b)?;
                }
                for (patterns, body) in arms {
                    for p in patterns {
                        if let Pattern::Literal(e) = p {
                            self.expr(e, b)?;
                        }
                    }
                    self.block(body, b)?;
                }
            }
            Expr::Else { value, fallback } => {
                self.expr(value, b)?;
                self.block(fallback, b)?;
            }
            Expr::Catch { value, body, .. } => {
                self.expr(value, b)?;
                self.block(body, b)?;
            }
            _ => {}
        }
        Ok(())
    }
}
