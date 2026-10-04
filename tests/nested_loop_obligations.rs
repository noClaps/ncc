use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("nested.nc"), release)
}

fn folded(source: &str, expected: &str) {
    let generated = compile(source, true).unwrap();
    assert!(!generated.contains("} goto "), "{generated}");
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("nested.nc");
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
fn unproven_inner_loops_block_arithmetic_before_their_entry() {
    for source in [
        "mut int i=1;while i<3 {int fail=1/(i-i);while true {};i=i+1};@println(i)",
        "mut int i=1;fn condition() bool {i=i+1;return i<4};while condition() {int fail=1/(i-i);while true {}};@println(i)",
        "mut int i=1;for key in [1,2] {int fail=1/(i-i);while true {}};@println(i)",
        "mut int i=1;while i<3 {int fail=1/(i-i);if i>0 {true->{while true {}} false->{}};i=i+1};@println(i)",
        "mut int i=1;for key in [1,2] {int fail=1/(i-i);for inner in [1] {while true {}}};@println(i)",
        "mut int i=1;fn hidden() {while true {}};for key in [1,2] {int fail=1/(i-i);hidden()};@println(i)",
        "mut int i=1;fn iterable() int[] {int fail=1/(i-i);while i>0 {};return [1]};for key in iterable() {};@println(i)",
        "mut int i=1;fn hidden() {};for key in [1,2] {fn hidden() {while true {}};int fail=1/(i-i);hidden()};@println(i)",
        "mut int i=1;fn hidden() {};fn invoke((fn() void) hidden) {hidden()};fn spin() {while true {}};for key in [1,2] {int fail=1/(i-i);invoke(spin)};@println(i)",
        "mut int i=1;fn condition() bool {int fail=1/(i-i);while i>0 {};return i<3};while condition() {i=i+1};@println(i)",
        "mut int i=1;fn idle() {};fn spin() {while true {}};mut (fn() void) callback=idle;for key in [1,2] {int fail=1/(i-i);callback=spin;callback()};@println(i)",
        // The counter cannot borrow a shared initial value valid only at first entry.
        "mut int i=1;mut int j=0;while i<3 {int fail=1/(i-i);while j<3 {j=j+1};j=0;i=i+1};@println(i)",
        // Iterator shadowing must not reuse the known outer key as a local rank.
        "mut int key=0;mut int i=1;for key in [1,2] {int fail=1/(i-i);while key<3 {}};@println(i)",
    ] {
        for release in [false, true] {
            let generated = compile(source, release).unwrap();
            assert!(generated.contains("} goto "), "{source}");
        }
    }
}

#[test]
fn local_counters_are_certified_for_every_enclosing_entry() {
    folded(
        r#"
mut int i=0
while i<2 {
    mut int j=0
    while j<2 {
        mut int k=2
        while k>0 { @print(i,j,k,"|");k=k-1 }
        j=j+1
    }
    i=i+1
}
@println(i)
"#,
        "002|001|012|011|102|101|112|111|2\n",
    );
}

#[test]
fn for_traversals_and_fresh_iterator_bindings_preserve_output() {
    folded(
        r#"
mut int key=99
for key in [4,5,6] {
    mut uint j=key
    while j>0 { j=j-1 }
    @print(key,j,"|")
}
@println(key)
"#,
        "00|10|20|99\n",
    );
}

#[test]
fn strings_and_maps_keep_their_finite_traversal_keys() {
    folded(
        r#"
for key in "ab" {
    mut int j=0
    while j<2 { @print(key,j,"|");j=j+1 }
}
for key in ["x":7] {
    mut int j=0
    while j<2 { @print(key,j,"|");j=j+1 }
}
@println()
"#,
        "00|01|10|11|x0|x1|\n",
    );
}

#[test]
fn nested_local_and_outward_exits_remain_in_the_proof_tree() {
    folded(
        r#"
mut int i=0
rows: while i<3 {
    i=i+1
    mut int j=0
    inner: while j<3 {
        j=j+1
        if j==1 {true->{continue :inner} false->{}}
        @print(i,j,"|")
        if i==2 {true->{break :rows} false->{break :inner}}
    }
}
@println(i)
"#,
        "12|22|2\n",
    );
}

#[test]
fn condition_prefix_and_final_false_check_keep_their_effects() {
    folded(
        r#"
mut int i=0
mut int checks=0
fn condition() bool {
    mut int j=0
    while j<2 { checks=checks+1;j=j+1 }
    @print("c",checks,"|")
    return i<2
}
while condition() {
    mut int j=2
    while j>0 { @print(i,j,"|");j=j-1 }
    i=i+1
}
@println(i,":",checks)
"#,
        "c2|02|01|c4|12|11|c6|2:6\n",
    );
}

#[test]
fn fully_certified_nested_loops_still_diagnose_reached_failures() {
    let source =
        "mut int i=1;while i<3 {int fail=1/(i-i);mut int j=0;while j<2 {j=j+1};i=i+1};@println(i)";
    assert!(compile(source, false).is_ok());
    let errors = compile(source, true).unwrap_err();
    assert!(
        errors
            .0
            .iter()
            .any(|error| source.get(error.span.clone()) == Some("1/(i-i)"))
    );
}
