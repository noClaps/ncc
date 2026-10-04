use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> String {
    ncc::compile_source_with_options(source, Path::new("helper-results.nc"), release).unwrap()
}

fn folded(source: &str, expected: &str) {
    assert!(
        !compile(source, true).contains("} goto "),
        "certified loops must disappear"
    );
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("helper-results.nc");
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
        // Do not run these loops. Reached division failures detect speculative
        // evaluation during compilation even when the loop happens to terminate.
        assert!(compile(source, release).contains("} goto "));
    }
}

#[test]
fn exhaustive_boolean_returns_and_direct_consumers_fold() {
    folded(
        "mut int i=0;fn step() int {if i==0 {true->{return 1} false->{return 2}}};while i<6 {int saved=step();i=i+saved};@println(i)",
        "7\n",
    );
}

#[test]
fn nonuniform_container_results_supply_exact_projection_steps() {
    for (ty, first, second, projection) in [
        ("int[]", "[1]", "[2]", "saved[0]"),
        ("(int,int)", "(1,9)", "(2,8)", "saved[0]"),
        ("[str]int", "[\"k\":1]", "[\"k\":2]", "saved[\"k\"]"),
        ("int[][]", "[[1]]", "[[2]]", "saved[0][0]"),
    ] {
        folded(
            &format!(
                "mut int i=0;fn steps() {ty} {{if i {{0->{{return {first}}} _->{{return {second}}}}}}};while i<6 {{{ty} saved=steps();i=i+{projection}}};@println(i)"
            ),
            "7\n",
        );
    }
}

#[test]
fn branch_return_steps_supply_progress_to_helper_consumers() {
    folded(
        r"
mut int i=0
fn step() int {if i {0->{return 1} _->{return 2}}}
fn advance() {i=i+step()}
while i<6 {advance()}
@println(i)
",
        "7\n",
    );
}

#[test]
fn declarations_and_nested_return_forwarding_keep_exact_values() {
    folded(
        r"
mut int i=0
fn step() int {if i {0->{return 1} _->{return 2}}}
fn forward() int {return step()}
fn advance() {int saved=forward();i=i+saved}
while i<5 {advance()}
@println(i)
",
        "5\n",
    );
}

#[test]
fn early_returns_preserve_effect_order_and_skip_later_effects() {
    folded(
        r#"
mut int i=0
mut int calls=0
fn step() int {
    calls=calls+1
    @print("a",i,"|")
    if i {0->{return 1} _->{}}
    @print("b|")
    return 2
}
fn advance() {i=i+step();@print("c",i,"|")}
while i<4 {advance()}
@println(i,":",calls)
"#,
        "a0|c1|a1|b|c3|a3|b|c5|5:3\n",
    );
}

#[test]
fn argument_snapshots_and_written_parameter_copies_are_distinct() {
    folded(
        r"
mut int i=0
fn step(int n) int {if n {0->{return 1} _->{return 2}}}
fn advance() {int saved=step(i);i=i+saved}
while i<4 {advance()}
@println(i)
",
        "5\n",
    );
    unproven(
        r"
mut int i=1
fn step(int saved) int {mut int copy=saved;copy=copy+1;if copy {2->{return copy} _->{return 1}}}
fn advance() {i=step(i)}
while i<4 {int fail=1/(i-i);advance()}
@println(i)
",
    );
}

#[test]
fn sibling_calls_and_later_arguments_preserve_each_sample() {
    folded(
        r#"
mut int i=0
mut int calls=0
fn step(int before, int ignored, int after) int {
    @print(before,":",after,"|")
    if before {0->{return 1} _->{return 2}}
}
fn argument() int {calls=calls+1;return calls}
fn advance() {
    int first=step(calls,argument(),calls)
    int second=step(calls,argument(),calls)
    i=i+first+second
}
while i<6 {advance()}
@println(i,":",calls)
"#,
        "0:1|1:2|2:3|3:4|7:4\n",
    );
}

#[test]
fn exact_return_arithmetic_and_lexical_shadows_supply_progress() {
    folded(
        r"
mut int i=0
fn step(int one, int two) int {
    if i {0->{int one=two;return one} _->{}}
    {int two=100}
    return one+one
}
fn advance() {int saved=step(1,2);i=i+saved}
while i<5 {advance()}
@println(i)
",
        "6\n",
    );
}

#[test]
fn zero_negative_and_reset_paths_cannot_manufacture_progress() {
    for source in [
        "mut int i=1;fn step() int {if i {1->{return 0} _->{return 2}}};fn advance() {i=i+step()};while i<4 {int fail=1/(i-i);advance()};@println(i)",
        "mut int i=1;fn step() int {if i {1->{return -1} _->{return 2}}};fn advance() {i=i+step()};while i<4 {int fail=1/(i-i);advance()};@println(i)",
        "mut int i=1;fn step() int {i=0;if i {0->{return 1} _->{return 2}}};fn advance() {i=i+step()};while i<4 {int fail=1/(i-i);advance()};@println(i)",
        "mut int i=1;fn step() int {if i {1->{while true {};return 1} _->{return 2}}};fn advance() {i=i+step()};while i<4 {int fail=1/(i-i);advance()};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn byte_steps_and_descending_integer_steps_fold() {
    folded(
        r"
mut byte i=0
fn step() byte {if i {0->{return 1} _->{return 2}}}
fn advance() {i=i+step()}
while i<5 {advance()}
@println(i)
",
        "5\n",
    );
    folded(
        r"
mut int i=6
fn step() int {if i {6->{return 1} _->{return 2}}}
fn advance() {i=i-step()}
while i>0 {advance()}
@println(i)
",
        "-1\n",
    );
}

#[test]
fn branch_excursions_and_final_updates_must_not_overflow() {
    for source in [
        "mut byte i=254;fn step() byte {if i {254->{return 2} _->{return 1}}};fn advance() {i=i+step()};while i<255 {int fail=1/(@as(int,i)-@as(int,i));advance()};@println(i)",
        "mut byte i=254;fn step() byte {if i {254->{i=i+2;return 1} _->{return 2}}};fn advance() {i=i+step()};while i<255 {int fail=1/(@as(int,i)-@as(int,i));advance()};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn return_site_samples_precede_later_mutations_and_output() {
    folded(
        r#"
mut int i=0
mut int value=10
fn sample() int {if i {0->{return value} _->{return value+1}}}
fn advance() {
    int saved=sample()
    value=value+10
    @println(saved,":",value)
    i=i+1
}
while i<2 {advance()}
@println(i)
"#,
        "10:20\n21:30\n2\n",
    );
}

#[test]
fn labeled_branch_exits_and_caller_continues_keep_their_scopes() {
    folded(
        r"
mut int i=0
fn step() int {
    inside: if i {0->{return 1} _->{break :inside}}
    return 2
}
fn advance() {i=i+step()}
inside: while i<4 {advance();continue :inside}
@println(i)
",
        "5\n",
    );
}

#[test]
fn samples_changed_by_later_effects_stay_storage_reads() {
    unproven(
        r"
mut int i=1
mut int value=1
fn step() int {if i {1->{return value} _->{return 2}}}
fn later() int {value=0;return 0}
fn advance() {i=i+step()+later()}
while i<4 {int fail=1/(i-i);advance()}
@println(i)
",
    );
}
