use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("ranking.nc"), release)
}

fn folded(source: &str, expected: &str) {
    assert!(
        !compile(source, true).unwrap().contains("} goto "),
        "{source}"
    );
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("ranking.nc");
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
        assert_eq!(output.stdout, expected.as_bytes(), "{source}");
    }
}

fn unproven(source: &str) {
    for release in [false, true] {
        // Reached arithmetic probes distinguish rejection from speculative entry.
        assert!(
            compile(source, release).unwrap().contains("} goto "),
            "{source}"
        );
    }
}

#[test]
fn converging_bounds_and_same_direction_bindings_fold() {
    for (body, expected) in [
        ("i=i+2;n=n+1", "20:20\n"),
        ("i=i-1;n=n-2", "-10:-10\n"),
        ("n=n-1", "0:0\n"),
        ("i=i+1;n=n-1", "5:5\n"),
    ] {
        folded(
            &format!("mut int i=0;mut int n=10;while i<n {{{body}}};@println(i,\":\",n)"),
            expected,
        );
    }
}

#[test]
fn comparisons_include_inclusive_and_exact_inequality_ranks() {
    for (operator, expected) in [("<", "15:15\n"), ("<=", "18:16\n"), ("!=", "15:15\n")] {
        folded(
            &format!(
                "mut int i=0;mut int n=10;while i{operator}n {{i=i+3;n=n+1}};@println(i,\":\",n)"
            ),
            expected,
        );
    }
    for (operator, expected) in [(">", "-5:-5\n"), (">=", "-8:-6\n"), ("!=", "-5:-5\n")] {
        folded(
            &format!(
                "mut int i=10;mut int n=0;while i{operator}n {{i=i-3;n=n-1}};@println(i,\":\",n)"
            ),
            expected,
        );
    }
}

#[test]
fn correlated_branch_progress_and_labeled_continues_fold() {
    folded(
        r#"
mut int i=0;mut int n=4
rows: while i!=n {
    if i%3==0 {true->{i=i+2;n=n+1;continue :rows} false->{i=i+11;n=n+10}}
}
@println(i,":",n)
"#,
        "26:26\n",
    );
}

#[test]
fn shared_helpers_and_bound_shadows_use_storage_identities() {
    folded(
        r#"
mut int i=0;mut int n=5
fn advance() {i=i+2;n=n+1}
fn calculate() int {
    mut int n=100
    while i<5 {advance();n=n-1}
    return n
}
@println(calculate(),":",i,":",n)
while i<n {int n=100;advance()}
@println(i,":",n)
"#,
        "97:6:8\n10:10\n",
    );
}

#[test]
fn fixed_condition_updates_preserve_first_final_and_break_checks() {
    folded(
        r#"
mut int i=0;mut int n=5;mut int checks=0
fn condition() bool {checks=checks+1;@print("c",checks,"|");n=n-1;return i<n}
while condition() {@print("b",i,"|");i=i+1}
@println(i,":",n,":",checks)
"#,
        "c1|b0|c2|b1|c3|2:2:3\n",
    );
    folded(
        r#"
mut int i=0;mut int n=10;mut int checks=0
fn condition() bool {checks=checks+1;i=i+2;n=n+1;return i<n}
while condition() {break}
@println(i,":",n,":",checks)
"#,
        "2:11:1\n",
    );
    folded(
        r#"
mut int i=0;mut int n=1
fn condition() bool {n=n-1;return i<n}
while condition() {i=i+1}
@println(i,":",n)
"#,
        "0:0\n",
    );
}

#[test]
fn diverging_reset_skipped_and_nonexact_ranks_stay_runtime() {
    for source in [
        "mut int i=1;mut int n=5;while i<n {int fail=1/(i-i);i=i+1;n=n+1};@println(i)",
        "mut int i=1;mut int n=5;while i<n {int fail=1/(i-i);i=i+1;n=n+2};@println(i)",
        "mut int i=1;mut int n=5;while i<n {int fail=1/(i-i);n=5;i=i+1};@println(i)",
        "mut int i=1;mut int n=5;while i<n {int fail=1/(i-i);if i%2==0 {true->{n=n-1} false->{continue}}};@println(i)",
        "mut int i=0;mut int n=9;while i!=n {int fail=1/(i-i);i=i+3;n=n+1};@println(i)",
        "mut int i=0;mut int n=5;fn condition() bool {n=n-1;return i<n};while condition(){int fail=1/(i-i);continue};@println(i)",
        "mut int i=0;mut int n=5;fn condition() bool {n=5;return i<n};while condition(){int fail=1/(i-i);i=i+1};@println(i)",
        "mut int i=0;mut int n=5;mut int j=0;while i<n {int fail=1/(i-i);while j!=5 {j=j+2};n=n-1};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn both_bindings_intermediate_and_exit_path_overflow_block_execution() {
    for source in [
        "mut byte i=250;mut byte n=254;while i<n {int fail=1/(@as(int,i)-@as(int,i));i=i+2;n=n+1};@println(i)",
        "mut byte i=0;mut byte n=1;while i<=n {int fail=1/(@as(int,i)-@as(int,i));n=n-1};@println(i)",
        "mut byte i=0;mut byte n=10;while i<n {int fail=1/(@as(int,i)-@as(int,i));n=n+250;n=n-251};@println(i)",
        "mut byte i=0;mut byte n=10;while i<n {int fail=1/(@as(int,i)-@as(int,i));if i==0 {true->{n=n+250;break} false->{i=i+2;n=n+1}}};@println(i)",
        "mut byte i=0;mut byte n=1;fn condition() bool {n=n-1;return i<=n};while condition(){int fail=1/(@as(int,i)-@as(int,i))};@println(i)",
        "mut byte i=0;mut byte n=0;fn condition() bool {n=n-1;return i<n};while condition(){};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn unsigned_limits_and_temporary_excursions_can_be_certified() {
    folded(
        r#"mut uint i=18446744073709551605;mut uint n=18446744073709551610
while i<n {i=i+2;n=n+1};@println(i,":",n)"#,
        "18446744073709551615:18446744073709551615\n",
    );
    folded(
        r#"mut byte i=0;mut byte n=10
while i<n {n=n+100;n=n-101};@println(i,":",n)"#,
        "0:0\n",
    );
    folded(
        r#"mut byte i=0;mut byte n=10
fn condition() bool {n=n+100;n=n-101;return i<n}
while condition() {};@println(i,":",n)"#,
        "0:0\n",
    );
}

#[test]
fn variable_closing_rank_strides_remain_finite() {
    folded(
        r#"mut int i=0;mut int n=10
while i<n {if i%2==0 {true->{i=i+3;n=n+1} false->{i=i+11;n=n+10}}}
@println(i,":",n)"#,
        "45:44\n",
    );
}

#[test]
fn ranking_failures_keep_original_expression_locations() {
    let source = "mut int i=0;mut int n=5;while i<n {int fail=1/(n-n);n=n-1};@println(i)";
    assert!(compile(source, false).is_ok());
    let error = compile(source, true).unwrap_err();
    assert!(format!("{error:?}").contains("division by zero"));
    assert!(
        error
            .0
            .iter()
            .any(|diagnostic| source.get(diagnostic.span.clone()) == Some("1/(n-n)"))
    );
}
