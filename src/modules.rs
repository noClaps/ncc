//! Resolve source modules once, enforce exports, and qualify their symbols.
use crate::{ast::*, diagnostic::Diagnostics, lexer, parser};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
type Names = HashMap<String, String>;

pub fn load(module: Module, path: &Path) -> Result<Module, Diagnostics> {
    load_with_sources(module, path, &HashMap::new())
}

/// Unsaved editor buffers take precedence over files, including new files.
pub fn load_with_sources(
    module: Module,
    path: &Path,
    sources: &HashMap<PathBuf, String>,
) -> Result<Module, Diagnostics> {
    let mut loader = Loader {
        done: HashMap::new(),
        active: HashSet::new(),
        items: vec![],
        next: 0,
        sources,
    };
    loader.visit(module, path, true)?;
    Ok(Module {
        items: loader.items,
    })
}
struct Loader<'a> {
    done: HashMap<PathBuf, Names>,
    active: HashSet<PathBuf>,
    items: Vec<Item>,
    next: usize,
    sources: &'a HashMap<PathBuf, String>,
}
impl Loader<'_> {
    fn visit(&mut self, mut module: Module, path: &Path, root: bool) -> Result<Names, Diagnostics> {
        let key = source_key(path);
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
            for (name, public) in symbols(item) {
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
                    let source = read_source(&imported_path, self.sources).map_err(|e| {
                        Diagnostics::one(
                            format!("cannot import {}: {e}", imported_path.display()),
                            0..0,
                        )
                    })?;
                    let imported_module = lexer::lex(&source)
                        .and_then(|tokens| parser::parse_at(tokens, &imported_path))
                        .map_err(|error| error.at_source(&imported_path, 0..0))?;
                    let exports = self.visit(imported_module, &imported_path, false)?;
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
        for (alias, exports) in &aliases {
            for (name, qualified) in exports {
                names.insert(format!("{alias}.{name}"), qualified.clone());
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
pub fn source_key(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
pub fn read_source(path: &Path, sources: &HashMap<PathBuf, String>) -> std::io::Result<String> {
    sources
        .get(&source_key(path))
        .cloned()
        .map_or_else(|| std::fs::read_to_string(path), Ok)
}
fn symbols(item: &Item) -> Vec<(&str, bool)> {
    match item {
        Item::Function(f) => vec![(&f.name, f.public)],
        Item::Struct(s) => vec![(&s.name, s.public)],
        Item::Enum(e) => vec![(&e.name, e.public)],
        Item::TypeAlias { name, public, .. } => vec![(name, *public)],
        Item::Global(v) => v
            .binding_names()
            .into_iter()
            .map(|name| (name, v.public))
            .collect(),
        _ => vec![],
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
                qualify_type(&mut p.ty, &local);
                local.insert(p.name.clone(), p.name.clone());
            }
            qualify_type(&mut f.return_type, &local);
            block(&mut f.body, &local, aliases)?;
        }
        Item::Global(v) => {
            qualify_type(&mut v.ty, names);
            expr(&mut v.value, names, aliases)?;
            qualify_binding(&mut v.pattern, names);
        }
        Item::Statement(s) => {
            statement(s, &mut names.clone(), aliases)?;
        }
        Item::Test { body, .. } => block(body, names, aliases)?,
        Item::Struct(s) => {
            s.name = names[&s.name].clone();
            let mut local = names.clone();
            for n in &s.generics {
                local.remove(n);
            }
            for f in &mut s.fields {
                qualify_type(&mut f.ty, &local);
            }
        }
        Item::Enum(e) => {
            e.name = names[&e.name].clone();
            let mut local = names.clone();
            for n in &e.generics {
                local.remove(n);
            }
            for v in &mut e.variants {
                for ty in &mut v.values {
                    qualify_type(ty, &local);
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
fn qualify_binding(pattern: &mut Pattern, names: &Names) {
    match pattern {
        Pattern::Name(name) => {
            if let Some(qualified) = names.get(name) {
                *name = qualified.clone();
            }
        }
        Pattern::Tuple(patterns) => {
            for pattern in patterns {
                qualify_binding(pattern, names);
            }
        }
        _ => {}
    }
}
fn hide(pattern: &Pattern, names: &mut Names) {
    match pattern {
        Pattern::Name(n) => {
            names.insert(n.clone(), n.clone());
        }
        Pattern::Tuple(p) | Pattern::Array(p) | Pattern::Variant { values: p, .. } => {
            for p in p {
                hide(p, names);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, p) in fields {
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
        Stmt::Expr(e) | Stmt::Assert(e) | Stmt::Throw(e) | Stmt::LabeledIf { value: e, .. } => {
            expr(e, names, aliases)?
        }
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
            local.insert(name.clone(), name.clone());
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
                local.insert(p.name.clone(), p.name.clone());
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
            if let Expr::Name(alias) = &**object
                && !names.contains_key(alias)
                && let Some(exports) = aliases.get(alias)
            {
                *e = Expr::Name(exports.get(name).cloned().ok_or_else(|| {
                    Diagnostics::one(format!("module `{alias}` does not export `{name}`"), 0..0)
                })?);
                return Ok(());
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
                let mut local = names.clone();
                for p in patterns {
                    qualify_pattern(p, names, aliases)?;
                    hide(p, &mut local);
                }
                block(b, &local, aliases)?;
            }
        }
        Expr::Else { value, fallback } => {
            expr(value, names, aliases)?;
            block(fallback, names, aliases)?;
        }
        Expr::Catch { value, name, body } => {
            expr(value, names, aliases)?;
            let mut local = names.clone();
            local.insert(name.clone(), name.clone());
            block(body, &local, aliases)?;
        }
        _ => {}
    }
    Ok(())
}
fn qualify_pattern(
    p: &mut Pattern,
    names: &Names,
    aliases: &HashMap<String, Names>,
) -> Result<(), Diagnostics> {
    match p {
        Pattern::Name(n) => {
            if let Some(qualified) = names.get(n) {
                *p = Pattern::Literal(Box::new(Expr::Name(qualified.clone())));
            }
        }
        Pattern::Literal(e) => expr(e, names, aliases)?,
        Pattern::Tuple(ps) | Pattern::Array(ps) => {
            for p in ps {
                qualify_pattern(p, names, aliases)?;
            }
        }
        Pattern::Variant { name, values } => {
            if let Some((owner, variant)) = name.rsplit_once('.')
                && let Some(qualified) = names.get(owner)
            {
                *name = format!("{qualified}.{variant}");
            }
            for p in values {
                qualify_pattern(p, names, aliases)?;
            }
        }
        Pattern::Struct { name, fields } => {
            if let Some(qualified) = names.get(name) {
                *name = qualified.clone();
            }
            for (_, p) in fields {
                qualify_pattern(p, names, aliases)?;
            }
        }
        Pattern::Wildcard => {}
    }
    Ok(())
}
