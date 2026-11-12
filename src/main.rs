mod ast;
mod cgen;
mod check;
mod dce;
mod diag;
mod driver;
mod eval;
mod fmt;
mod lexer;
mod lint;
mod lsp;
mod modules;
mod parser;
mod shake;
mod tir;
mod types;

use std::process::ExitCode;

const HELP: &str = "Usage: ncc [init|build|run|check|test|lsp|fmt]

Commands:
  init        Initialises a project.
  build       Compiles the given file.
  run         Executes the given file.
  check       Type-checks the given file.
  test        Run tests in the given file.
  lsp         Starts the language server.
  fmt         Formats the given file.

Options:
  -h, --help  Show this help and exit.";

const HELP_FMT: &str = "Usage: ncc fmt <file>

Formats the given file.

The file is edited in place.

Arguments:
  <file>              The file to act upon.

Options:
  --check             Checks if the file is formatted. Exits with code 1 if not
                      formatted.
  -o, --output [out]  Write the formatted source to the out file. Writes to
                      stdout if out is - or omitted.
  -h, --help          Show this help and exit.";

const HELP_LSP: &str = "Usage: ncc lsp

Starts the language server.

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

const HELP_RUN: &str = "Usage: ncc run <file> [-- <args>...]

Executes the given file.

This is a shorthand for building the file to an executable, running it, then
deleting the executable.

Arguments:
  <file>                    The file to act upon.
  <args>...                 Arguments passed to the program (see `@args()`).

Options:
  -r, --release             Run the file in release mode.
  -h, --help                Show this help and exit.";

const HELP_CHECK: &str = "Usage: ncc check <file>

Type-checks the given file.

Arguments:
  <file>      The file to act upon.

Options:
  -h, --help  Show this help and exit.";

const HELP_TEST: &str = "Usage: ncc test <file> [-- <args>...]

Run tests in the given file.

Arguments:
  <file>      The file to act upon.
  <args>...   Arguments passed to the test program (see `@args()`).

Options:
  -h, --help  Show this help and exit.";

const HELP_INIT: &str = "Usage: ncc init

Initialises a project.

Options:
  --name <name>  Initialises the project with the given name.
                 (default: directory name)
  -h, --help     Show this help and exit.";

fn show_help(text: &str) -> ExitCode {
    println!("{text}");
    ExitCode::SUCCESS
}

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
        "-h" | "--help" => show_help(HELP),
        "fmt" => {
            let mut file: Option<String> = None;
            let mut check = false;
            // None: edit in place; Some(None): stdout; Some(Some(path)): that file
            let mut output: Option<Option<String>> = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "-h" | "--help" => return show_help(HELP_FMT),
                    "--check" => check = true,
                    "-o" | "--output" => {
                        // the value is optional: take the next argument only when it is not the
                        // input file (the file is already known, or another argument follows)
                        let next = rest.get(i + 1).map(String::as_str);
                        let takes = match next {
                            Some("-") => true,
                            Some(n) if n.starts_with('-') => false,
                            Some(_) => {
                                file.is_some()
                                    || rest.get(i + 2).is_some_and(|s| !s.starts_with('-'))
                            }
                            None => false,
                        };
                        if takes {
                            let v = rest[i + 1].clone();
                            output = Some(if v == "-" { None } else { Some(v) });
                            i += 1;
                        } else {
                            output = Some(None);
                        }
                    }
                    a if a.starts_with('-') && a.len() > 1 => {
                        return usage_error(&format!("unknown option `{a}`"), HELP_FMT);
                    }
                    a => {
                        if file.is_some() {
                            return usage_error(&format!("unexpected argument `{a}`"), HELP_FMT);
                        }
                        file = Some(a.to_string());
                    }
                }
                i += 1;
            }
            let Some(file) = file else {
                return usage_error("missing <file>", HELP_FMT);
            };
            driver::fmt(std::path::Path::new(&file), check, output)
        }
        "lsp" => match rest.first().map(String::as_str) {
            Some("-h" | "--help") => show_help(HELP_LSP),
            Some(a) => usage_error(&format!("unexpected argument `{a}`"), HELP_LSP),
            None => lsp::run(),
        },
        "init" => {
            let mut name = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-h" | "--help" => return show_help(HELP_INIT),
                    "--name" => match it.next() {
                        Some(n) => name = Some(n.clone()),
                        None => return usage_error("`--name` requires a value", HELP_INIT),
                    },
                    a => return usage_error(&format!("unexpected argument `{a}`"), HELP_INIT),
                }
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
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let a = a.as_str();
                match a {
                    "-h" | "--help" => return show_help(help),
                    // everything after `--` goes to the program, options included
                    "--" if cmd == "run" || cmd == "test" => {
                        opts.args = it.cloned().collect();
                        break;
                    }
                    "-r" | "--release" if cmd == "build" || cmd == "run" => opts.release = true,
                    "-f" | "--format" if cmd == "build" => {
                        let Some(f) = it.next() else {
                            return usage_error("`--format` requires a value", help);
                        };
                        opts.format = Some(match f.as_str() {
                            "c" => driver::Format::C,
                            "asm" => driver::Format::Asm,
                            "exe" => driver::Format::Exe,
                            _ => {
                                return usage_error(
                                    &format!("unknown format `{f}`; expected c, asm or exe"),
                                    help,
                                );
                            }
                        });
                    }
                    "-o" | "--output" if cmd == "build" => {
                        let Some(o) = it.next() else {
                            return usage_error("`--output` requires a value", help);
                        };
                        opts.output = Some(o.into());
                    }
                    _ if a.starts_with('-') && a.len() > 1 => {
                        return usage_error(&format!("unknown option `{a}`"), help);
                    }
                    _ => {
                        if opts.file.is_some() {
                            return usage_error(&format!("unexpected argument `{a}`"), help);
                        }
                        opts.file = Some(a.into());
                    }
                }
            }
            if opts.file.is_none() {
                return usage_error("missing <file>", help);
            }
            match cmd.as_str() {
                "build" => driver::build(&opts),
                "run" => driver::run(&opts, cgen::Mode::Run),
                "check" => driver::check(&opts),
                _ => driver::run(&opts, cgen::Mode::Test),
            }
        }
        _ => usage_error(&format!("unknown command `{cmd}`"), HELP),
    }
}
