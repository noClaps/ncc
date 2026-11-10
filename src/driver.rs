//! Command implementations: compiling NC to C and invoking the system C compiler.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::cgen::{self, Mode};
use crate::diag::SourceMap;
use crate::tir::Program;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    C,
    Asm,
    Exe,
}

#[derive(Default)]
pub struct Options {
    pub file: Option<PathBuf>,
    pub release: bool,
    pub format: Option<Format>,
    pub output: Option<PathBuf>,
}

/// Parse and type-check a program, printing diagnostics on failure.
fn frontend(file: &Path, sm: &mut SourceMap, release: bool) -> Option<Program> {
    let r = crate::modules::load_with(sm, file, &Default::default()).map(|(graph, load_errors)| {
        let (mut p, mut errors, _) = crate::check::check_partial(graph);
        errors.extend(load_errors);
        // compile-time panics are reported alongside type errors
        if let Err(more) = crate::eval::Evaluator::run(&mut p, sm, release && errors.is_empty()) {
            errors.extend(more);
        }
        errors.sort_by_key(|d| (d.span.file, d.span.start));
        errors.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
        if errors.is_empty() {
            for w in crate::lint::race_warnings(&p) {
                eprintln!("{}", w.render_as("warning", sm));
            }
        }
        (p, errors)
    });
    let errors = match r {
        Ok((p, errors)) if errors.is_empty() => return Some(p),
        Ok((_, errors)) => errors,
        Err(errors) => errors,
    };
    for d in &errors {
        eprintln!("{}\n", d.render(sm));
    }
    if errors.len() > 1 {
        eprintln!("error: found {} errors", errors.len());
    }
    None
}

/// The C compiler command from `$CC`, falling back to `cc`.
fn cc_command() -> (String, Vec<String>) {
    let cc = std::env::var("CC")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "cc".into());
    let mut parts = cc.split_whitespace().map(|s| s.to_string());
    let prog = parts.next().unwrap();
    (prog, parts.collect())
}

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir();
        loop {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = base.join(format!("ncc-{}-{n}", std::process::id()));
            match std::fs::create_dir(&p) {
                Ok(()) => return Ok(TempDir(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compile a program to the given format at `out`. Returns false on failure (already reported).
fn compile(
    p: &Program,
    sm: &SourceMap,
    mode: Mode,
    release: bool,
    format: Format,
    out: &Path,
) -> bool {
    let (c_src, header) = cgen::generate(p, sm, mode);
    if format == Format::C {
        if let Err(e) = std::fs::write(out, c_src) {
            eprintln!("error: cannot write `{}`: {e}", out.display());
            return false;
        }
        return true;
    }
    let tmp = match TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return false;
        }
    };
    let main_c = tmp.0.join("main.c");
    let ok = std::fs::write(&main_c, &c_src)
        .and_then(|_| std::fs::write(tmp.0.join("nc.h"), cgen::RUNTIME))
        .and_then(|_| std::fs::write(tmp.0.join("nc_gen.h"), &header));
    if let Err(e) = ok {
        eprintln!("error: cannot write temporary files: {e}");
        return false;
    }
    let (prog, mut args) = cc_command();
    args.push("-std=c23".into());
    if release {
        args.extend(["-O3", "-DNDEBUG"].map(String::from));
    } else {
        args.extend(["-O0", "-g", "-fno-omit-frame-pointer"].map(String::from));
    }
    args.push("-w".into());
    args.push(format!("-I{}", tmp.0.display()));
    match format {
        Format::Asm => {
            args.push("-S".into());
            args.push(main_c.display().to_string());
        }
        _ => {
            args.push(main_c.display().to_string());
            let mut seen = Vec::new();
            for x in &p.externs {
                if !seen.contains(&x.file) {
                    seen.push(x.file.clone());
                }
            }
            for f in seen {
                if !f.is_file() {
                    eprintln!("error: extern implementation `{}` not found", f.display());
                    return false;
                }
                args.push(f.display().to_string());
            }
        }
    }
    args.push("-o".into());
    args.push(out.display().to_string());
    if format == Format::Exe {
        args.push("-lm".into());
        args.push("-lpthread".into());
    }
    match Command::new(&prog).args(&args).status() {
        Ok(s) if s.success() => true,
        Ok(_) => {
            eprintln!("error: the C compiler (`{prog}`) failed");
            false
        }
        Err(e) => {
            eprintln!("error: cannot run the C compiler `{prog}`: {e}");
            false
        }
    }
}

pub fn check(o: &Options) -> ExitCode {
    let mut sm = SourceMap::default();
    match frontend(o.file.as_ref().unwrap(), &mut sm, false) {
        Some(_) => ExitCode::SUCCESS,
        None => ExitCode::FAILURE,
    }
}

fn format_from_ext(p: &Path) -> Format {
    match p.extension().and_then(|e| e.to_str()) {
        Some("c") => Format::C,
        Some("s") | Some("asm") => Format::Asm,
        _ => Format::Exe,
    }
}

pub fn build(o: &Options) -> ExitCode {
    let file = o.file.as_ref().unwrap();
    let format = o.format.unwrap_or_else(|| {
        o.output
            .as_deref()
            .map(format_from_ext)
            .unwrap_or(Format::Exe)
    });
    let out = o.output.clone().unwrap_or_else(|| {
        let stem = file.with_extension("");
        match format {
            Format::C => stem.with_extension("c"),
            Format::Asm => stem.with_extension("s"),
            Format::Exe => stem,
        }
    });
    let mut sm = SourceMap::default();
    let Some(p) = frontend(file, &mut sm, o.release) else {
        return ExitCode::FAILURE;
    };
    if compile(&p, &sm, Mode::Run, o.release, format, &out) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_exe(exe: &Path) -> ExitCode {
    match Command::new(exe).status() {
        Ok(s) => match s.code() {
            Some(c) => ExitCode::from(c as u8),
            None => {
                eprintln!("error: program terminated by a signal");
                ExitCode::from(134)
            }
        },
        Err(e) => {
            eprintln!("error: cannot run program: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_and_run(o: &Options, mode: Mode) -> ExitCode {
    let file = o.file.as_ref().unwrap();
    let mut sm = SourceMap::default();
    let Some(p) = frontend(file, &mut sm, o.release) else {
        return ExitCode::FAILURE;
    };
    let tmp = match TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let exe = tmp.0.join("program");
    if !compile(&p, &sm, mode, o.release, Format::Exe, &exe) {
        return ExitCode::FAILURE;
    }
    run_exe(&exe)
}

pub fn run(o: &Options) -> ExitCode {
    build_and_run(o, Mode::Run)
}

pub fn test(o: &Options) -> ExitCode {
    build_and_run(o, Mode::Test)
}

pub fn init(name: Option<String>) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot determine the current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let name = name.unwrap_or_else(|| {
        cwd.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let nc = cwd.join(".nc");
    if nc.exists() {
        eprintln!(
            "error: `{}` already exists; this directory is already a project",
            nc.display()
        );
        return ExitCode::FAILURE;
    }
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let r = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(nc.join("deps"))?;
        std::fs::write(nc.join("config.toml"), format!("name = \"{escaped}\"\n"))?;
        let gi = cwd.join(".gitignore");
        if !gi.exists() {
            std::fs::write(gi, ".nc/deps/\n")?;
        }
        let main = cwd.join("main.nc");
        if !main.exists() {
            std::fs::write(main, "@println(\"Hello, world!\")\n")?;
        }
        Ok(())
    })();
    match r {
        Ok(()) => {
            println!("Initialised project `{name}`");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
