//! NC compiler front end and C backend.
pub mod ast;
#[path = "c_backend.rs"]
pub mod codegen;
pub mod diagnostic;
mod flow;
pub mod generics;
pub mod lexer;
pub mod modules;
pub mod optimizer;
pub mod parser;
pub mod sema;
pub mod target;
pub mod temp;
mod test_slice;
pub mod unicode;
mod unicode_data;
mod visit;
mod warnings;

use std::path::Path;

use diagnostic::Diagnostics;

/// Parse, type-check, and compile one NC source module, ignoring test blocks.
pub fn compile_source(source: &str, path: &Path) -> Result<String, Diagnostics> {
    compile_source_with_options(source, path, false)
}
pub fn compile_source_with_options(
    source: &str,
    path: &Path,
    release: bool,
) -> Result<String, Diagnostics> {
    compile_source_with_diagnostics(source, path, release).map(|output| output.c)
}

pub struct CompileOutput {
    pub c: String,
    pub warnings: Diagnostics,
}

/// Compile with non-fatal diagnostics, without writing to either output stream.
pub fn compile_source_with_diagnostics(
    source: &str,
    path: &Path,
    release: bool,
) -> Result<CompileOutput, Diagnostics> {
    compile(source, path, release, false)
}

/// Compile test blocks and their transitive dependencies, including prior mutations.
/// Unrelated top-level execution is omitted; selected items retain source order.
pub fn compile_test_source(source: &str, path: &Path) -> Result<String, Diagnostics> {
    compile_test_source_with_options(source, path, false)
}

pub fn compile_test_source_with_options(
    source: &str,
    path: &Path,
    release: bool,
) -> Result<String, Diagnostics> {
    compile_test_source_with_diagnostics(source, path, release).map(|output| output.c)
}

/// Compile including tests and return non-fatal diagnostics without printing them.
pub fn compile_test_source_with_diagnostics(
    source: &str,
    path: &Path,
    release: bool,
) -> Result<CompileOutput, Diagnostics> {
    compile(source, path, release, true)
}

fn compile(
    source: &str,
    path: &Path,
    release: bool,
    tests: bool,
) -> Result<CompileOutput, Diagnostics> {
    let tokens = lexer::lex(source)?;
    let module = modules::load_with_tests(parser::parse_at(tokens, path)?, path, tests)?;
    let checked = sema::check(generics::specialize(module)?, path)?;
    let warnings = warnings::data_races(&checked);
    let checked = optimizer::resolve_embeds(checked, path)?;
    if release {
        let checked = sema::check(optimizer::optimize(checked)?, path)?;
        return Ok(CompileOutput {
            c: codegen::emit(&checked)?,
            warnings,
        });
    }
    Ok(CompileOutput {
        c: codegen::emit(&checked)?,
        warnings,
    })
}
