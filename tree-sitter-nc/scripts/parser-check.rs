//! Syntax-only oracle for the optional Tree-sitter/compiler parity checks.
//! Does not load imports, type-check, generate C, or execute NC programs.
use std::io::{self, BufRead};
use std::path::Path;

fn main() {
    for path in io::stdin().lock().lines() {
        let path = path.expect("read fixture path");
        let source = std::fs::read_to_string(&path).expect("read syntax fixture");
        let accepted = ncc::lexer::lex(&source)
            .and_then(|tokens| ncc::parser::parse_at(tokens, Path::new(&path)))
            .is_ok();
        println!("{}", if accepted { "ok" } else { "error" });
    }
}
