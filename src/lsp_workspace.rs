//! Read-only workspace discovery; unsaved buffers override disk contents.
use super::{Document, document_path, file_uri, index::Index};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct Workspace {
    roots: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_include_unopened_callers_and_respect_overlays_and_shadowing() {
        let directory = crate::temp::Directory::new().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("dep.nc"),
            "pub fn value() int { return 1 }\n@println(value())",
        )
        .unwrap();
        std::fs::write(root.join("other.nc"), "import { \"dep\" as dep }\n@println(dep.value())\nstruct Holder { int value }\nHolder dep = Holder{.value = 3}\n@println(dep.value)\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::write(
            root.join("target/ignored.nc"),
            "import { \"../dep\" as dep } @println(dep.value())",
        )
        .unwrap();
        let uri = file_uri(&root.join("main.nc"));
        let source = "import { \"dep\" as dep }\n@println(dep.value())\n";
        let mut open = HashMap::from([(
            uri.clone(),
            Document {
                text: source.into(),
                version: 1,
            },
        )]);
        let mut workspace = Workspace::default();
        workspace.initialize(&json!({"rootUri":file_uri(root)}));
        let at = source.find("dep.value").unwrap() + 4;
        let refs = workspace.references(&uri, at, true, &open);
        assert_eq!(refs.as_array().unwrap().len(), 4, "{refs}");
        assert_eq!(
            workspace
                .references(&uri, at, false, &open)
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let dependency = file_uri(&root.join("dep.nc"));
        open.insert(
            dependency.clone(),
            Document {
                text: "pub fn value() int { return 2 }".into(),
                version: 2,
            },
        );
        assert_eq!(
            workspace
                .references(&uri, at, true, &open)
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            workspace
                .references(&dependency, 7, true, &open)
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(workspace.documents(&open).len(), 3);
    }
}

impl Workspace {
    pub fn initialize(&mut self, params: &Value) {
        self.roots = params["workspaceFolders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|folder| folder["uri"].as_str())
            .map(document_path)
            .collect();
        if self.roots.is_empty()
            && let Some(uri) = params["rootUri"].as_str()
        {
            self.roots.push(document_path(uri));
        }
    }
    pub fn documents(&self, open: &HashMap<String, Document>) -> HashMap<String, Document> {
        fn visit(path: &Path, result: &mut HashMap<String, Document>) {
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !name.starts_with('.') && !matches!(&*name, "target" | "node_modules") {
                        visit(&path, result);
                    }
                } else if path.extension().is_some_and(|ext| ext == "nc")
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    result.insert(file_uri(&path), Document { text, version: 0 });
                }
            }
        }
        let mut result = HashMap::new();
        for root in &self.roots {
            visit(root, &mut result);
        }
        for (uri, document) in open {
            result.remove(&file_uri(&document_path(uri)));
            result.insert(uri.clone(), document.clone());
        }
        result
    }
    pub fn references(
        &self,
        uri: &str,
        at: usize,
        declaration: bool,
        open: &HashMap<String, Document>,
    ) -> Value {
        let documents = self.documents(open);
        let Some(document) = documents.get(uri) else {
            return json!([]);
        };
        let index = Index::new(&document.text);
        let (target, name) = if let Some((path, name)) = index.imported_member(at) {
            (
                document_path(uri)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(path)
                    .with_extension("nc"),
                name,
            )
        } else if let Some(name) = index.exported_at(at) {
            (document_path(uri), name)
        } else {
            return index.references(uri, at, declaration);
        };
        let target = crate::modules::source_key(&target);
        let target_uri = file_uri(&target);
        let source = documents
            .iter()
            .find(|(uri, _)| crate::modules::source_key(&document_path(uri)) == target);
        let (target_uri, text) = if let Some((uri, doc)) = source {
            (uri.clone(), doc.text.clone())
        } else if let Ok(text) = std::fs::read_to_string(&target) {
            (target_uri, text)
        } else {
            return json!([]);
        };
        let target_index = Index::new(&text);
        let Some(position) = target_index.exported_position(&name) else {
            return json!([]);
        };
        let mut references = target_index
            .references(&target_uri, position, declaration)
            .as_array()
            .unwrap()
            .clone();
        for (uri, document) in &documents {
            let path = document_path(uri);
            for (import, location) in Index::new(&document.text).imported_references(uri, &name) {
                let imported = path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(import)
                    .with_extension("nc");
                if crate::modules::source_key(&imported) == target {
                    references.push(location);
                }
            }
        }
        references.sort_by_key(Value::to_string);
        references.dedup();
        json!(references)
    }
}
