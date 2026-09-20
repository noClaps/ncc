//! Non-fatal diagnostics, separate from mandatory type checking.
use crate::{ast::Expr, diagnostic::Diagnostics, lexer, modules, parser, sema};
use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Warning {
    pub path: PathBuf,
    pub span: Range<usize>,
    pub code: &'static str,
    pub message: String,
}

pub fn check(source: &str, path: &Path) -> Result<Vec<Warning>, Diagnostics> {
    let module = modules::load(parser::parse_at(lexer::lex(source)?, path)?, path)?;
    let checked = sema::check(crate::generics::specialize(module)?, path)?;
    let mut warnings = vec![];
    let mut seen = HashSet::new();
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| {
            let Expr::Lambda(f) = e else {
                return;
            };
            let Some(captures) = checked.captures.get(&(e as *const Expr as usize)) else {
                return;
            };
            if captures.is_empty() || !seen.insert((f.source_path.clone(), f.span.start)) {
                return;
            }
            let text = if f.source_path == path {
                source.into()
            } else {
                std::fs::read_to_string(&f.source_path).unwrap_or_default()
            };
            if disabled(&text, "capture") {
                return;
            }
            warnings.push(Warning {
                path: f.source_path.clone(), span: f.span.clone(), code: "capture",
                message: format!("function captures {}; prefer explicit function parameters instead of capturing surrounding values", captures.iter().map(|(n,_,_)| format!("`{n}`")).collect::<Vec<_>>().join(", ")),
            });
        });
    }
    warnings.sort_by(|a, b| (&a.path, a.span.start).cmp(&(&b.path, b.span.start)));
    Ok(warnings)
}
fn disabled(source: &str, lint: &str) -> bool {
    let Ok(tokens) = lexer::lex(source) else {
        return false;
    };
    let mut start = 0;
    for token in tokens {
        for line in source[start..token.span.start].lines() {
            if let Some(names) = line.trim().strip_prefix("// @ncc lint disable ") {
                if names.split([',', ' ', '[', ']']).any(|name| name == lint) {
                    return true;
                }
            }
        }
        start = token.span.end;
    }
    false
}
