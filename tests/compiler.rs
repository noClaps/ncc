use std::{fs, process::Command};

#[test]
fn compiles_and_runs_functions_conditionals_and_output() {
    let source = r#"
        fn classify(int value) int {
          if value {
            0 -> { return 10 }
            _ -> { return 20 }
          }
        }
        test "conditional" {
          assert classify(0) == 10
          assert classify(9) == 20
          @println(classify(0))
        }
    "#;
    let c = ncc::compile_source(source, std::path::Path::new("test.nc")).unwrap();
    let directory = std::env::temp_dir().join(format!("ncc-test-{}", std::process::id()));
    let _ = fs::create_dir_all(&directory);
    let source_path = directory.join("program.c");
    let executable = directory.join("program");
    fs::write(&source_path, c).unwrap();
    assert!(
        Command::new("cc")
            .arg(&source_path)
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new(&executable).output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "10\n");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn rejects_immutable_assignment() {
    let source = "test \"immutable\" { int value = 1 value = 2 }";
    let error = ncc::check_source(source, std::path::Path::new("test.nc")).unwrap_err();
    assert!(error.to_string().contains("cannot mutate immutable"));
}
