#![allow(dead_code)]
mod ast;
mod check;
mod diag;
mod lexer;
mod modules;
mod parser;
mod tir;
mod types;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "check" {
        let mut sm = diag::SourceMap::default();
        let r = modules::load(&mut sm, std::path::Path::new(&args[2])).and_then(check::check);
        match r {
            Ok(p) => println!("ok: {} funcs", p.funcs.len()),
            Err(d) => { eprintln!("{}", d.render(&sm)); std::process::exit(1) }
        }
    }
}
