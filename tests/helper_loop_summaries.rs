//! End-to-end regressions for `optimizer::loops::summaries` integration.
use std::{path::Path, process::Command};

fn with_byte_domain(source: &str) -> String {
    let values = (0u8..=255)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("byte[] bounds = [{values}]\n{source}")
}

fn compile(source: &str, release: bool) -> String {
    ncc::compile_source_with_options(
        &with_byte_domain(source),
        Path::new("helper_loop_summaries.nc"),
        release,
    )
    .unwrap()
}

fn folded(source: &str, expected: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("helper_loop_summaries.nc");
    std::fs::write(&path, with_byte_domain(source)).unwrap();
    for release in [false, true] {
        let c = compile(source, release);
        if release {
            assert!(!c.contains("} goto "), "certified loops must fold");
        } else {
            assert!(c.contains("} goto "), "debug must retain source loops");
        }
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
        // The dependent division would diagnose speculative entry in release.
        // Never execute these deliberately unproven/unsafe loops.
        assert!(compile(source, release).contains("} goto "));
    }
}

#[test]
fn variable_byte_bounds_preserve_zero_trips_and_call_order() {
    folded(
        r#"
mut int i=0
fn advance() {
    byte bound=bounds[i]
    mut byte j=0
    while j<bound {j=j+1;i=i+1}
    i=i+1
}
while i<12 {advance();@print(i,"|")}
@println(i)
"#,
        "1|3|7|15|15\n",
    );
}

#[test]
fn parameter_snapshots_and_local_shadows_keep_distinct_storage() {
    folded(
        r"
mut int i=0
fn advance(byte original) {
    byte bound=original
    mut byte j=0
    while j<bound {j=j+1;i=i+1}
    {mut int i=100;i=i+1}
    i=i+1
}
while i<12 {advance(bounds[i])}
@println(i)
",
        "15\n",
    );
}

#[test]
fn inclusive_variable_bounds_guarantee_at_least_one_trip() {
    folded(
        r"
mut int i=0
fn advance() {
    byte original=bounds[i]
    int bound=@as(int,original)
    mut int j=0
    while j<=bound {j=j+1;i=i+1}
}
while i<12 {advance()}
@println(i)
",
        "15\n",
    );
}

#[test]
fn variable_initial_values_and_descending_summaries() {
    folded(
        r"
mut int i=0
fn advance() {
    mut byte j=bounds[i]
    while j>0 {j=j-1;i=i+1}
    i=i+1
}
while i<12 {advance()}
@println(i)
",
        "15\n",
    );
}

#[test]
fn nonuniform_monotone_paths_bound_progress_and_excursions() {
    folded(
        r"
mut int i=0
fn advance() {
    byte original=bounds[i]
    int bound=@as(int,original)
    mut int j=0
    while j<=bound {
        if j==0 {true->{j=j+1;i=i+1} false->{j=j+2;i=i+2}}
    }
}
while i<12 {advance()}
@println(i)
",
        "20\n",
    );
}

#[test]
fn negative_progress_can_supply_a_descending_outer_rank() {
    folded(
        r"
mut int i=15
fn advance() {
    byte original=bounds[i]
    int bound=@as(int,original)
    mut int j=0
    while j<=bound {j=j+2;i=i-1}
}
while i>0 {advance()}
@println(i)
",
        "0\n",
    );
}

#[test]
fn zero_trips_nonprogress_resets_and_changed_bounds_remain_unproven() {
    for helper in [
        "byte bound=bounds[i];mut byte j=0;while j<bound {j=j+1;i=i+1}",
        "byte original=bounds[i];int bound=@as(int,original);mut int j=0;while j<=bound {if j==0 {true->{i=i+1} false->{j=j+1}}}",
        "byte original=bounds[i];int bound=@as(int,original);mut int j=0;while j<=bound {j=0;j=j+1;i=i+1}",
        "byte original=bounds[i];mut int bound=@as(int,original);mut int j=0;while j<=bound {bound=bound+1;j=j+1;i=i+1}",
        "byte original=bounds[i];int bound=@as(int,original);mut int j=0;while j<=bound {j=j+1;i=i+2;i=i-1}",
        "byte original=bounds[i];int bound=@as(int,original);mut int j=0;while j<=bound {j=j+1;if j==1 {true->{return} false->{}};i=i+1}",
        "byte bound=bounds[i];mut byte j=0;while j!=bound {j=j+2;i=i+1};i=i+1",
        "byte bound=bounds[i];mut byte j=0;j=j+1;while j<bound {j=j+1;i=i+1};i=i+1",
        "byte bound=bounds[i];mut byte j=0;while j<bound {j=j+1;i=0};i=i+1",
    ] {
        unproven(&format!(
            "mut int i=0;fn advance() {{{helper}}};while i<4 {{int fail=1/(i-i);advance()}};@println(i)"
        ));
    }
}

#[test]
fn counter_and_protected_binding_overflow_cannot_hide_in_summaries() {
    for source in [
        "mut int i=0;fn advance() {byte bound=bounds[i];mut byte j=0;while j<=bound {j=j+1;i=i+1}};while i<4 {int fail=1/(i-i);advance()};@println(i)",
        "mut byte i=254;fn advance() {int bound=@as(int,i);mut int j=0;while j<=bound {j=j+1;i=i+1}};while i<255 {int fail=1/(@as(int,i)-@as(int,i));advance()};@println(i)",
        "mut int i=0;fn advance() {byte bound=bounds[i];mut byte j=0;while j<bound {j=j+2;i=i+1};i=i+1};while i<4 {int fail=1/(i-i);advance()};@println(i)",
    ] {
        unproven(source);
    }
}
