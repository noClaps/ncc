//! NC compiler front end and C backend.
pub mod ast;
#[path = "c_backend.rs"]
pub mod codegen;
pub mod diagnostic;
pub mod formatter;
pub mod generics;
pub mod lexer;
pub mod lsp;
pub mod modules;
pub mod parser;
pub mod sema;

use std::path::Path;

use diagnostic::Diagnostics;

/// Parse, type-check, and compile one NC source module to C99.
pub fn compile_source(source: &str, path: &Path) -> Result<String, Diagnostics> {
    let tokens = lexer::lex(source)?;
    let module = modules::load(parser::parse(tokens)?, path)?;
    let checked = sema::check(generics::specialize(module)?, path)?;
    codegen::emit(&checked)
}

/// Parse and type-check one NC source module.
pub fn check_source(source: &str, path: &Path) -> Result<(), Diagnostics> {
    let tokens = lexer::lex(source)?;
    let module = modules::load(parser::parse(tokens)?, path)?;
    sema::check(generics::specialize(module)?, path).map(|_| ())
}
