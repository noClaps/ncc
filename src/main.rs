mod ast;
mod cgen;
mod check;
mod diag;
mod driver;
mod eval;
mod lexer;
mod lint;
mod modules;
mod parser;
mod tir;
mod types;

use std::process::ExitCode;

const HELP: &str = "Usage: ncc [init|build|run|check|test]

Commands:
  init        Initialises a project.
  build       Compiles the given file.
  run         Executes the given file.
  check       Type-checks the given file.
  test        Run tests in the given file.

Options:
  -h, --help  Show this help and exit.";

const HELP_BUILD: &str = "Usage: ncc build <file>

Compiles the given file.

The output format of the file can also be detected from the extension of the
output file passed to --output.

Arguments:
  <file>                    The file to act upon.

Options:
  -r, --release             Build the file in release mode.
  -f, --format <c|asm|exe>  The format to compile to. (default: exe)
  -o, --output <out>        The file to output to. (default: filename)
  -h, --help                Show this help and exit.";

const HELP_RUN: &str = "Usage: ncc run <file>

Executes the given file.

This is a shorthand for building the file to an executable, running it, then
deleting the executable.

Arguments:
  <file>                    The file to act upon.

Options:
  -r, --release             Run the file in release mode.
  -h, --help                Show this help and exit.";

const HELP_CHECK: &str = "Usage: ncc check <file>

Type-checks the given file.

Arguments:
  <file>      The file to act upon.

Options:
  -h, --help  Show this help and exit.";

const HELP_TEST: &str = "Usage: ncc test <file>

Run tests in the given file.

Arguments:
  <file>      The file to act upon.

Options:
  -h, --help  Show this help and exit.";

const HELP_INIT: &str = "Usage: ncc init

Initialises a project.

Options:
  --name <name>  Initialises the project with the given name.
                 (default: directory name)
  -h, --help     Show this help and exit.";

fn usage_error(msg: &str, help: &str) -> ExitCode {
    eprintln!("error: {msg}\n\n{help}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        println!("{HELP}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "-h" | "--help" => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        "init" => {
            let mut name = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "-h" | "--help" => {
                        println!("{HELP_INIT}");
                        return ExitCode::SUCCESS;
                    }
                    "--name" => {
                        i += 1;
                        match rest.get(i) {
                            Some(n) => name = Some(n.clone()),
                            None => return usage_error("`--name` requires a value", HELP_INIT),
                        }
                    }
                    a => return usage_error(&format!("unexpected argument `{a}`"), HELP_INIT),
                }
                i += 1;
            }
            driver::init(name)
        }
        "build" | "run" | "check" | "test" => {
            let help = match cmd.as_str() {
                "build" => HELP_BUILD,
                "run" => HELP_RUN,
                "check" => HELP_CHECK,
                _ => HELP_TEST,
            };
            let mut opts = driver::Options::default();
            let mut i = 0;
            while i < rest.len() {
                let a = rest[i].as_str();
                match a {
                    "-h" | "--help" => {
                        println!("{help}");
                        return ExitCode::SUCCESS;
                    }
                    "-r" | "--release" if cmd == "build" || cmd == "run" => opts.release = true,
                    "-f" | "--format" if cmd == "build" => {
                        i += 1;
                        let Some(f) = rest.get(i) else {
                            return usage_error("`--format` requires a value", help);
                        };
                        opts.format = Some(match f.as_str() {
                            "c" => driver::Format::C,
                            "asm" => driver::Format::Asm,
                            "exe" => driver::Format::Exe,
                            _ => return usage_error(&format!("unknown format `{f}`; expected c, asm or exe"), help),
                        });
                    }
                    "-o" | "--output" if cmd == "build" => {
                        i += 1;
                        let Some(o) = rest.get(i) else {
                            return usage_error("`--output` requires a value", help);
                        };
                        opts.output = Some(o.into());
                    }
                    _ if a.starts_with('-') && a.len() > 1 => {
                        return usage_error(&format!("unknown option `{a}`"), help)
                    }
                    _ => {
                        if opts.file.is_some() {
                            return usage_error(&format!("unexpected argument `{a}`"), help);
                        }
                        opts.file = Some(a.into());
                    }
                }
                i += 1;
            }
            if opts.file.is_none() {
                return usage_error("missing <file>", help);
            }
            match cmd.as_str() {
                "build" => driver::build(&opts),
                "run" => driver::run(&opts),
                "check" => driver::check(&opts),
                _ => driver::test(&opts),
            }
        }
        _ => usage_error(&format!("unknown command `{cmd}`"), HELP),
    }
}
