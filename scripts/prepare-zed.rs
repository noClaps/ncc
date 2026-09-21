//! Compile with rustc and run from the repository root. No extra dependencies.
use std::{error::Error, fs, path::Path, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::current_dir()?.canonicalize()?;
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !revision.status.success() {
        return Err("run this command from the ncc repository root".into());
    }
    // Pin only committed grammar sources, so Zed builds exactly the tested tree.
    let committed = Command::new("git")
        .args(["cat-file", "-e", "HEAD:tree-sitter-nc/src/parser.c"])
        .status()?;
    if !committed.success() {
        return Err(
            "commit the generated Tree-sitter parser before preparing the extension".into(),
        );
    }
    let destination = root.join("target/zed-extension");
    copy_tree(&root.join("editors/zed"), &destination)?;
    fs::copy(
        root.join("tree-sitter-nc/queries/highlights.scm"),
        destination.join("languages/nc/highlights.scm"),
    )?;
    let repository = format!("file://{}", root.display());
    let manifest = fs::read_to_string(root.join("editors/zed/extension.toml.in"))?
        .replace("@REPOSITORY@", &format!("{repository:?}"))
        .replace(
            "@REVISION@",
            &format!("{:?}", String::from_utf8(revision.stdout)?.trim()),
        );
    fs::write(destination.join("extension.toml"), manifest)?;
    println!("Install Dev Extension in Zed: {}", destination.display());
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == "target" {
            continue;
        }
        let destination = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}
