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

impl Workspace {
    pub fn change_folders(&mut self, params: &Value) {
        for folder in params["event"]["removed"].as_array().into_iter().flatten() {
            if let Some(uri) = folder["uri"].as_str() {
                let path = document_path(uri);
                self.roots.retain(|root| root != &path);
            }
        }
        for folder in params["event"]["added"].as_array().into_iter().flatten() {
            if let Some(uri) = folder["uri"].as_str() {
                let path = document_path(uri);
                if !self.roots.contains(&path) {
                    self.roots.push(path);
                }
            }
        }
    }
    fn exported_target(
        &self,
        uri: &str,
        at: usize,
        documents: &HashMap<String, Document>,
    ) -> Option<(String, String, usize, String)> {
        let index = Index::new(&documents.get(uri)?.text);
        let (path, name) = if let Some((path, name)) = index.imported_member(at) {
            (
                document_path(uri).parent()?.join(path).with_extension("nc"),
                name,
            )
        } else {
            (document_path(uri), index.exported_at(at)?)
        };
        let path = crate::modules::source_key(&path);
        if !self
            .roots
            .iter()
            .any(|root| path.starts_with(crate::modules::source_key(root)))
        {
            return None;
        }
        let (uri, text) = documents
            .iter()
            .find(|(uri, _)| crate::modules::source_key(&document_path(uri)) == path)
            .map(|(uri, doc)| (uri.clone(), doc.text.clone()))
            .or_else(|| {
                std::fs::read_to_string(&path)
                    .ok()
                    .map(|text| (file_uri(&path), text))
            })?;
        let position = Index::new(&text).exported_position(&name)?;
        Some((uri, text, position, name))
    }
    pub fn prepare_rename(&self, uri: &str, at: usize, open: &HashMap<String, Document>) -> Value {
        let Some(document) = open.get(uri) else {
            return Value::Null;
        };
        let index = Index::new(&document.text);
        let local = index.prepare_rename(at);
        if !local.is_null() {
            return local;
        }
        let documents = self.documents(open);
        self.exported_target(uri, at, &documents)
            .map_or(Value::Null, |(_, _, _, name)| index.rename_range(at, &name))
    }
    pub fn rename(
        &self,
        uri: &str,
        at: usize,
        name: &str,
        open: &HashMap<String, Document>,
    ) -> Value {
        if !super::index::valid_name(name) {
            return Value::Null;
        }
        let Some(document) = open.get(uri) else {
            return Value::Null;
        };
        let index = Index::new(&document.text);
        if !index.prepare_rename(at).is_null() {
            return index.rename(uri, at, name);
        }
        let documents = self.documents(open);
        let Some((_, target, position, _)) = self.exported_target(uri, at, &documents) else {
            return Value::Null;
        };
        if !Index::new(&target).can_rename_export(position, name) {
            return Value::Null;
        }
        let references = self.references(uri, at, true, &documents);
        let mut edits = serde_json::Map::<String, Value>::new();
        for reference in references.as_array().unwrap() {
            let uri = reference["uri"].as_str().unwrap();
            edits
                .entry(uri)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!({"range":reference["range"],"newText":name}));
        }
        // Verify the whole edit against a single in-memory snapshot. This catches
        // missed type references, shadowing and cross-module name collisions.
        let mut sources = documents
            .iter()
            .map(|(uri, doc)| {
                (
                    crate::modules::source_key(&document_path(uri)),
                    doc.text.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        for (uri, changes) in &edits {
            let path = crate::modules::source_key(&document_path(uri));
            let Some(text) = sources.get_mut(&path) else {
                return Value::Null;
            };
            let mut ranges = changes
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|edit| {
                    Some((
                        super::offset(text, &edit["range"]["start"])?,
                        super::offset(text, &edit["range"]["end"])?,
                    ))
                })
                .collect::<Vec<_>>();
            if ranges.len() != changes.as_array().unwrap().len() {
                return Value::Null;
            }
            ranges.sort_unstable();
            for (start, end) in ranges.into_iter().rev() {
                text.replace_range(start..end, name);
            }
        }
        for uri in edits.keys() {
            let path = crate::modules::source_key(&document_path(uri));
            if crate::lint::check_with_sources(&sources[&path], &path, &sources).is_err() {
                return Value::Null;
            }
        }
        json!({"changes":edits})
    }
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
        std::fs::write(root.join("other.nc"), "import { \"dep\" as dep }\n@println(dep.value())\nstruct Holder { int value }\n{ Holder dep = Holder{.value = 3}\n@println(dep.value) }\n").unwrap();
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
        assert_eq!(
            workspace.prepare_rename(&uri, at, &open)["placeholder"],
            "value"
        );
        let rename = workspace.rename(&uri, at, "answer", &open);
        assert_eq!(rename["changes"].as_object().unwrap().len(), 3, "{rename}");
        assert_eq!(
            rename["changes"][file_uri(&root.join("other.nc"))]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(workspace.rename(&uri, at, "int", &open).is_null());
        assert_eq!(
            std::fs::read_to_string(root.join("dep.nc")).unwrap(),
            "pub fn value() int { return 1 }\n@println(value())"
        );
        open.get_mut(&dependency)
            .unwrap()
            .text
            .push_str("\nfn answer() int { return 0 }");
        assert!(workspace.rename(&uri, at, "answer", &open).is_null());
    }
}
