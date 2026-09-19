use ncc::{check_source, compile_source};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

fn usage() {
    eprintln!("Usage: ncc [build|check|fmt|lsp|run] <file> [options]");
}
fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        usage();
        return ExitCode::FAILURE;
    };
    if command == "-h" || command == "--help" {
        usage();
        return ExitCode::SUCCESS;
    }
    if command == "lsp" {
        eprintln!("ncc: LSP is not yet available");
        return ExitCode::FAILURE;
    }
    let Some(file) = args.next() else {
        usage();
        return ExitCode::FAILURE;
    };
    let path = PathBuf::from(file);
    let source = match fs::read_to_string(&path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("ncc: {e}");
            return ExitCode::FAILURE;
        }
    };
    match command.as_str() {
        "check" => match check_source(&source, &path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprint!("{e}");
                ExitCode::FAILURE
            }
        },
        "fmt" => match ncc::formatter::format(&source) {
            Ok(x) => match fs::write(&path, x) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("ncc: {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprint!("{e}");
                ExitCode::FAILURE
            }
        },
        "build" | "run" => build(&source, &path, args, command == "run"),
        _ => {
            usage();
            ExitCode::FAILURE
        }
    }
}
fn build(source: &str, path: &Path, args: impl Iterator<Item = String>, run: bool) -> ExitCode {
    let mut output = None;
    let mut want_c = false;
    let mut it = args.peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => output = it.next().map(PathBuf::from),
            "-f" | "--format" => want_c = it.next().is_some_and(|x| x.eq_ignore_ascii_case("c")),
            "-r" | "--release" | "-d" | "--debug" => {}
            _ => {
                eprintln!("ncc: unknown option {a}");
                return ExitCode::FAILURE;
            }
        }
    }
    let c = match compile_source(source, path) {
        Ok(c) => c,
        Err(e) => {
            eprint!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let out = output.unwrap_or_else(|| {
        if want_c {
            path.with_extension("c")
        } else {
            path.with_extension("")
        }
    });
    if want_c || out.extension().is_some_and(|x| x == "c") {
        return match fs::write(out, c) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("ncc: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let c_path = out.with_extension("c");
    if let Err(e) = fs::write(&c_path, c) {
        eprintln!("ncc: {e}");
        return ExitCode::FAILURE;
    }
    let status = match Command::new("cc").arg(&c_path).arg("-o").arg(&out).status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ncc: cannot run C compiler: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        return ExitCode::FAILURE;
    }
    if run {
        match Command::new(&out).status() {
            Ok(s) if s.success() => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("ncc: cannot run program: {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        ExitCode::SUCCESS
    }
}
