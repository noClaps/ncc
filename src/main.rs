#![allow(dead_code)]
mod ast;
mod diag;
mod lexer;
mod parser;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "parse" {
        let src = std::fs::read_to_string(&args[2]).unwrap();
        let mut sm = diag::SourceMap::default();
        let id = sm.add(args[2].clone().into(), src.clone());
        match parser::parse_module(&src, id) {
            Ok(m) => println!("ok: {} items", m.items.len()),
            Err(d) => { eprintln!("{}", d.render(&sm)); std::process::exit(1) }
        }
    }
}
