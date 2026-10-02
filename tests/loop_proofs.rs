use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("proofs.nc"), release)
}

fn folded(source: &str, expected: &str) {
    let c = compile(source, true).unwrap();
    assert!(!c.contains("} goto "), "certified loops must disappear");
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("proofs.nc");
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

fn unproven(source: &str) {
    for release in [false, true] {
        // The body contains a variable-dependent arithmetic probe. Entering an
        // uncertified candidate would diagnose it rather than preserve runtime code.
        assert!(
            compile(source, release).unwrap().contains("} goto "),
            "{source}"
        );
    }
}

#[test]
fn effectful_conditions_preserve_every_check_and_nested_output() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {
    checks=checks+1
    @print("c", checks, ":")
    return i<3
}
while condition() { @print(i,"|") i=i+1 }
@println(i,":",checks)
"#,
        "c1:0|c2:1|c3:2|c4:3:4\n",
    );
}

#[test]
fn local_closure_conditions_and_helpers_share_the_actual_counter() {
    folded(
        r"
fn calculate() (int,int) {
    mut int i=0
    mut int checks=0
    fn condition() bool {checks=checks+1 return i<3}
    fn advance() {i=i+1}
    while condition() {advance()}
    return (i,checks)
}
@println(calculate())
",
        "(3, 4)\n",
    );
}

#[test]
fn conditional_updates_allow_different_monotonic_strides() {
    folded(
        r#"
mut int i=0
while i<9 {if i%2==0 {true->{i=i+1} false->{i=i+2}}}
mut int j=9
while j>0 {if j%2==0 {true->{j=j-2} false->{j=j-1}}}
@println(i,":",j)
"#,
        "9:0\n",
    );
}

#[test]
fn every_continue_path_has_progress_and_break_paths_need_none() {
    folded(
        r#"
mut int i=0
rows: while i<7 {
    if i<3 {true->{i=i+1 continue :rows} false->{i=i+2}}
}
mut int j=0
while j<4 {if j==2 {true->{break} false->{j=j+1}}}
@println(i,":",j)
"#,
        "7:2\n",
    );
}

#[test]
fn helper_chains_and_pure_scalar_updates_are_expanded_without_execution() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn next(int value) int {return value+1}
fn advance() {i=next(i)}
fn outer() {advance()}
fn test_condition() bool {checks=checks+1 return i<5}
fn condition() bool {checks=checks+10 return test_condition()}
while condition() {outer()}
@println(i,":",checks)
"#,
        "5:66\n",
    );
}

#[test]
fn conditional_shared_helper_progress_covers_every_branch() {
    folded(
        r"
mut int i=0
fn advance() {if i%2==0 {true->{i=i+1} false->{i=i+2}}}
while i<9 {advance()}
@println(i)
",
        "9\n",
    );
}

#[test]
fn helper_updates_do_not_confuse_global_and_caller_local_shadows() {
    folded(
        r#"
mut int i=0
fn advance() {i=i+1}
fn calculate() int {
    mut int i=100
    while i<103 {advance() i=i+1}
    return i
}
@println(calculate(),":",i)
"#,
        "103:3\n",
    );
}

#[test]
fn immutable_captured_bounds_are_copies_not_mutable_aliases() {
    folded(
        r"
fn calculate() (int,int) {
    mut int i=0
    mut int source_bound=3
    int n=source_bound
    fn condition() bool {return i<n}
    while condition() {source_bound=source_bound+1 i=i+1}
    return (i,source_bound)
}
@println(calculate())
",
        "(3, 6)\n",
    );
}

#[test]
fn body_local_bound_shadows_do_not_change_the_actual_condition_bound() {
    folded(
        r"
mut int i=0
mut int n=3
while i<n {int n=100 i=i+1}
@println((i,n))
",
        "(3, 3)\n",
    );
}

#[test]
fn known_callable_aliases_can_supply_progress() {
    folded(
        r"
mut int i=0
fn advance() {i=i+1}
(fn() void) alias=advance
while i<3 {alias()}
@println(i)
",
        "3\n",
    );
}

#[test]
fn changing_bounds_and_reset_helpers_are_not_assumed_invariant() {
    for source in [
        "mut int i=1 mut int n=4 fn change(){n=n+1} while i<n {int fail=1/(i-i) change() i=i+1} @println(i)",
        "mut int i=1 fn reset(){i=0} while i<4 {int fail=1/(i-i) reset() i=i+1} @println(i)",
        "mut int i=1 mut int n=4 fn condition() bool {n=n+1 return i<n} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
        "mut int i=1 fn condition() bool {i=0 return i<4} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn lexical_condition_shadows_cannot_fake_counter_progress() {
    for source in [
        "mut int i=1 fn condition() bool {int i=0 return i<3} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
        "mut int i=1 fn inner(int i) bool {return i<3} fn condition() bool {int i=0 return inner(i)} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
        "mut int i=1 mut int n=3 fn condition() bool {int n=100 return i<n} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn skipped_progress_nonexact_strides_and_direction_changes_stay_runtime() {
    for source in [
        "mut int i=1 while i<4 {int fail=1/(i-i) if i%2==0 {true->{i=i+1} false->{continue}}} @println(i)",
        "mut int i=1 while i!=4 {int fail=1/(i-i) if i%2==0 {true->{i=i+1} false->{i=i+2}}} @println(i)",
        "mut int i=1 while i<4 {int fail=1/(i-i) if i%2==0 {true->{i=i+1} false->{i=i-1}}} @println(i)",
        "mut int i=1 fn advance(int value) int {return value+1} while i<4 {int fail=1/(i-i) int discarded=advance(i)} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn recursive_native_and_replaced_helpers_are_not_executed_for_a_proof() {
    for source in [
        "mut int i=1 fn advance(){advance()} while i<4 {int fail=1/(i-i) advance()} @println(i)",
        "mut int i=1 fn condition() bool {return condition()} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
        "mut int i=1 fn advance(){i=i+1} fn reset(){i=0} mut (fn() void) callback=advance fn replace(){callback=reset} while i<4 {int fail=1/(i-i) replace() callback()} @println(i)",
        "mut int i=1 fn condition() bool {return @args().len<3} while condition(){int fail=1/(i-i) i=i+1} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn conditional_overflow_on_continuing_and_exit_paths_blocks_certification() {
    for source in [
        "mut byte i=254 while i<255 {int fail=1/(@as(int,i)-@as(int,i)) if i==254 {true->{i=i+1} false->{i=i+2}}} @println(i)",
        "mut byte i=254 while i<255 {int fail=1/(@as(int,i)-@as(int,i)) if i==254 {true->{i=i+1} false->{i=i+2 break}}} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn condition_helper_diagnostics_retain_the_original_source_expression() {
    let source = "mut int i=0 mut int checks=0 fn condition() bool {checks=checks+1 int fail=1/(checks-checks) return i<3} while condition(){i=i+1} @println(i)";
    assert!(compile(source, false).is_ok());
    let error = compile(source, true).unwrap_err();
    assert!(format!("{error:?}").contains("division by zero"));
    assert!(
        error
            .0
            .iter()
            .any(|diagnostic| source.get(diagnostic.span.clone()) == Some("1/(checks-checks)"))
    );
}
