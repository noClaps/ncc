//! Monomorphize generic applications before checking bodies.
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
        type_templates: module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Struct(s) if !s.generics.is_empty() => Some((s.name.clone(), item.clone())),
                Item::Enum(e) if !e.generics.is_empty() => Some((e.name.clone(), item.clone())),
                _ => None,
            })
            .collect(),
        type_instances: HashMap::new(),
        generated_types: vec![],
        values: HashMap::new(),
        return_type: None,
    };
    module
        .items
        .retain(|i| !matches!(i, Item::Function(f) if !f.generics.is_empty()));
    module.items.retain(|i| {
        !matches!(i, Item::Struct(s) if !s.generics.is_empty())
            && !matches!(i, Item::Enum(e) if !e.generics.is_empty())
    });
    for item in &mut module.items {
        pass.item_types(item, &HashMap::new())?;
        match item {
            Item::Function(f) => pass.function_body(f, &HashMap::new())?,
            Item::Global(v) => pass.variable(v, &HashMap::new())?,
            Item::Statement(s) => pass.statement(s, &HashMap::new())?,
            Item::Test { body, .. } => pass.block(body, &HashMap::new())?,
            _ => {}
        }
    }
    module
        .items
        .extend(pass.generated.into_iter().map(Item::Function));
    module.items.extend(pass.generated_types);
    Ok(module)
}
struct Pass {
    templates: HashMap<String, Function>,
    instances: HashMap<(String, Vec<Type>), String>,
    generated: Vec<Function>,
    type_templates: HashMap<String, Item>,
    type_instances: HashMap<(String, Vec<Type>), String>,
    generated_types: Vec<Item>,
    values: HashMap<String, Type>,
    return_type: Option<Type>,
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
    fn hint(&self, e: &mut Expr, ty: &Type) {
        match (e, ty) {
            (Expr::Call { callee, args, .. }, Type::Named(instance, _)) => {
                if let Expr::Member { object, name } = &mut **callee
                    && let Expr::Name(owner) = &mut **object
                {
                    if self
                        .type_instances
                        .iter()
                        .any(|((base, _), n)| base == owner && n == instance)
                    {
                        *owner = instance.clone();
                    }
                    if owner == instance
                        && let Some(Item::Enum(decl)) = self
                            .generated_types
                            .iter()
                            .find(|i| matches!(i, Item::Enum(d) if d.name == *instance))
                        && let Some(variant) = decl.variants.iter().find(|v| v.name == *name)
                    {
                        for (arg, t) in args.iter_mut().zip(&variant.values) {
                            self.hint(arg, t);
                        }
                    }
                }
            }
            (Expr::Member { object, .. }, Type::Named(instance, _)) => {
                if let Expr::Name(owner) = &mut **object
                    && self
                        .type_instances
                        .iter()
                        .any(|((base, _), n)| base == owner && n == instance)
                {
                    *owner = instance.clone();
                }
            }
            (Expr::Array(xs), Type::Array(t, _)) => {
                for x in xs {
                    self.hint(x, t);
                }
            }
            (Expr::Tuple(xs), Type::Tuple(ts)) => {
                for (x, t) in xs.iter_mut().zip(ts) {
                    self.hint(x, t);
                }
            }
            _ => {}
        }
    }
    fn variable(&mut self, v: &mut VarDecl, b: &HashMap<String, Type>) -> Result<(), Diagnostics> {
        self.ty(&mut v.ty, b)?;
        self.hint(&mut v.value, &v.ty);
        self.expr(&mut v.value, b)?;
        if let Pattern::Name(name) = &v.pattern {
            self.values.insert(name.clone(), v.ty.clone());
        }
        Ok(())
    }
    fn function_body(
        &mut self,
        f: &mut Function,
        b: &HashMap<String, Type>,
    ) -> Result<(), Diagnostics> {
        let values = self.values.clone();
        let ret = self.return_type.replace(f.return_type.clone());
        for p in &f.params {
            self.values.insert(p.name.clone(), p.ty.clone());
        }
        let result = self.block(&mut f.body, b);
        self.values = values;
        self.return_type = ret;
        result
    }
    fn ty(&mut self, ty: &mut Type, bindings: &HashMap<String, Type>) -> Result<(), Diagnostics> {
        substitute(ty, bindings);
        match ty {
            Type::Named(name, args) => {
                for arg in args.iter_mut() {
                    self.ty(arg, bindings)?;
                }
                if let Some(mut item) = self.type_templates.get(name).cloned() {
                    let parameters = match &item {
                        Item::Struct(s) => &s.generics,
                        Item::Enum(e) => &e.generics,
                        _ => unreachable!(),
                    };
                    if parameters.len() != args.len() {
                        return Err(Diagnostics::one(
                            format!("incorrect number of type arguments for `{name}`"),
                            0..0,
                        ));
                    }
                    let key = (name.clone(), args.clone());
                    if let Some(instance) = self.type_instances.get(&key) {
                        *ty = Type::Named(instance.clone(), vec![]);
                        return Ok(());
                    }
                    if self.type_instances.len() >= 256 {
                        return Err(Diagnostics::one(
                            "generic type specialization limit exceeded",
                            0..0,
                        ));
                    }
                    let instance =
                        format!("specialized_type_{}_{}", self.type_instances.len(), name);
                    let bindings = parameters
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect();
                    self.type_instances.insert(key, instance.clone());
                    match &mut item {
                        Item::Struct(s) => {
                            s.name = instance.clone();
                            s.generics.clear();
                        }
                        Item::Enum(e) => {
                            e.name = instance.clone();
                            e.generics.clear();
                        }
                        _ => unreachable!(),
                    }
                    self.item_types(&mut item, &bindings)?;
                    self.generated_types.push(item);
                    *ty = Type::Named(instance, vec![]);
                }
            }
            Type::Array(t, _) | Type::Optional(t) | Type::ErrorUnion(t) | Type::Future(t) => {
                self.ty(t, bindings)?
            }
            Type::Map(k, v) => {
                self.ty(k, bindings)?;
                self.ty(v, bindings)?;
            }
            Type::Tuple(ts) => {
                for t in ts {
                    self.ty(t, bindings)?;
                }
            }
            Type::Function(ts, r) => {
                for t in ts {
                    self.ty(t, bindings)?;
                }
                self.ty(r, bindings)?;
            }
        }
        Ok(())
    }
    fn item_types(
        &mut self,
        item: &mut Item,
        b: &HashMap<String, Type>,
    ) -> Result<(), Diagnostics> {
        match item {
            Item::Struct(s) => {
                for f in &mut s.fields {
                    self.ty(&mut f.ty, b)?;
                }
            }
            Item::Enum(e) => {
                for v in &mut e.variants {
                    for t in &mut v.values {
                        self.ty(t, b)?;
                    }
                }
            }
            Item::Function(f) => {
                for p in &mut f.params {
                    self.ty(&mut p.ty, b)?;
                }
                self.ty(&mut f.return_type, b)?;
            }
            Item::Global(v) => self.ty(&mut v.ty, b)?,
            Item::TypeAlias { ty, .. } => self.ty(ty, b)?,
            Item::Extern { functions, .. } => {
                for f in functions {
                    for p in &mut f.params {
                        self.ty(&mut p.ty, b)?;
                    }
                    self.ty(&mut f.return_type, b)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
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
            self.ty(&mut p.ty, &bindings)?;
        }
        self.ty(&mut f.return_type, &bindings)?;
        self.function_body(&mut f, &bindings)?;
        self.generated.push(f);
        Ok(instance)
    }
    fn block(
        &mut self,
        b: &mut Block,
        bindings: &HashMap<String, Type>,
    ) -> Result<(), Diagnostics> {
        let values = self.values.clone();
        for s in &mut b.statements {
            self.statement(s, bindings)?;
        }
        self.values = values;
        Ok(())
    }
    fn statement(&mut self, s: &mut Stmt, b: &HashMap<String, Type>) -> Result<(), Diagnostics> {
        match s {
            Stmt::Var(v) => {
                self.variable(v, b)?;
            }
            Stmt::Block(body) | Stmt::Lock { body, .. } => self.block(body, b)?,
            Stmt::Expr(e) | Stmt::Assert(e) | Stmt::Throw(e) | Stmt::LabeledIf { value: e, .. } => {
                self.expr(e, b)?
            }
            Stmt::Return(Some(e)) => {
                if let Some(t) = &self.return_type {
                    self.hint(e, t);
                }
                self.expr(e, b)?;
            }
            Stmt::Break(Some(e), _) => self.expr(e, b)?,
            Stmt::Assign { target, value } => {
                if let Expr::Name(name) = target
                    && let Some(t) = self.values.get(name)
                {
                    self.hint(value, t);
                }
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
            Expr::Lambda(f) => {
                for p in &mut f.params {
                    self.ty(&mut p.ty, b)?;
                }
                self.ty(&mut f.return_type, b)?;
                self.function_body(f, b)?;
            }
            Expr::Call {
                callee,
                args,
                generics,
            } => {
                for a in args {
                    self.expr(a, b)?;
                }
                for t in generics.iter_mut() {
                    self.ty(t, b)?;
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
                } else if let Expr::Name(name) = &**callee
                    && self.templates.contains_key(name)
                {
                    return Err(Diagnostics::one(
                        format!("generic function `{name}` requires explicit type arguments"),
                        0..0,
                    ));
                }
            }
            Expr::Cast { ty, value } => {
                let constructor = matches!((&*ty, &**value), (Type::Named(n, args), Expr::StructInit { name, .. }) if n == name && !args.is_empty());
                self.ty(ty, b)?;
                if constructor {
                    let Type::Named(instance, _) = ty else {
                        unreachable!()
                    };
                    if let Expr::StructInit { name, .. } = &mut **value {
                        *name = instance.clone();
                    }
                }
                self.expr(value, b)?;
                if constructor {
                    *e = *value.clone();
                }
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
                let subject_type = subject.as_ref().and_then(|s| {
                    if let Expr::Name(n) = &**s {
                        self.values.get(n).cloned()
                    } else {
                        None
                    }
                });
                if let Some(s) = subject {
                    self.expr(s, b)?;
                }
                for (patterns, body) in arms {
                    for p in patterns {
                        if let Some(Type::Named(instance, _)) = &subject_type
                            && let Pattern::Variant { name, .. } = p
                            && let Some((owner, variant)) = name.rsplit_once('.')
                            && self
                                .type_instances
                                .iter()
                                .any(|((base, _), n)| base == owner && n == instance)
                        {
                            *name = format!("{instance}.{variant}");
                        }
                        if let Pattern::Literal(e) = p {
                            if let Some(t) = &subject_type {
                                self.hint(e, t);
                            }
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
