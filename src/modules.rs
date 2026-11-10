//! Loading the module graph from the entry file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ast::{Item, Module};
use crate::diag::{Diag, Result, Results, SourceMap, Span};
use crate::parser::parse_module_recover;

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

pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".nc").join("config.toml").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// The file an import path names, from the importing file's directory: `./x` and `../x` are
/// relative, `.../x` is from the project root and anything else is a dependency. `None` when
/// the path needs the project root (given lazily by `root`) and there is none.
pub fn import_file(
    dir: &Path,
    path: &str,
    root: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    Some(if let Some(rel) = path.strip_prefix(".../") {
        root()?.join(format!("{rel}.nc"))
    } else if path.starts_with("./") || path.starts_with("../") {
        dir.join(format!("{}.nc", path.strip_prefix("./").unwrap_or(path)))
    } else {
        root()?.join(".nc").join("deps").join(format!("{path}.nc"))
    })
}

/// Load the module graph, preferring in-memory contents from `overlay` (keyed by canonical path).
/// Every parse and import error across all modules is reported.
/// Returns the graph of everything that could be loaded together with those errors; fails
/// outright only when the entry file itself cannot be read.
pub fn load_with(
    sm: &mut SourceMap,
    entry: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> Results<(ModuleGraph, Vec<Diag>)> {
    let entry = std::fs::canonicalize(entry).map_err(|e| {
        vec![Diag::new(
            Span::default(),
            format!("cannot open `{}`: {e}", entry.display()),
        )]
    })?;
    let entry_dir = entry.parent().unwrap().to_path_buf();
    let mut loader = Loader {
        sm,
        entry_dir,
        graph: ModuleGraph {
            modules: vec![],
            order: vec![],
        },
        by_path: HashMap::new(),
        root: None,
        overlay,
        errors: Vec::new(),
    };
    loader.load_one(&entry, None).map_err(|d| vec![d])?;
    let Loader {
        mut graph, errors, ..
    } = loader;
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
    Ok((graph, errors))
}

/// State shared while loading the module graph.
struct Loader<'a> {
    sm: &'a mut SourceMap,
    entry_dir: PathBuf,
    graph: ModuleGraph,
    by_path: HashMap<PathBuf, usize>,
    /// project root (directory holding `.nc/config.toml`), looked up on first use
    root: Option<Option<PathBuf>>,
    overlay: &'a HashMap<PathBuf, String>,
    errors: Vec<Diag>,
}

impl Loader<'_> {
    fn load_one(&mut self, path: &Path, from: Option<Span>) -> Result<usize> {
        if let Some(&i) = self.by_path.get(path) {
            return Ok(i);
        }
        let text = match self.overlay.get(path) {
            Some(t) => Ok(t.clone()),
            None => std::fs::read_to_string(path),
        }
        .map_err(|e| {
            Diag::new(
                from.unwrap_or_default(),
                format!("cannot read module `{}`: {e}", path.display()),
            )
        })?;
        let file = self.sm.add(path.to_path_buf(), text.clone());
        let (ast, parse_errors) = parse_module_recover(&text, file);
        self.errors.extend(parse_errors);
        let idx = self.graph.modules.len();
        self.by_path.insert(path.to_path_buf(), idx);
        self.graph.modules.push(LoadedModule {
            path: path.to_path_buf(),
            file,
            ast,
            imports: HashMap::new(),
        });
        let imports: Vec<_> = self.graph.modules[idx]
            .ast
            .items
            .iter()
            .filter_map(|it| {
                if let Item::Import(im) = it {
                    Some(im.clone())
                } else {
                    None
                }
            })
            .collect();
        let dir = path.parent().unwrap().to_path_buf();
        for im in imports {
            let root = &mut self.root;
            let entry_dir = &self.entry_dir;
            let target = import_file(&dir, &im.path, || {
                root.get_or_insert_with(|| find_root(entry_dir)).clone()
            });
            let Some(target) = target else {
                self.errors.push(Diag::new(
                    im.span,
                    format!(
                        "cannot resolve import `{}`: no `.nc/config.toml` found in `{}` or any parent directory",
                        im.path,
                        self.entry_dir.display()
                    ),
                ));
                continue;
            };
            let Ok(target) = std::fs::canonicalize(&target) else {
                self.errors.push(Diag::new(
                    im.span,
                    format!(
                        "cannot find module `{}` (looked for `{}`)",
                        im.path,
                        target.display()
                    ),
                ));
                continue;
            };
            if self.graph.modules[idx].imports.contains_key(&im.alias) {
                self.errors.push(Diag::new(
                    im.span,
                    format!("duplicate import alias `{}`", im.alias),
                ));
                continue;
            }
            match self.load_one(&target, Some(im.span)) {
                Ok(m) => {
                    self.graph.modules[idx].imports.insert(im.alias.clone(), m);
                }
                Err(d) => self.errors.push(d),
            }
        }
        Ok(idx)
    }
}
