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
    /// arguments for the program (`ncc run <file> -- <args>...`)
    pub args: Vec<String>,
}

/// Parse and type-check a program, printing diagnostics on failure.
/// `codegen` is set when the program will be compiled to C (not just checked): the
/// compilation target is then needed for `@target`.
fn frontend(file: &Path, sm: &mut SourceMap, release: bool, codegen: bool) -> Option<Program> {
    let r = crate::modules::load_with(sm, file, &Default::default()).map(|(graph, load_errors)| {
        let entry_file = graph.modules[0].file;
        let (mut p, mut errors, index) = crate::check::check(graph, false);
        errors.extend(load_errors);
        if codegen && errors.is_empty() && uses_target(&p) {
            match detect_target() {
                Ok(t) => p.target = Some(t),
                Err(e) => errors.push(crate::diag::Diag::new(Default::default(), e)),
            }
        }
        // loops are checked before release folding rewrites them, so both modes agree
        let loops = errors.is_empty().then(|| crate::lint::loops(&p, sm));
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
            // unreachable code after a loop that never finishes: like unused code below
            let unreachable = loops.map(|l| l.unreachable).unwrap_or_default();
            if release {
                errors.extend(unreachable);
            } else {
                for d in unreachable {
                    eprintln!("{}", d.render_as("warning", sm));
                }
            }
            // unused code: warnings in debug mode, errors in release mode
            let unused = crate::lint::unused(&index, sm);
            if release {
                errors.extend(unused.into_iter().map(|u| u.diag));
            } else {
                for u in unused {
                    eprintln!("{}", u.diag.render_as("warning", sm));
                }
            }
        }
        if release && errors.is_empty() {
            // dropping unused imported initialisers can make more code dead, and the reverse
            crate::dce::run(&mut p);
            while crate::shake::run(&mut p, entry_file) {
                crate::dce::run(&mut p);
            }
            p.entry_file = Some(entry_file);
        }
        (p, errors)
    });
    let errors = match r {
        Ok((p, errors)) if errors.is_empty() => return Some(p),
        Ok((_, errors)) | Err(errors) => errors,
    };
    for d in &errors {
        eprintln!("{}\n", d.render(sm));
    }
    if errors.len() > 1 {
        eprintln!("error: found {} errors", errors.len());
    }
    None
}

fn uses_target(p: &Program) -> bool {
    let mut found = false;
    for f in &p.funcs {
        for s in &f.body.stmts {
            crate::check::flow::visit_stmt(s, &mut |e| {
                found |= matches!(e.kind, crate::tir::EK::Target);
            });
        }
    }
    found
}

/// The C compiler's target as `(os, cpu)`, from its predefined macros.
fn detect_target() -> Result<(String, String), String> {
    let (prog, mut args) = cc_command();
    args.extend(["-dM", "-E", "-x", "c", "-"].map(String::from));
    let out = Command::new(&prog)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run the C compiler `{prog}` to detect the target: {e}"))?;
    let macros = String::from_utf8_lossy(&out.stdout);
    let defined = |m: &str| {
        macros
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(m))
    };
    let os = [
        ("__ANDROID__", "android"),
        ("__APPLE__", "macos"),
        ("__linux__", "linux"),
        ("_WIN32", "windows"),
        ("__FreeBSD__", "freebsd"),
        ("__OpenBSD__", "openbsd"),
        ("__NetBSD__", "netbsd"),
        ("__wasi__", "wasi"),
    ]
    .iter()
    .find(|(m, _)| defined(m))
    .map_or("unknown", |(_, n)| *n);
    let cpu = match [
        ("__aarch64__", "aarch64"),
        ("__x86_64__", "x86_64"),
        ("__i386__", "x86"),
        ("__arm__", "arm"),
        ("__powerpc64__", "powerpc64"),
        ("__wasm32__", "wasm32"),
        ("__wasm64__", "wasm64"),
    ]
    .iter()
    .find(|(m, _)| defined(m))
    {
        Some((_, n)) => n,
        None if macros.contains("#define __riscv_xlen 64") => "riscv64",
        None if defined("__riscv") => "riscv32",
        None => "unknown",
    };
    Ok((os.to_string(), cpu.to_string()))
}

/// The C compiler command from `$CC`, falling back to `cc`.
fn cc_command() -> (String, Vec<String>) {
    let cc = std::env::var("CC")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "cc".into());
    let mut parts = cc.split_whitespace().map(ToString::to_string);
    let prog = parts.next().unwrap();
    (prog, parts.collect())
}

struct TempDir(PathBuf);
impl TempDir {
    /// A new temporary directory; `None` (already reported) when it cannot be created.
    fn new() -> Option<Self> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir();
        loop {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = base.join(format!("ncc-{}-{n}", std::process::id()));
            match std::fs::create_dir(&p) {
                Ok(()) => return Some(Self(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    eprintln!("error: cannot create a temporary directory: {e}");
                    return None;
                }
            }
        }
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `p` for messages and `#line`: relative to the working directory when inside it, without
/// `.` components.
fn display_path(p: &Path) -> String {
    let clean: PathBuf = p
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    let rel = std::env::current_dir()
        .ok()
        .and_then(|d| clean.strip_prefix(d).ok().map(Path::to_path_buf));
    rel.unwrap_or(clean).display().to_string()
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
    // extern C files go into the generated translation unit; release builds leave out files
    // whose functions are never called
    let reached = p.entry_file.map(|f| crate::shake::reach(p, f).externs);
    let mut files: Vec<&Path> = Vec::new();
    for (i, x) in p.externs.iter().enumerate() {
        if reached.as_ref().is_none_or(|r| r.contains(&i)) && !files.contains(&x.file.as_path()) {
            files.push(&x.file);
        }
    }
    let mut externs = Vec::new();
    for f in &files {
        match std::fs::read_to_string(f) {
            Ok(text) => externs.push(cgen::ExternSource {
                path: display_path(f),
                text,
            }),
            Err(_) => {
                eprintln!("error: extern implementation `{}` not found", f.display());
                return false;
            }
        }
    }
    let (c_src, header) = cgen::generate(p, sm, mode, &externs);
    if format == Format::C {
        if let Err(e) = std::fs::write(out, c_src) {
            eprintln!("error: cannot write `{}`: {e}", out.display());
            return false;
        }
        return true;
    }
    let Some(tmp) = TempDir::new() else {
        return false;
    };
    let main_c = tmp.0.join("main.c");
    let ok = std::fs::write(&main_c, &c_src)
        .and_then(|_| std::fs::write(tmp.0.join("nc.h"), cgen::HEADER))
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
    // headers the extern files include from their own directories
    for f in &files {
        if let Some(d) = f.parent().filter(|d| !d.as_os_str().is_empty()) {
            args.push(format!("-I{}", d.display()));
        }
    }
    if format == Format::Asm {
        args.push("-S".into());
    }
    args.push(main_c.display().to_string());
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
    match frontend(o.file.as_ref().unwrap(), &mut sm, false, false) {
        Some(_) => ExitCode::SUCCESS,
        None => ExitCode::FAILURE,
    }
}

fn format_from_ext(p: &Path) -> Format {
    match p.extension().and_then(|e| e.to_str()) {
        Some("c") => Format::C,
        Some("s" | "asm") => Format::Asm,
        _ => Format::Exe,
    }
}

pub fn build(o: &Options) -> ExitCode {
    let file = o.file.as_ref().unwrap();
    let format = o
        .format
        .unwrap_or_else(|| o.output.as_deref().map_or(Format::Exe, format_from_ext));
    let out = o.output.clone().unwrap_or_else(|| {
        let stem = file.with_extension("");
        match format {
            Format::C => stem.with_extension("c"),
            Format::Asm => stem.with_extension("s"),
            Format::Exe => stem,
        }
    });
    let mut sm = SourceMap::default();
    let Some(p) = frontend(file, &mut sm, o.release, true) else {
        return ExitCode::FAILURE;
    };
    if compile(&p, &sm, Mode::Run, o.release, format, &out) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_exe(exe: &Path, args: &[String]) -> ExitCode {
    match Command::new(exe).args(args).status() {
        Ok(s) => {
            if let Some(c) = s.code() {
                ExitCode::from(c as u8)
            } else {
                eprintln!("error: program terminated by a signal");
                ExitCode::from(134)
            }
        }
        Err(e) => {
            eprintln!("error: cannot run program: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_and_run(o: &Options, mode: Mode) -> ExitCode {
    let file = o.file.as_ref().unwrap();
    let mut sm = SourceMap::default();
    let Some(p) = frontend(file, &mut sm, o.release, true) else {
        return ExitCode::FAILURE;
    };
    let Some(tmp) = TempDir::new() else {
        return ExitCode::FAILURE;
    };
    let exe = tmp.0.join("program");
    if !compile(&p, &sm, mode, o.release, Format::Exe, &exe) {
        return ExitCode::FAILURE;
    }
    run_exe(&exe, &o.args)
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

pub fn fmt(file: &Path, check: bool, output: Option<Option<String>>) -> ExitCode {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let formatted = match crate::fmt::format(&src) {
        Ok(f) => f,
        Err(errors) => {
            let mut sm = SourceMap::default();
            sm.add(file.to_path_buf(), src);
            for d in &errors {
                eprintln!("{}\n", d.render(&sm));
            }
            eprintln!(
                "error: cannot format `{}` because it has syntax errors",
                file.display()
            );
            return ExitCode::FAILURE;
        }
    };
    if check {
        if formatted == src {
            return ExitCode::SUCCESS;
        }
        eprintln!("`{}` is not formatted", file.display());
        return ExitCode::FAILURE;
    }
    let r = match output {
        None => {
            if formatted == src {
                return ExitCode::SUCCESS;
            }
            std::fs::write(file, &formatted).map_err(|e| (file.display().to_string(), e))
        }
        Some(None) => {
            use std::io::Write;
            std::io::stdout()
                .write_all(formatted.as_bytes())
                .map_err(|e| ("stdout".into(), e))
        }
        Some(Some(out)) => std::fs::write(&out, &formatted).map_err(|e| (out, e)),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err((what, e)) => {
            eprintln!("error: cannot write `{what}`: {e}");
            ExitCode::FAILURE
        }
    }
}
