//! Loading the module graph from the entry file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ast::{Item, Module};
use crate::diag::{Diag, Result, SourceMap, Span};
use crate::parser::parse_module;

pub struct LoadedModule {
    pub path: PathBuf,
    pub file: u32,
    pub ast: Module,
    /// import alias -> module index
    pub imports: HashMap<String, usize>,
}

pub struct ModuleGraph {
    pub modules: Vec<LoadedModule>,
    /// Modules ordered so that imports come before importers (entry last).
    pub order: Vec<usize>,
}

fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".nc").join("config.toml").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

pub fn load(sm: &mut SourceMap, entry: &Path) -> Result<ModuleGraph> {
    let entry = std::fs::canonicalize(entry)
        .map_err(|e| Diag::new(Span::default(), format!("cannot open `{}`: {e}", entry.display())))?;
    let entry_dir = entry.parent().unwrap().to_path_buf();
    let mut graph = ModuleGraph { modules: vec![], order: vec![] };
    let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
    let mut root: Option<Option<PathBuf>> = None;
    load_one(sm, &entry, &entry_dir, &mut graph, &mut by_path, &mut root, None)?;
    // order: DFS post-order from the entry
    let mut visited = vec![false; graph.modules.len()];
    fn visit(g: &ModuleGraph, i: usize, visited: &mut Vec<bool>, out: &mut Vec<usize>) {
        if visited[i] {
            return;
        }
        visited[i] = true;
        let mut deps: Vec<(&String, &usize)> = g.modules[i].imports.iter().collect();
        // keep source order of imports
        let order_of = |alias: &str| {
            g.modules[i]
                .ast
                .items
                .iter()
                .position(|it| matches!(it, Item::Import(im) if im.alias == alias))
                .unwrap_or(0)
        };
        deps.sort_by_key(|(a, _)| order_of(a));
        for (_, &d) in deps {
            visit(g, d, visited, out);
        }
        out.push(i);
    }
    let mut order = vec![];
    visit(&graph, 0, &mut visited, &mut order);
    graph.order = order;
    Ok(graph)
}

fn load_one(
    sm: &mut SourceMap,
    path: &Path,
    entry_dir: &Path,
    graph: &mut ModuleGraph,
    by_path: &mut HashMap<PathBuf, usize>,
    root: &mut Option<Option<PathBuf>>,
    from: Option<Span>,
) -> Result<usize> {
    if let Some(&i) = by_path.get(path) {
        return Ok(i);
    }
    let text = std::fs::read_to_string(path).map_err(|e| {
        Diag::new(from.unwrap_or_default(), format!("cannot read module `{}`: {e}", path.display()))
    })?;
    let file = sm.add(path.to_path_buf(), text.clone());
    let ast = parse_module(&text, file)?;
    let idx = graph.modules.len();
    by_path.insert(path.to_path_buf(), idx);
    graph.modules.push(LoadedModule { path: path.to_path_buf(), file, ast, imports: HashMap::new() });
    let imports: Vec<_> = graph.modules[idx]
        .ast
        .items
        .iter()
        .filter_map(|it| if let Item::Import(im) = it { Some(im.clone()) } else { None })
        .collect();
    let dir = path.parent().unwrap().to_path_buf();
    for im in imports {
        let from_root = im.path.strip_prefix(".../");
        let target = if from_root.is_none() && (im.path.starts_with("./") || im.path.starts_with("../")) {
            dir.join(format!("{}.nc", im.path))
        } else {
            if root.is_none() {
                *root = Some(find_root(entry_dir));
            }
            match root.as_ref().unwrap() {
                Some(r) => match from_root {
                    Some(rel) => r.join(format!("{rel}.nc")),
                    None => r.join(".nc").join("deps").join(format!("{}.nc", im.path)),
                },
                None => {
                    return Err(Diag::new(
                        im.span,
                        format!(
                            "cannot resolve import `{}`: no `.nc/config.toml` found in `{}` or any parent directory",
                            im.path,
                            entry_dir.display()
                        ),
                    ))
                }
            }
        };
        let target = std::fs::canonicalize(&target).map_err(|_| {
            Diag::new(im.span, format!("cannot find module `{}` (looked for `{}`)", im.path, target.display()))
        })?;
        if graph.modules[idx].imports.contains_key(&im.alias) {
            return Err(Diag::new(im.span, format!("duplicate import alias `{}`", im.alias)));
        }
        let m = load_one(sm, &target, entry_dir, graph, by_path, root, Some(im.span))?;
        graph.modules[idx].imports.insert(im.alias.clone(), m);
    }
    Ok(idx)
}
