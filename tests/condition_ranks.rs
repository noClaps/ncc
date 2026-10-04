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
fn condition() bool {checks=checks+1;i=i+1;return i<5}
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
fn condition() bool {checks=checks+1;@print("a",i,":");increment();@print("b",i,"|");return i<6}
while condition() {@print("body",i,"|");i=i+1}
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
fn condition() bool {checks=checks+1;i=i-1;i=i-2;return i>0}
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
fn condition() bool {checks=checks+1;i=i+1;return i<10}
while condition() {i=i+2;break}
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
fn condition() bool {checks=checks+1;i=i+1;return i<255}
while condition() {}
@println(i,":",checks)
"#,
        "255:1\n",
    );
}

#[test]
fn first_and_final_condition_overflow_are_not_interpreted() {
    for source in [
        "mut byte i=255;fn condition() bool {i=i+1;return i<255};while condition(){};@println(i)",
        "mut byte i=253;fn condition() bool {i=i+1;return i<=255};while condition(){};@println(i)",
        "mut byte i=0;fn condition() bool {i=i-1;return i>0};while condition(){};@println(i)",
        "mut byte i=2;fn condition() bool {i=i-1;return i>=0};while condition(){};@println(i)",
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
        "mut int i=1;fn condition() bool {i=0;return i<3};while condition(){int fail=1/(i-i);i=i+1};@println(i)",
        "mut int i=1;fn condition() bool {i=i-1;i=i+2;return i<3};while condition(){int fail=1/(i-i)};@println(i)",
        "mut int i=0;fn condition() bool {i=i+2;return i!=5};while condition(){int fail=1/(i-i)};@println(i)",
        "mut int i=0;mut int n=5;fn condition() bool {i=i+1;n=n+2;return i<n};while condition(){int fail=1/(i-i)};@println(i)",
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
fn certified_continues_diagnose_reached_arithmetic_failures() {
    let source = "mut int i=1;fn condition() bool {i=i+1;return i<5};while condition(){int fail=1/(i-i);continue};@println(i)";
    assert!(compile(source, false).unwrap().contains("} goto "));
    let diagnostics = compile(source, true).unwrap_err();
    let error = diagnostics.to_string();
    assert!(error.contains("division by zero"), "{error}");
    let start = source.find("1/(i-i)").unwrap();
    assert_eq!(diagnostics.0[0].span, start..start + "1/(i-i)".len());
}

#[test]
fn conditional_prefixes_and_nested_continues_preserve_each_check() {
    folded(
        r#"
mut int i=0
mut int checks=0
mut bool alternate=false
fn increment() {
    if alternate {true -> {i=i+1} false -> {i=i+2}}
}
fn condition() bool {
    checks=checks+1
    alternate=not alternate
    @print("check",checks,":",i,"|")
    increment()
    return i<7
}
outer: while condition() {
    if i {1 -> {continue :outer} _ -> {}}
    mut int j=0
    inner: while j<2 {
        j=j+1
        @print("inner",i,":",j,"|")
        if i {3 -> {continue :outer} _ -> {continue :inner}}
    }
    @print("body",i,"|")
}
@println(i,":",checks)
"#,
        "check1:0|check2:1|inner3:1|check3:3|inner4:1|inner4:2|body4|check4:4|inner6:1|inner6:2|body6|check5:6|7:5\n",
    );
}

#[test]
fn optional_prefix_updates_combine_with_body_progress() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {
    checks=checks+1
    if i%2==0 {true -> {i=i+1} false -> {}}
    return i<6
}
while condition() {i=i+1;continue}
@println(i,":",checks)
"#,
        "7:4\n",
    );
}

#[test]
fn conditional_negative_and_exact_inequality_strides() {
    folded(
        r#"
mut int i=10
mut int checks=0
fn condition() bool {
    checks=checks+1
    if i%2==0 {true -> {i=i-1} false -> {i=i-2}}
    return i>0
}
while condition() {continue}
@println(i,":",checks)
"#,
        "-1:6\n",
    );
    folded(
        r"
mut uint i=0u
fn condition() bool {
    if i%4u==0u {true -> {i=i+1u;i=i+1u} false -> {i=i+2u}}
    return i!=10u
}
outer: while condition() {continue :outer}
@println(i)
",
        "10\n",
    );
}

#[test]
fn nested_for_jumps_preserve_outer_checks_and_breaks() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {checks=checks+1;i=i+1;return i<5}
outer: while condition() {
    for key in [10,20] {
        @print(i,":",key,"|")
        if i {1 -> {continue :outer} 3 -> {break :outer} _ -> {continue}}
    }
    @print("done|")
}
@println(i,":",checks)
"#,
        "1:0|2:0|2:1|done|3:0|3:3\n",
    );
}

#[test]
fn conditional_prefix_first_and_final_overflow_remain_runtime() {
    for source in [
        "mut byte i=254;fn condition() bool {if i%2==0 {true -> {i=i+2} false -> {i=i+1}};return i<255};while condition(){continue};@println(i)",
        "mut byte i=252;fn condition() bool {if i%2==0 {true -> {i=i+1} false -> {i=i+2}};return i<=255};while condition(){continue};@println(i)",
        "mut byte i=2;fn condition() bool {if i%2==0 {true -> {i=i-1} false -> {i=i-2}};return i>0};while condition(){continue};@println(i)",
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
fn missing_conditional_progress_and_nested_rank_mutations_are_not_entered() {
    for source in [
        "mut int i=0;fn condition() bool {if i%2==0 {true -> {i=i+1} false -> {}};return i<5};while condition(){int fail=1/(i-i);continue};@println(i)",
        "mut int i=0;fn condition() bool {if i%2==0 {true -> {i=i+1} false -> {i=i+2}};return i!=6};while condition(){int fail=1/(i-i);continue};@println(i)",
        "mut int i=0;fn condition() bool {i=i+1;return i<5};while condition(){int fail=1/(i-i);mut int j=0;while j<2 {j=j+1;i=0}};@println(i)",
        "mut int i=0;mut int n=5;fn condition() bool {i=i+1;return i<n};while condition(){int fail=1/(i-i);for key in [0] {n=n+1}};@println(i)",
        "mut int i=0;fn condition() bool {i=i+1;return i<5};while condition(){while true {}};@println(i)",
    ] {
        for release in [false, true] {
            assert!(
                compile(source, release).unwrap().contains("} goto "),
                "{source}"
            );
        }
    }
}
