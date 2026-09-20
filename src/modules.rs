//! Resolve source modules once, enforce exports, and qualify their symbols.
use crate::{ast::*, diagnostic::Diagnostics, lexer, parser};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
type Names = HashMap<String, String>;

pub fn load(module: Module, path: &Path) -> Result<Module, Diagnostics> {
    let mut loader = Loader {
        done: HashMap::new(),
        active: HashSet::new(),
        items: vec![],
        next: 0,
    };
    loader.visit(module, path, true)?;
    Ok(Module {
        items: loader.items,
    })
}
struct Loader {
    done: HashMap<PathBuf, Names>,
    active: HashSet<PathBuf>,
    items: Vec<Item>,
    next: usize,
}
impl Loader {
    fn visit(&mut self, mut module: Module, path: &Path, root: bool) -> Result<Names, Diagnostics> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(exports) = self.done.get(&key) {
            return Ok(exports.clone());
        }
        if !self.active.insert(key.clone()) {
            return Err(Diagnostics::one(
                format!("cyclic module import: {}", path.display()),
                0..0,
            ));
        }
        let prefix = if root {
            String::new()
        } else {
            self.next += 1;
            format!("m{}_", self.next)
        };
        let mut aliases = HashMap::<String, Names>::new();
        let mut names = Names::new();
        let mut exports = Names::new();
        for item in &module.items {
            if let Some((name, public)) = symbol(item) {
                let qualified = format!("{prefix}{name}");
                names.insert(name.into(), qualified.clone());
                if public {
                    exports.insert(name.into(), qualified);
                }
            }
        }
        for item in &mut module.items {
            match item {
                Item::Import {
                    path: imported,
                    alias,
                } => {
                    if aliases.contains_key(alias) {
                        return Err(Diagnostics::one(
                            format!("duplicate module alias `{alias}`"),
                            0..0,
                        ));
                    }
                    let imported_path = path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(&*imported)
                        .with_extension("nc");
                    let source = std::fs::read_to_string(&imported_path).map_err(|e| {
                        Diagnostics::one(
                            format!("cannot import {}: {e}", imported_path.display()),
                            0..0,
                        )
                    })?;
                    let exports = self.visit(
                        parser::parse_at(lexer::lex(&source)?, &imported_path)?,
                        &imported_path,
                        false,
                    )?;
                    aliases.insert(alias.clone(), exports);
                }
                Item::Extern {
                    path: implementation,
                    alias,
                    functions,
                } => {
                    let external = path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(&*implementation);
                    let external = external.canonicalize().map_err(|e| {
                        Diagnostics::one(
                            format!(
                                "cannot open external implementation {}: {e}",
                                external.display()
                            ),
                            0..0,
                        )
                    })?;
                    *implementation = external.to_string_lossy().into_owned();
                    let mut symbols = Names::new();
                    for f in functions {
                        let qualified = format!("{prefix}extern_{alias}_{}", f.name);
                        symbols.insert(f.name.clone(), qualified.clone());
                        f.name = qualified;
                    }
                    aliases.insert(alias.clone(), symbols);
                }
                _ => {}
            }
        }
        for mut item in module.items {
            if matches!(item, Item::Import { .. }) {
                continue;
            }
            qualify_item(&mut item, &names, &aliases)?;
            self.items.push(item);
        }
        self.active.remove(&key);
        self.done.insert(key, exports.clone());
        Ok(exports)
    }
}
fn symbol(item: &Item) -> Option<(&str, bool)> {
    match item {
        Item::Function(f) => Some((&f.name, f.public)),
        Item::Struct(s) => Some((&s.name, s.public)),
        Item::Enum(e) => Some((&e.name, e.public)),
        Item::TypeAlias { name, public, .. } => Some((name, *public)),
        Item::Global(v) => {
            if let Pattern::Name(name) = &v.pattern {
                Some((name, v.public))
            } else {
                None
            }
        }
        _ => None,
    }
}
fn qualify_type(ty: &mut Type, names: &Names) {
    match ty {
        Type::Named(n, args) => {
            if let Some(name) = names.get(n) {
                *n = name.clone();
            }
            for arg in args {
                qualify_type(arg, names);
            }
        }
        Type::Array(t, _) | Type::Optional(t) | Type::ErrorUnion(t) | Type::Future(t) => {
            qualify_type(t, names)
        }
        Type::Map(k, v) => {
            qualify_type(k, names);
            qualify_type(v, names);
        }
        Type::Tuple(types) => {
            for ty in types {
                qualify_type(ty, names);
            }
        }
        Type::Function(params, ret) => {
            for ty in params {
                qualify_type(ty, names);
            }
            qualify_type(ret, names);
        }
    }
}
fn qualify_item(
    item: &mut Item,
    names: &Names,
    aliases: &HashMap<String, Names>,
) -> Result<(), Diagnostics> {
    match item {
        Item::Function(f) => {
            f.name = names[&f.name].clone();
            let mut local = names.clone();
            for generic in &f.generics {
                local.remove(generic);
            }
            for p in &mut f.params {
                qualify_type(&mut p.ty, names);
                local.remove(&p.name);
            }
            qualify_type(&mut f.return_type, names);
            block(&mut f.body, &local, aliases)?;
        }
        Item::Global(v) => {
            qualify_type(&mut v.ty, names);
            expr(&mut v.value, names, aliases)?;
            if let Pattern::Name(n) = &mut v.pattern {
                if let Some(name) = names.get(n) {
                    *n = name.clone();
                }
            }
        }
        Item::Statement(s) => {
            statement(s, &mut names.clone(), aliases)?;
        }
        Item::Test { body, .. } => block(body, names, aliases)?,
        Item::Struct(s) => {
            s.name = names[&s.name].clone();
            for f in &mut s.fields {
                qualify_type(&mut f.ty, names);
            }
        }
        Item::Enum(e) => {
            e.name = names[&e.name].clone();
            for v in &mut e.variants {
                for ty in &mut v.values {
                    qualify_type(ty, names);
                }
            }
        }
        Item::TypeAlias { name, ty, .. } => {
            *name = names[name].clone();
            qualify_type(ty, names);
        }
        Item::Extern { functions, .. } => {
            for f in functions {
                for p in &mut f.params {
                    qualify_type(&mut p.ty, names);
                }
                qualify_type(&mut f.return_type, names);
            }
        }
        _ => {}
    }
    Ok(())
}
fn hide(pattern: &Pattern, names: &mut Names) {
    match pattern {
        Pattern::Name(n) => {
            names.remove(n);
        }
        Pattern::Tuple(p) => {
            for p in p {
                hide(p, names);
            }
        }
        _ => {}
    }
}
fn block(
    b: &mut Block,
    names: &Names,
    aliases: &HashMap<String, Names>,
) -> Result<(), Diagnostics> {
    let mut names = names.clone();
    for s in &mut b.statements {
        statement(s, &mut names, aliases)?;
    }
    Ok(())
}
fn statement(
    s: &mut Stmt,
    names: &mut Names,
    aliases: &HashMap<String, Names>,
) -> Result<(), Diagnostics> {
    match s {
        Stmt::Var(v) => {
            qualify_type(&mut v.ty, names);
            expr(&mut v.value, names, aliases)?;
            hide(&v.pattern, names);
        }
        Stmt::Block(b) => block(b, names, aliases)?,
        Stmt::Expr(e) | Stmt::Assert(e) | Stmt::Throw(e) => expr(e, names, aliases)?,
        Stmt::Return(e) | Stmt::Break(e, _) => {
            if let Some(e) = e {
                expr(e, names, aliases)?;
            }
        }
        Stmt::Assign { target, value } => {
            expr(target, names, aliases)?;
            expr(value, names, aliases)?;
        }
        Stmt::For {
            name,
            iterable,
            body,
            ..
        } => {
            expr(iterable, names, aliases)?;
            let mut local = names.clone();
            local.remove(name);
            block(body, &local, aliases)?;
        }
        Stmt::While {
            condition, body, ..
        } => {
            expr(condition, names, aliases)?;
            block(body, names, aliases)?;
        }
        Stmt::Lock { name, body, .. } => {
            if let Some(n) = names.get(name) {
                *name = n.clone();
            }
            block(body, names, aliases)?;
        }
        Stmt::Continue(_) => {}
    }
    Ok(())
}
fn expr(e: &mut Expr, names: &Names, aliases: &HashMap<String, Names>) -> Result<(), Diagnostics> {
    match e {
        Expr::Lambda(f) => {
            let mut local = names.clone();
            for p in &mut f.params {
                qualify_type(&mut p.ty, names);
                local.remove(&p.name);
            }
            qualify_type(&mut f.return_type, names);
            block(&mut f.body, &local, aliases)?;
        }
        Expr::Name(n) => {
            if let Some(name) = names.get(n) {
                *n = name.clone();
            }
        }
        Expr::Member { object, name } => {
            if let Expr::Name(alias) = &**object {
                if let Some(exports) = aliases.get(alias) {
                    *e = Expr::Name(exports.get(name).cloned().ok_or_else(|| {
                        Diagnostics::one(format!("module `{alias}` does not export `{name}`"), 0..0)
                    })?);
                    return Ok(());
                }
            }
            expr(object, names, aliases)?;
        }
        Expr::Cast { ty, value } => {
            qualify_type(ty, names);
            expr(value, names, aliases)?;
        }
        Expr::Unary { value, .. } | Expr::Async(value) | Expr::Await(value) | Expr::Try(value) => {
            expr(value, names, aliases)?
        }
        Expr::Binary { left, right, .. } => {
            expr(left, names, aliases)?;
            expr(right, names, aliases)?;
        }
        Expr::Call {
            callee,
            args,
            generics,
        } => {
            expr(callee, names, aliases)?;
            for e in args {
                expr(e, names, aliases)?;
            }
            for ty in generics {
                qualify_type(ty, names);
            }
        }
        Expr::Index { object, index } => {
            expr(object, names, aliases)?;
            expr(index, names, aliases)?;
        }
        Expr::Array(values) | Expr::Tuple(values) => {
            for e in values {
                expr(e, names, aliases)?;
            }
        }
        Expr::Map(entries) => {
            for (k, v) in entries {
                expr(k, names, aliases)?;
                expr(v, names, aliases)?;
            }
        }
        Expr::StructInit { name, fields } => {
            if let Some(n) = names.get(name) {
                *name = n.clone();
            }
            for (_, e) in fields {
                expr(e, names, aliases)?;
            }
        }
        Expr::If { subject, arms } => {
            if let Some(e) = subject {
                expr(e, names, aliases)?;
            }
            for (patterns, b) in arms {
                for p in patterns {
                    if let Pattern::Literal(e) = p {
                        expr(e, names, aliases)?;
                    }
                }
                block(b, names, aliases)?;
            }
        }
        Expr::Else { value, fallback } => {
            expr(value, names, aliases)?;
            block(fallback, names, aliases)?;
        }
        Expr::Catch { value, name, body } => {
            expr(value, names, aliases)?;
            let mut local = names.clone();
            local.remove(name);
            block(body, &local, aliases)?;
        }
        _ => {}
    }
    Ok(())
}
