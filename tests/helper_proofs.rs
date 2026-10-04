use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("helpers.nc"), release)
}

fn folded(source: &str, expected: &str) {
    let c = compile(source, true).unwrap();
    assert!(!c.contains("} goto "), "certified loops must disappear");
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("helpers.nc");
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
        // Variable-dependent arithmetic probes detect speculative entry without
        // executing an infinite loop in either mode.
        assert!(compile(source, release).unwrap().contains("} goto "));
    }
}

#[test]
fn early_returns_exit_only_the_helper_and_preserve_output_order() {
    folded(
        r#"
mut int i=0
fn advance(int step) {
    @print("a",i,":")
    i=i+step
    if i {1->{@print("r|") return} _->{}}
    @print("b|")
    i=i+1
}
while i<5 {advance(1) @print("c",i,"|")}
@println(i)
"#,
        "a0:r|c1|a1:b|c3|a3:b|c5|5\n",
    );
}

#[test]
fn early_condition_returns_keep_branch_effects_and_the_final_false_check() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {
    checks=checks+1
    @print("c",checks,":")
    i=i+1
    if i {1->{return i<5} _->{}}
    i=i+1
    return i<6
}
while condition() {@print(i,"|")}
@println(i,":",checks)
"#,
        "c1:1|c2:3|c3:5|c4:7:4\n",
    );
}

#[test]
fn labeled_helper_conditionals_do_not_confuse_caller_loop_labels() {
    folded(
        r"
mut int i=0
fn advance() {
    i=i+1
    inside: if i {1->{return} _->{break :inside}}
    i=i+1
}
inside: while i<5 {advance() continue :inside}
@println(i)
",
        "5\n",
    );
}

#[test]
fn counted_and_array_helper_loops_supply_progress_with_copied_steps() {
    folded(
        r"
mut int i=0
fn advance(int step) {
    mut int j=4
    while j>=0 {j=j-2 i=i+step}
}
while i<6 {advance(1)}
fn array_advance() {for key in [10,20,30] {i=i+1}}
while i<12 {array_advance()}
@println(i)
",
        "12\n",
    );
}

#[test]
fn early_returns_cross_certified_helper_loops_without_exiting_the_caller() {
    folded(
        r"
mut int i=0
fn condition() bool {
    i=i+1
    mut int j=0
    while j<2 {
        j=j+1
        if j {1->{return i<3} _->{}}
    }
    return i<3
}
while condition() {}
fn advance() {
    i=i+1
    for key in [10,20] {return}
    i=i+100
}
while i<6 {advance()}
@println(i)
",
        "6\n",
    );
}

#[test]
fn effectful_arguments_are_ordered_and_parameter_snapshots_are_independent() {
    folded(
        r#"
mut int i=0
mut int calls=0
fn argument() int {calls=calls+1 i=i+1 @print("a",calls,"|") return 1}
fn advance(int before, int step, int after) {
    @print(before,":",after,"|")
    {mut int step=100 step=step+after}
    i=i+step
}
while i<4 {advance(i,argument(),i)}
@println(i,":",calls)
"#,
        "a1|0:1|a2|2:3|4:2\n",
    );
}

#[test]
fn immutable_captures_and_parameter_shadows_keep_their_lexical_storage() {
    folded(
        r"
fn calculate() (int,int) {
    mut int i=0
    mut int bound=3
    int copied=bound
    fn condition(int other) bool {
        if i {0->{return i<copied} _->{}}
        {int copied=100 int other=copied}
        return i<copied
    }
    fn advance(int step) {
        {mut int i=100 i=i+step}
        bound=bound+1
        i=i+step
    }
    while condition(9) {advance(1)}
    return (i,bound)
}
@println(calculate())
",
        "(3, 6)\n",
    );
}

#[test]
fn parameter_copies_and_early_returns_cannot_manufacture_progress() {
    for source in [
        "mut int i=1 fn argument() int {i=i+1 return 1} fn reset(int saved,int ignored) {i=saved} while i<4 {int fail=1/(i-i) reset(i,argument())} @println(i)",
        "mut int i=1 fn advance(int saved) {if i {1->{return} _->{}} i=i+saved} while i<4 {int fail=1/(i-i) advance(1)} @println(i)",
        "mut int i=1 fn advance(int i) {mut int copy=i copy=copy+1} while i<4 {int fail=1/(i-i) advance(i)} @println(i)",
        "mut int i=1 fn condition(int saved) bool {i=i+1 return saved<4} while condition(i) {int fail=1/(i-i)} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn unproven_helper_loops_resets_and_overflow_do_not_execute() {
    for source in [
        "mut int i=1 fn advance() {while true {} i=i+1} while i<4 {int fail=1/(i-i) advance()} @println(i)",
        "mut int i=1 fn advance() {mut int j=0 while j<2 {j=0 j=j+1} i=i+1} while i<4 {int fail=1/(i-i) advance()} @println(i)",
        "mut int i=1 fn advance() {mut int j=0 while j<2 {j=j+1 i=0} i=i+1} while i<4 {int fail=1/(i-i) advance()} @println(i)",
        "mut int i=1 fn advance() {mut byte j=254 while j<255 {j=j+2} i=i+1} while i<4 {int fail=1/(i-i) advance()} @println(i)",
        "mut byte i=254 fn advance() {for key in [1,2] {i=i+1}} while i<255 {int fail=1/(@as(int,i)-@as(int,i)) advance()} @println(i)",
        "mut int i=1 fn condition() bool {if i {1->{return true} _->{}} return i<4} while condition() {int fail=1/(i-i) i=i+1} @println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn effectful_value_arguments_supply_progress_through_assignments_and_declarations() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn argument() int {checks=checks+1 @print("a",checks,"|") return 1}
fn next(int step) int {return step}
while i<3 {i=i+next(argument())}
fn advance() {int step=next(argument()) i=i+step}
while i<6 {advance()}
@println(i,":",checks)
"#,
        "a1|a2|a3|a4|a5|a6|6:6\n",
    );
}

#[test]
fn output_operands_and_assignment_indices_keep_their_snapshots() {
    folded(
        r#"
mut int i=0
mut int[] values=[0]
fn argument() int {i=i+1 @print("a|") return i}
fn key() uint {i=i+1 @print("k|") return 0u}
while i<4 {
    @println(i,":",argument(),":",i)
    values[key()]=i
}
@println(i,":",values)
"#,
        "a|0:1:1\nk|a|2:3:3\nk|4:[3]\n",
    );
}

#[test]
fn helper_loop_bounds_use_immutable_globals_and_captured_copies() {
    folded(
        r"
int limit=2
mut int i=0
fn advance() {mut int j=0 while j<limit {j=j+1 i=i+1}}
while i<4 {advance()}
fn calculate() int {
    mut int source=2
    int bound=source
    fn local_advance() {
        mut int j=0
        while j<bound {j=j+1 i=i+1}
        source=source+1
    }
    while i<8 {local_advance()}
    return source
}
@println((calculate(),i))
",
        "(4, 8)\n",
    );
}

#[test]
fn effectful_short_circuit_operands_do_not_become_unconditional_progress() {
    unproven(
        "mut int i=1 fn argument() bool {i=i+1 return true} fn ignore(bool value) {} while i<4 {int fail=1/(i-i) ignore(false and argument())} @println(i)",
    );
}

#[test]
fn test_mode_assertion_arguments_retain_effects_and_failure_locations() {
    let source = r#"
mut int calls=0
fn condition() bool {calls=calls+1 return true}
test "helper assertion" {
    mut int i=0
    while i<3 {
        assert condition()
        i=i+1
        int fail=1/(i-i)
    }
}
"#;
    let path = Path::new("helper-assertion.nc");
    assert!(ncc::compile_test_source_with_options(source, path, false).is_ok());
    let error = ncc::compile_test_source_with_options(source, path, true).unwrap_err();
    assert!(
        error
            .0
            .iter()
            .any(|diagnostic| { source.get(diagnostic.span.clone()) == Some("1/(i-i)") })
    );
}

#[test]
fn effectful_argument_failure_keeps_its_original_expression_location() {
    let source = "mut int i=0 mut int calls=0 fn argument() int {calls=calls+1 int fail=1/(calls-calls) return 1} fn advance(int step) {i=i+step} while i<3 {advance(argument())} @println(i)";
    assert!(compile(source, false).is_ok());
    let error = compile(source, true).unwrap_err();
    assert!(format!("{error:?}").contains("division by zero"));
    assert!(
        error
            .0
            .iter()
            .any(|diagnostic| { source.get(diagnostic.span.clone()) == Some("1/(calls-calls)") })
    );
}
