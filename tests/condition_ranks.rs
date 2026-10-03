use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("condition.nc"), release)
}

fn folded(source: &str, expected: &str) {
    assert!(!compile(source, true).unwrap().contains("} goto "));
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("condition.nc");
    std::fs::write(&path, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&path).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes());
    }
}

#[test]
fn empty_bodies_can_progress_in_the_condition_and_retain_the_final_update() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {checks=checks+1 i=i+1 return i<5}
while condition() {}
@println(i,":",checks)
"#,
        "5:5\n",
    );
}

#[test]
fn body_and_condition_progress_keep_nested_output_order() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn increment() {i=i+1}
fn condition() bool {checks=checks+1 @print("a",i,":") increment() @print("b",i,"|") return i<6}
while condition() {@print("body",i,"|") i=i+1}
@println(i,":",checks)
"#,
        "a0:b1|body1|a2:b3|body3|a4:b5|body5|a6:b7|7:4\n",
    );
}

#[test]
fn negative_condition_progress_and_multiple_updates_are_certified() {
    folded(
        r#"
mut int i=10
mut int checks=0
fn condition() bool {checks=checks+1 i=i-1 i=i-2 return i>0}
while condition() {}
@println(i,":",checks)
"#,
        "-2:4\n",
    );
}

#[test]
fn break_is_not_followed_by_an_extra_condition_update() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {checks=checks+1 i=i+1 return i<10}
while condition() {i=i+2 break}
@println(i,":",checks)
"#,
        "3:1\n",
    );
}

#[test]
fn initially_false_conditions_still_run_their_first_update() {
    folded(
        r#"
mut byte i=254
mut int checks=0
fn condition() bool {checks=checks+1 i=i+1 return i<255}
while condition() {}
@println(i,":",checks)
"#,
        "255:1\n",
    );
}

#[test]
fn first_and_final_condition_overflow_are_not_interpreted() {
    for source in [
        "mut byte i=255 fn condition() bool {i=i+1 return i<255} while condition(){} @println(i)",
        "mut byte i=253 fn condition() bool {i=i+1 return i<=255} while condition(){} @println(i)",
        "mut byte i=0 fn condition() bool {i=i-1 return i>0} while condition(){} @println(i)",
        "mut byte i=2 fn condition() bool {i=i-1 return i>=0} while condition(){} @println(i)",
    ] {
        for release in [false, true] {
            assert!(
                compile(source, release).unwrap().contains("} goto "),
                "{source}"
            );
        }
    }
}

#[test]
fn resets_mixed_prefix_direction_and_nonexact_strides_do_not_fake_progress() {
    for source in [
        "mut int i=1 fn condition() bool {i=0 return i<3} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
        "mut int i=1 fn condition() bool {i=i-1 i=i+2 return i<3} while condition(){int fail=1/(i-i)} @println(i)",
        "mut int i=0 fn condition() bool {i=i+2 return i!=5} while condition(){int fail=1/(i-i)} @println(i)",
        "mut int i=0 mut int n=5 fn condition() bool {i=i+1 n=n+2 return i<n} while condition(){int fail=1/(i-i)} @println(i)",
    ] {
        for release in [false, true] {
            assert!(
                compile(source, release).unwrap().contains("} goto "),
                "{source}"
            );
        }
    }
}

#[test]
fn continues_cannot_skip_proof_only_condition_updates() {
    let source = "mut int i=1 fn condition() bool {i=i+1 return i<5} while condition(){int fail=1/(i-i) continue} @println(i)";
    for release in [false, true] {
        assert!(compile(source, release).unwrap().contains("} goto "));
    }
}
