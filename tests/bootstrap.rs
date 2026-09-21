use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn compile_input(compiler: &Path, source: &[u8]) -> std::process::Output {
    let mut child = Command::new(compiler)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn nc_compiler_builds_and_runs_another_program() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp = ncc::temp::Directory::new().unwrap();
    let compiler = temp.path().join("nc-bootstrap");
    for mode in ["--debug", "--release"] {
        let built = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("build")
            .arg(root.join("examples/bootstrap/compiler.nc"))
            .arg(mode)
            .arg("-o")
            .arg(&compiler)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let source = fs::read(root.join("examples/bootstrap/input.nc")).unwrap();
        let output = compile_input(&compiler, &source);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let emitted = temp.path().join("output.c");
        let executable = temp.path().join("output");
        fs::write(&emitted, &output.stdout).unwrap();
        let status = Command::new("cc")
            .arg(&emitted)
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap();
        assert!(status.success());
        let output = Command::new(&executable).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"42\n43\n-5\n");
        let error = compile_input(&compiler, b"@println(missing)");
        assert!(!error.status.success());
        assert!(String::from_utf8_lossy(&error.stderr).contains("unknown name: missing"));
        assert!(error.stdout.is_empty());
        let error = compile_input(&compiler, b"@println(2 +)");
        assert!(!error.status.success());
    }
}
