use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("compound.nc"), release)
}

fn folded(source: &str, expected: &str) {
    assert!(
        !compile(source, true).unwrap().contains("} goto "),
        "{source}"
    );
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("compound.nc");
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
        assert!(
            compile(source, release).unwrap().contains("} goto "),
            "{source}"
        );
    }
}

#[test]
fn calculated_limits_with_multiple_changing_bindings_fold() {
    folded(
        r#"
mut int i=0;mut int n=10;int extra=3
while i<2*n+extra {i=i+3;n=n+1}
@println(i,":",n)
"#,
        "69:33\n",
    );
    folded(
        r#"
mut int i=0;mut int n=3;mut int extra=4
while 2*i+1<3*n+extra {i=i+3;n=n+1;extra=extra+1}
@println(i,":",n,":",extra)
"#,
        "18:9:10\n",
    );
}

#[test]
fn known_steps_and_calculated_steps_are_constants_for_the_proof() {
    folded(
        r#"
mut int i=0;mut int n=10;int step=3
while i<n {i=i+step;n=n+1}
@println(i,":",n)
"#,
        "15:15\n",
    );
    folded(
        r#"
mut int i=0;mut int n=10;int step=2;int extra=1
while i<n {i=i+step+extra;n=n+1}
@println(i,":",n)
"#,
        "15:15\n",
    );
    folded(
        r#"
mut int i=0;mut int n=10;mut int step=3
while i<n {i=i+step;n=n+1}
@println(i,":",n,":",step)
"#,
        "15:15:3\n",
    );
}

#[test]
fn all_comparisons_work_with_calculated_limits() {
    for (operator, expected) in [("<", "12:6\n"), ("<=", "14:7\n"), ("!=", "12:6\n")] {
        folded(
            &format!(
                "mut int i=0;mut int n=0;while i{operator}n+6 {{i=i+2;n=n+1}};@println(i,\":\",n)"
            ),
            expected,
        );
    }
    for (operator, expected) in [(">", "-12:-6\n"), (">=", "-14:-7\n"), ("!=", "-12:-6\n")] {
        folded(
            &format!(
                "mut int i=0;mut int n=0;while i{operator}n-6 {{i=i-2;n=n-1}};@println(i,\":\",n)"
            ),
            expected,
        );
    }
}

#[test]
fn correlated_branches_continues_and_exiting_paths_preserve_effects() {
    folded(
        r#"
mut int i=0;mut int n=0
rows: while i<n+4 {
    @print(i,":",n,"|")
    if i%2==0 {true->{i=i+3;n=n+2;continue :rows} false->{i=i+10;n=n+9}}
}
@println(i,":",n)
"#,
        "0:0|3:2|13:11|23:20|33:29\n",
    );
    folded(
        r#"
mut int i=0;mut int n=3
while i<2*n+2 {if i==2 {true->{@print("exit|");break} false->{i=i+1;n=n-1}}}
@println(i,":",n)
"#,
        "exit|2:1\n",
    );
}

#[test]
fn shared_helpers_copied_constants_and_local_shadows_stay_distinct() {
    folded(
        r"
fn calculate() (int,int) {
    mut int i=0;mut int n=4
    int factor=2
    fn condition() bool {return i<factor*n+1}
    fn advance() {i=i+3;n=n+1}
    while condition() {int n=100;advance()}
    return (i,n)
}
@println(calculate())
",
        "(27, 13)\n",
    );
}

#[test]
fn calculated_condition_keeps_first_final_and_break_checks() {
    folded(
        r#"
mut int i=0;mut int n=4;mut int checks=0
fn condition() bool {checks=checks+1;@print("c",checks,"|");n=n-1;return i<2*n+1}
while condition() {@print("b",i,"|");i=i+1}
@println(i,":",n,":",checks)
"#,
        "c1|b0|c2|b1|c3|b2|c4|3:0:4\n",
    );
    folded(
        r#"
mut int i=0;mut int n=4
fn condition() bool {n=n-1;return i<2*n+1}
while condition() {i=i+1;break}
@println(i,":",n)
"#,
        "1:3\n",
    );
}

#[test]
fn calculated_limits_do_not_hide_nonprogress_resets_or_missing_arms() {
    for source in [
        "mut int i=0;mut int n=10;while i<2*n+3 {int fail=1/(i-i);i=i+2;n=n+1};@println(i)",
        "mut int i=0;mut int n=10;while i<2*n+3 {int fail=1/(i-i);i=0;n=n-1};@println(i)",
        "mut int i=0;mut int n=10;while i<2*n+3 {int fail=1/(i-i);if i>0 {true->{i=i+3;n=n+1} false->{}}};@println(i)",
        "mut int i=0;mut int n=10;mut int step=3;while i<n {int fail=1/(i-i);i=i+step;n=n+1;step=step-1};@println(i)",
        "mut int i=0;mut int n=0;while i!=n+5 {int fail=1/(i-i);i=i+3;n=n+1};@println(i)",
        "mut int i=0;mut int n=10;fn reset(){n=10};while i<2*n+3 {int fail=1/(i-i);reset();i=i+3};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn calculated_limit_intermediates_and_final_check_cannot_overflow() {
    for source in [
        "mut byte i=0;mut byte n=127;while i<2*n+1 {int fail=1/(@as(int,i)-@as(int,i));i=i+3;n=n+1};@println(i)",
        "mut byte i=0;mut byte n=250;while i<(n+10)-10 {int fail=1/(@as(int,i)-@as(int,i));i=i+1;n=n-1};@println(i)",
        "mut int i=0;mut int n=9223372036854775807;while i<2*n-n {int fail=1/(i-i);i=i+1;n=n-1};@println(i)",
        "mut byte i=0;mut byte n=127;while i<2*n {int fail=1/(@as(int,i)-@as(int,i));i=i+255;n=n+1};@println(i)",
        "mut int i=0;mut int n=5;mut int j=0;while i<2*n+1 {int fail=1/(i-i);while j!=5 {j=j+2};n=n-1};@println(i)",
    ] {
        unproven(source);
    }
}

#[test]
fn reached_arithmetic_errors_keep_their_source_location() {
    let source = "mut int i=0;mut int n=5;while i<2*n+1 {int fail=1/(n-n);i=i+3;n=n+1};@println(i)";
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

#[test]
fn resetting_secondary_counter_finishes_and_keeps_output_order() {
    folded(
        r#"
mut int rows=3;mut int columns=4
while rows>0 {
    @print(rows,":",columns,"|")
    if columns>0 {true->{columns=columns-1} false->{rows=rows-1;columns=4}}
}
@println(rows,":",columns)
"#,
        "3:4|3:3|3:2|3:1|3:0|2:4|2:3|2:2|2:1|2:0|1:4|1:3|1:2|1:1|1:0|0:4\n",
    );
}

#[test]
fn resetting_helpers_strides_and_increasing_counters_fold() {
    folded(
        r#"
mut byte rows=6;mut byte columns=4
byte step=2;byte width=4
fn advance() {if columns>=step {true->{columns=columns-step} false->{rows=rows-step;columns=width}}}
while rows>0 {advance()}
@println(rows,":",columns)
"#,
        "0:4\n",
    );
    folded(
        r#"
mut int rows=0;mut int columns=-4
rows_loop: while rows<3 {
    if columns<0 {true->{columns=columns+2;continue :rows_loop} false->{rows=rows+1;columns=-4}}
}
@println(rows,":",columns)
"#,
        "3:-4\n",
    );
}

#[test]
fn resets_do_not_hide_infinite_paths_or_overflow() {
    for source in [
        "mut int rows=3;mut int columns=4;while rows>0 {if columns>0 {true->{columns=columns-1;columns=4} false->{rows=rows-1;columns=4}}};@println(rows)",
        "mut int rows=3;mut int columns=4;while rows>0 {if columns>0 {true->{continue} false->{rows=rows-1;columns=4}}};@println(rows)",
        "mut int rows=3;mut int columns=4;while rows>0 {if columns>0 {true->{columns=columns-1} false->{rows=3;columns=4}}};@println(rows)",
        "mut byte rows=3;mut byte columns=4;while rows>=0 {if columns>0 {true->{columns=columns-1} false->{rows=rows-1;columns=4}}};@println(rows)",
        "mut byte rows=3;mut byte columns=4;while rows>0 {if columns>0 {true->{columns=columns-2} false->{rows=rows-1;columns=4}}};@println(rows)",
        "mut byte rows=3;mut byte columns=4;while rows>0 {if columns>0 {true->{columns=columns-1} false->{rows=rows-1;columns=255;columns=columns+1;columns=4}}};@println(rows)",
        "mut int rows=3;mut int columns=4;fn condition() bool {int fail=1/(rows-rows);return rows>0};while condition() {if columns>0 {true->{columns=columns-1} false->{rows=rows-1;columns=-(-(-9223372036854775808))}}};@println(rows)",
    ] {
        unproven(source);
    }
}

#[test]
fn multiplying_counter_catches_an_additive_limit() {
    folded(
        r#"
mut int i=1;mut int n=100
while i<n {@print(i,":",n,"|");i=i*2;n=n+1}
@println(i,":",n)
"#,
        "1:100|2:101|4:102|8:103|16:104|32:105|64:106|128:107\n",
    );
    folded(
        r#"
mut uint i=1;mut uint n=100
uint factor=3;uint step=2
fn advance() {i=factor*i;n=n+step}
while n>i {advance()}
@println(i,":",n)
"#,
        "243:110\n",
    );
    folded(
        r"
mut byte i=1
while i<=100 {i=i*2}
@println(i)
",
        "128\n",
    );
}

#[test]
fn growth_breaks_and_uniform_branch_updates_fold() {
    folded(
        r#"
mut int i=1;mut int n=100
outer: while i<n {
    if i<10 {true->{i=i*2;n=n+1;continue :outer} false->{n=n+1;i=i*2}}
}
@println(i,":",n)
"#,
        "128:107\n",
    );
    folded(
        r#"
mut int i=1;mut int n=100
while i<n {if i>8 {true->{@print("break|");break} false->{i=i*2}}}
@println(i,":",n)
"#,
        "break|16:100\n",
    );
}

#[test]
fn multiplication_requires_growth_and_checks_the_last_update() {
    for source in [
        "mut int i=0;mut int n=100;while i<n {i=i*2;n=n+1};@println(i)",
        "mut int i=-1;mut int n=100;while i<n {i=i*2;n=n+1};@println(i)",
        "mut int i=1;mut int n=100;while i<n {i=i*1;n=n+1};@println(i)",
        "mut int i=1;mut int n=100;while i<n {if i>1 {true->{i=i*2;n=n+1} false->{}}};@println(i)",
        "mut byte i=1;mut byte n=200;while i<n {i=i*2;n=n+1};@println(i)",
        "mut byte i=1;mut byte n=128;while i<=n {i=i*2};@println(i)",
        "mut byte i=1;mut byte n=100;while i<n {n=n+200;n=n-199;i=i*2};@println(i)",
        "mut int i=1;mut int n=100;fn condition() bool {int fail=1/(i-i);return i<n};while condition() {n=n-9223372036854775807;n=n+(-(-9223372036854775808));i=i*2};@println(i)",
        "mut uint i=1;mut uint n=100;fn condition() bool {uint fail=1u/(i-i);return i<n};while condition() {i=i*2u;n=n-(-1u)};@println(i)",
    ] {
        unproven(source);
    }
}
