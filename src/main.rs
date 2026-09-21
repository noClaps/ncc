use ncc::{
    compile_source_with_options,
    diagnostic::{Diagnostic, Diagnostics},
};
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    C,
    Object,
    Executable,
}
struct Options {
    command: String,
    input: PathBuf,
    output: Option<PathBuf>,
    format: Option<Format>,
    release: bool,
}

fn help(command: &str) {
    match command {
        "build" => println!(
            "Usage: ncc build <file> [options]\n\nBuild to an executable, C source, or an object file.\n\n  -o, --output <file>   Output path (extension determines format)\n  -f, --format <C|obj|exe>  Override output format\n  -r, --release         Aggressive constant evaluation and optimisation\n  -d, --debug           Debug build (default)\n  -h, --help            Show help"
        ),
        "run" => println!(
            "Usage: ncc run <file> [options]\n\nCompile and run without leaving generated files.\n\n  -r, --release  Aggressive constant evaluation and optimisation\n  -d, --debug    Debug build (default)\n  -h, --help     Show help"
        ),
        "check" => println!(
            "Usage: ncc check <file>\n\nType-check and lint the file and its imports. Warnings do not fail the check.\nDisable a lint for a file with: // @ncc lint disable capture\n\n  -h, --help  Show help"
        ),
        "fmt" => println!(
            "Usage: ncc fmt <file>\n\nFormat the file in place.\n\n  -h, --help  Show help"
        ),
        "lsp" => println!(
            "Usage: ncc lsp\n\nStart the language server over standard input/output.\n\n  -h, --help  Show help"
        ),
        _ => println!(
            "Usage: ncc <command> [options]\n\nCommands:\n  build  Build an executable, C source, or object file\n  check  Type-check and lint a file and its imports\n  fmt    Format a file\n  lsp    Start the language server\n  run    Compile and execute without leaving build files\n\n  -h, --help     Show help\n  -V, --version  Show version\n\nUse `ncc <command> --help` for command-specific options."
        ),
    }
}
fn parse(args: Vec<String>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("missing command; use `ncc --help`")?;
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        help(&args.next().unwrap_or_default());
        return Ok(None);
    }
    if matches!(command.as_str(), "-V" | "--version") {
        println!("ncc {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    if !matches!(command.as_str(), "build" | "run" | "check" | "fmt" | "lsp") {
        return Err(format!("unknown command `{command}`; use `ncc --help`"));
    }
    let mut options = Options {
        command,
        input: PathBuf::new(),
        output: None,
        format: None,
        release: false,
    };
    let mut positional = false;
    while let Some(arg) = args.next() {
        if !positional && matches!(arg.as_str(), "-h" | "--help") {
            help(&options.command);
            return Ok(None);
        }
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(a, b)| (a, Some(b.to_owned())));
            match flag {
                "-r" | "--release" | "-d" | "--debug"
                    if matches!(options.command.as_str(), "run" | "build") && inline.is_none() =>
                {
                    options.release = matches!(flag, "-r" | "--release")
                }
                "-o" | "--output" | "-f" | "--format" if options.command == "build" => {
                    let value = inline
                        .or_else(|| args.next())
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .ok_or_else(|| format!("{flag} requires a value"))?;
                    if matches!(flag, "-o" | "--output") {
                        options.output = Some(value.into());
                    } else {
                        options.format = Some(match value.to_ascii_lowercase().as_str() {
                            "c" => Format::C,
                            "obj" => Format::Object,
                            "exe" => Format::Executable,
                            _ => {
                                return Err(format!(
                                    "unsupported output format `{value}` (expected C, obj, or exe)"
                                ));
                            }
                        });
                    }
                }
                _ => return Err(format!("unknown option `{arg}` for `{}`", options.command)),
            }
        } else if options.input.as_os_str().is_empty() && options.command != "lsp" {
            options.input = arg.into();
        } else {
            return Err(format!("unexpected argument `{arg}`"));
        }
    }
    if options.command != "lsp" && options.input.as_os_str().is_empty() {
        return Err(format!(
            "missing input file; use `ncc {} --help`",
            options.command
        ));
    }
    Ok(Some(options))
}
fn main() -> ExitCode {
    let result = parse(env::args().skip(1).collect())
        .and_then(|options| options.map_or(Ok(ExitCode::SUCCESS), execute));
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ncc: {error}");
            ExitCode::FAILURE
        }
    }
}
fn execute(o: Options) -> Result<ExitCode, String> {
    if o.command == "lsp" {
        #[cfg(not(feature = "lsp"))]
        return Err("this compiler was built without language-server support".into());
        #[cfg(feature = "lsp")]
        {
            ncc::lsp::serve(std::io::stdin().lock(), std::io::stdout().lock())
                .map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    let source = fs::read_to_string(&o.input).map_err(|e| format!("{}: {e}", o.input.display()))?;
    match o.command.as_str() {
        "check" => {
            let warnings =
                ncc::lint::check(&source, &o.input).map_err(|e| e.render(&source, &o.input))?;
            for w in warnings {
                let text = if w.path == o.input {
                    source.clone()
                } else {
                    fs::read_to_string(&w.path).unwrap_or_default()
                };
                let message = Diagnostics(vec![Diagnostic {
                    message: format!("[{}] {}", w.code, w.message),
                    span: w.span,
                    path: Some(w.path.clone()),
                }])
                .render(&text, &w.path)
                .replacen(": error:", ": warning:", 1);
                eprint!("{message}");
            }
        }
        "fmt" => {
            let formatted =
                ncc::formatter::format(&source).map_err(|e| e.render(&source, &o.input))?;
            if formatted != source {
                fs::write(&o.input, formatted).map_err(|e| e.to_string())?;
            }
        }
        _ => return build(&o, &source),
    }
    Ok(ExitCode::SUCCESS)
}
fn build(o: &Options, source: &str) -> Result<ExitCode, String> {
    let run = o.command == "run";
    let format = o.format.unwrap_or_else(|| {
        match o
            .output
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|s| s.to_str())
        {
            Some("c") => Format::C,
            Some("o") => Format::Object,
            _ => Format::Executable,
        }
    });
    let output = o.output.clone().unwrap_or_else(|| {
        o.input.with_extension(match format {
            Format::C => "c",
            Format::Object => "o",
            Format::Executable => "",
        })
    });
    if !run
        && output
            .canonicalize()
            .ok()
            .zip(o.input.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
    {
        return Err("output would overwrite the input source file".into());
    }
    let c = compile_source_with_options(source, &o.input, o.release)
        .map_err(|e| e.render(source, &o.input))?;
    if format == Format::C && !run {
        fs::write(&output, c).map_err(|e| format!("{}: {e}", output.display()))?;
        return Ok(ExitCode::SUCCESS);
    }
    let temporary = ncc::temp::Directory::new()
        .map_err(|e| format!("cannot create temporary build directory: {e}"))?;
    let c_path = temporary.path().join("program.c");
    let binary = temporary.path().join("program");
    fs::write(&c_path, &c).map_err(|e| e.to_string())?;
    let mut compiler = Command::new("cc");
    compiler.arg(&c_path).arg("-std=c11");
    if o.release {
        compiler.arg("-O3");
    } else {
        compiler.args(["-O0", "-g"]);
    }
    if c.contains("#include <pthread.h>") {
        compiler.arg("-pthread");
    }
    if format == Format::Object {
        compiler.arg("-c");
    } else if c.contains("#include <math.h>") {
        compiler.arg("-lm");
    }
    let status = compiler
        .arg("-o")
        .arg(&binary)
        .status()
        .map_err(|e| format!("cannot run C compiler: {e}"))?;
    if !status.success() {
        return Err("C compiler failed".into());
    }
    if run {
        let status = Command::new(&binary)
            .status()
            .map_err(|e| format!("cannot run program: {e}"))?;
        Ok(ExitCode::from(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        ))
    } else {
        fs::copy(&binary, &output).map_err(|e| format!("{}: {e}", output.display()))?;
        Ok(ExitCode::SUCCESS)
    }
}
