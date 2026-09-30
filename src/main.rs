use ncc::{compile_source_with_diagnostics, compile_test_source_with_diagnostics};
use std::{
    env, fs,
    path::{Path, PathBuf},
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
    target: Option<String>,
    arguments: Vec<String>,
}

fn help(command: &str) {
    match command {
        "build" => println!(
            "Usage: ncc build <file> [options]\n\nBuild to an executable, C source, or an object file.\n\n  -o, --output <file>   Output path (extension determines format)\n  -f, --format <C|obj|exe>  Override output format\n  --target <target>    Compilation target (or NC_TARGET environment variable)\n  -r, --release         Aggressive constant evaluation and optimisation\n  -d, --debug           Debug build (default)\n  -h, --help            Show help"
        ),
        "run" => println!(
            "Usage: ncc run <file> [options] [-- program arguments...]\n\nCompile and run without leaving generated files.\n\n  -r, --release  Aggressive constant evaluation and optimisation\n  -d, --debug    Debug build (default)\n  -h, --help     Show help"
        ),
        "test" => println!(
            "Usage: ncc test <file> [options] [-- program arguments...]\n\nCompile and execute test blocks without leaving generated files.\n\n  -r, --release  Aggressive constant evaluation and optimisation\n  -d, --debug    Debug build (default)\n  -h, --help     Show help"
        ),
        _ => println!(
            "Usage: ncc <command> [options]\n\nCommands:\n  build  Build an executable, C source, or object file\n  run    Compile and execute without leaving build files\n  test   Compile and execute test blocks without leaving build files\n\n  --targets     List supported compilation targets\n  -h, --help     Show help\n  -V, --version  Show version\n\nUse `ncc <command> --help` for command-specific options."
        ),
    }
}
fn parse(args: Vec<String>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("missing command; use `ncc --help`")?;
    if command == "--targets" {
        if let Some(arg) = args.next() {
            return Err(format!("unexpected argument `{arg}`"));
        }
        println!("{}", ncc::target::NAME);
        return Ok(None);
    }
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        help(&args.next().unwrap_or_default());
        return Ok(None);
    }
    if matches!(command.as_str(), "-V" | "--version") {
        println!("ncc {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    if !matches!(command.as_str(), "build" | "run" | "test") {
        return Err(format!("unknown command `{command}`; use `ncc --help`"));
    }
    let mut options = Options {
        command,
        input: PathBuf::new(),
        output: None,
        format: None,
        release: false,
        target: None,
        arguments: vec![],
    };
    let mut positional = false;
    while let Some(arg) = args.next() {
        if !positional && matches!(arg.as_str(), "-h" | "--help") {
            help(&options.command);
            return Ok(None);
        }
        if !positional && arg == "--" {
            if matches!(options.command.as_str(), "run" | "test")
                && !options.input.as_os_str().is_empty()
            {
                options.arguments.extend(args);
                break;
            }
            positional = true;
            continue;
        }
        if !positional && arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(a, b)| (a, Some(b.to_owned())));
            match flag {
                "--target" if options.command == "build" => {
                    let target = inline
                        .or_else(|| args.next())
                        .ok_or("--target requires a value")?;
                    ncc::target::validate(&target)?;
                    options.target = Some(target);
                }
                "-r" | "--release" | "-d" | "--debug" if inline.is_none() => {
                    options.release = matches!(flag, "-r" | "--release");
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
        } else if options.input.as_os_str().is_empty() {
            options.input = arg.into();
        } else {
            return Err(format!("unexpected argument `{arg}`"));
        }
    }
    if options.input.as_os_str().is_empty() {
        return Err(format!(
            "missing input file; use `ncc {} --help`",
            options.command
        ));
    }
    Ok(Some(options))
}
fn main() -> ExitCode {
    let result = parse(env::args().skip(1).collect())
        .and_then(|options| options.as_ref().map_or(Ok(ExitCode::SUCCESS), execute));
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ncc: {error}");
            ExitCode::FAILURE
        }
    }
}
fn execute(o: &Options) -> Result<ExitCode, String> {
    let source = fs::read_to_string(&o.input).map_err(|e| format!("{}: {e}", o.input.display()))?;
    build(o, &source)
}

fn build(o: &Options, source: &str) -> Result<ExitCode, String> {
    let run = matches!(o.command.as_str(), "run" | "test");
    let target = if run {
        ncc::target::NAME.to_owned()
    } else {
        o.target
            .clone()
            .or_else(|| env::var("NC_TARGET").ok())
            .unwrap_or_else(|| ncc::target::NAME.into())
    };
    ncc::target::validate(&target)?;
    let (format, output) = output_artifact(o);
    if !run
        && output
            .canonicalize()
            .ok()
            .zip(o.input.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
    {
        return Err("output would overwrite the input source file".into());
    }
    let compile = if o.command == "test" {
        compile_test_source_with_diagnostics
    } else {
        compile_source_with_diagnostics
    };
    let compiled = compile(source, &o.input, o.release).map_err(|e| e.render(source, &o.input))?;
    eprint!("{}", compiled.warnings.render_warnings(source, &o.input));
    let c = compiled.c;
    if format == Format::C && !run {
        fs::write(&output, c).map_err(|e| format!("{}: {e}", output.display()))?;
        return Ok(ExitCode::SUCCESS);
    }
    if !cfg!(target_os = "macos") {
        return Err("native macos-arm64 builds require a macOS C toolchain; use --format C to emit portable source".into());
    }
    let temporary = ncc::temp::Directory::new()
        .map_err(|e| format!("cannot create temporary build directory: {e}"))?;
    let c_path = temporary.path().join("program.c");
    let binary = temporary.path().join("program");
    fs::write(&c_path, &c).map_err(|e| e.to_string())?;
    compile_native(&c, &c_path, &binary, format, o.release)?;
    if run {
        let status = Command::new(&binary)
            .args(&o.arguments)
            .status()
            .map_err(|e| format!("cannot run program: {e}"))?;
        Ok(ExitCode::from(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        ))
    } else {
        publish_artifact(&binary, &output)?;
        Ok(ExitCode::SUCCESS)
    }
}

fn output_artifact(o: &Options) -> (Format, PathBuf) {
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
    (format, output)
}

fn compile_native(
    c: &str,
    c_path: &Path,
    binary: &Path,
    format: Format,
    release: bool,
) -> Result<(), String> {
    let mut native_compiler = Command::new("cc");
    native_compiler.arg(c_path).arg("-std=c11");
    native_compiler.args(["-arch", ncc::target::ARCH]);
    if release {
        native_compiler.arg("-O3");
    } else {
        native_compiler.args(["-O0", "-g"]);
    }
    if c.contains("#include <pthread.h>") {
        native_compiler.arg("-pthread");
    }
    if format == Format::Object {
        native_compiler.arg("-c");
    } else if c.contains("#include <math.h>") {
        native_compiler.arg("-lm");
    }
    let status = native_compiler
        .arg("-o")
        .arg(binary)
        .status()
        .map_err(|e| format!("cannot run C compiler: {e}"))?;
    if !status.success() {
        return Err("C compiler failed".into());
    }
    Ok(())
}

fn publish_artifact(binary: &Path, output: &Path) -> Result<(), String> {
    // Replace the inode atomically. Overwriting a previously executed Mach-O
    // in place can retain stale code-signature cache entries on macOS.
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = ncc::temp::Directory::new_in(parent).map_err(|e| e.to_string())?;
    let staged = staging.path().join("output");
    fs::copy(binary, &staged).map_err(|e| format!("{}: {e}", output.display()))?;
    fs::rename(&staged, output).map_err(|e| format!("{}: {e}", output.display()))?;
    Ok(())
}
