use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("loops.nc"), release)
}

#[test]
fn counted_loops_precompute_beyond_the_former_budget() {
    let source = r#"
fn count(uint n) uint {
    mut uint i = 0
    while i < n { i = i + 1 }
    return i
}
fn reverse() int {
    mut int i = 100001
    while i >= 1 { i = i - 2 }
    return i
}
fn exact() int {
    mut int i = -9
    while i != 9 { i = i + 3 }
    return i
}
@println(count(100001), ":", reverse(), ":", exact())
"#;
    let c = compile(source, true).unwrap();
    for name in ["count", "reverse", "exact"] {
        assert!(!c.contains(&format!("nc_fn_{name}(")));
    }
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("loops.nc");
    std::fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"100001:-1:9\n");
    }
}

#[test]
fn unknown_loops_are_not_entered_to_search_for_progress() {
    for source in [
        // The bound is missed by the stride.
        "mut int i=1;while i!=4 {i=1/(i-i);i=i+2};@println(i)",
        // One branch has no progress, even though this particular input terminates.
        "mut int i=1;while i<4 {mut int fail=1/(i-i);if i<4 {true->{i=i+1} false->{}}};@println(i)",
        // A continue can bypass the increment.
        "mut int i=1;while i<4 {mut int fail=1/(i-i);if i==1 {true->{continue} false->{}};i=i+1};@println(i)",
        // The bound changes.
        "mut int i=1;mut int n=4;while i<n {mut int fail=1/(i-i);n=n+1;i=i+1};@println(i)",
        // Shared bound mutations through helpers must also block proof.
        "mut int i=1;mut int n=4;fn change(){n=n+1};while i<n {mut int fail=1/(i-i);change();i=i+1};@println(i)",
        // A closure can mutate shared induction storage.
        "mut int i=1;fn reset=fn() {i=0};while i<4 {mut int fail=1/(i-i);reset();i=i+1};@println(i)",
    ] {
        for release in [false, true] {
            let c = compile(source, release).unwrap();
            assert!(c.contains("} goto "), "unknown loop was removed: {source}");
        }
    }
}

#[test]
fn nondividing_stride_and_overflow_do_not_receive_certificates() {
    for source in [
        "mut uint i=1;while i!=4 {i=i+2};@println(i)",
        "mut uint i=18446744073709551614u;while i<=18446744073709551615u {i=i+1};@println(i)",
        "mut byte i=254;while i<255 {i=i+2};@println(i)",
        "mut int i=1;while i<4 {i=i-1};@println(i)",
    ] {
        assert!(compile(source, true).unwrap().contains("} goto "));
    }
}

#[test]
fn certified_loop_failures_keep_original_source_locations() {
    let source = "mut int i=1;while i<4 {mut int fail=1/(i-i);i=i+1};@println(i)";
    assert!(compile(source, false).is_ok());
    let error = compile(source, true).unwrap_err();
    assert!(format!("{error:?}").contains("division by zero"));
}

#[test]
fn finite_for_snapshots_have_no_iteration_budget() {
    let values = vec!["0"; 12000].join(",");
    let source = format!(
        "fn total() uint {{int[] values=[{values}];mut uint sum=0;for key in values {{sum=sum+1}};return sum}};@println(total())"
    );
    assert!(!compile(&source, true).unwrap().contains("nc_fn_total("));
}

#[test]
fn literal_false_conditions_do_not_require_body_progress() {
    let source = "mut int i=1;while false {i=1/(i-i)};@println(i)";
    assert!(!compile(source, true).unwrap().contains("} goto "));
}
