use ncc::{check_source, compile_source};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    C,
    Object,
    Executable,
}

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
                eprint!("{}", e.render(&source, &path));
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
                eprint!("{}", e.render(&source, &path));
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
    let mut requested_format = None;
    let mut it = args.peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => output = it.next().map(PathBuf::from),
            "-f" | "--format" => requested_format = it.next(),
            "-r" | "--release" | "-d" | "--debug" => {}
            _ => {
                eprintln!("ncc: unknown option {a}");
                return ExitCode::FAILURE;
            }
        }
    }
    let requested_format = match requested_format.as_deref() {
        None => None,
        Some(format) if format.eq_ignore_ascii_case("c") => Some(OutputFormat::C),
        Some(format) if format.eq_ignore_ascii_case("obj") => Some(OutputFormat::Object),
        Some(format) if format.eq_ignore_ascii_case("exe") => Some(OutputFormat::Executable),
        Some(format) => {
            eprintln!("ncc: unsupported output format `{format}` (expected C, obj, or exe)");
            return ExitCode::FAILURE;
        }
    };
    let c = match compile_source(source, path) {
        Ok(c) => c,
        Err(e) => {
            eprint!("{}", e.render(source, path));
            return ExitCode::FAILURE;
        }
    };
    let format = if run {
        OutputFormat::Executable
    } else {
        requested_format.unwrap_or_else(|| {
            output.as_ref().map_or(OutputFormat::Executable, |out| {
                match out.extension().and_then(|extension| extension.to_str()) {
                    Some("c") => OutputFormat::C,
                    Some("o") => OutputFormat::Object,
                    _ => OutputFormat::Executable,
                }
            })
        })
    };
    let out = if run {
        None
    } else {
        Some(output.unwrap_or_else(|| match format {
            OutputFormat::C => path.with_extension("c"),
            OutputFormat::Object => path.with_extension("o"),
            OutputFormat::Executable => path.with_extension(""),
        }))
    };
    if format == OutputFormat::C {
        let out = out.expect("C output is only used by build");
        return match fs::write(out, c) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("ncc: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let temporary = TemporaryDirectory::new();
    if let Err(e) = fs::create_dir_all(temporary.path()) {
        eprintln!("ncc: cannot create temporary build directory: {e}");
        return ExitCode::FAILURE;
    }
    let c_path = temporary.path().join("program.c");
    if let Err(e) = fs::write(&c_path, c) {
        eprintln!("ncc: {e}");
        return ExitCode::FAILURE;
    }
    let out = if run {
        temporary.path().join("program")
    } else {
        out.expect("build output was determined above")
    };
    let mut compiler = Command::new("cc");
    compiler.arg(&c_path);
    if format == OutputFormat::Object {
        compiler.arg("-c");
    }
    let status = match compiler.arg("-o").arg(&out).status() {
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

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn new() -> Self {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        Self(env::temp_dir().join(format!("ncc-{}-{id}", std::process::id())))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
