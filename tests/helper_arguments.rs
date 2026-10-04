use std::{path::Path, process::Command};

fn compile(source: &str, release: bool) -> String {
    ncc::compile_source_with_options(source, Path::new("helper-arguments.nc"), release).unwrap()
}

fn compare(source: &str, expected: &str, folds: bool) {
    if folds {
        assert!(
            !compile(source, true).contains("} goto "),
            "certified loops must disappear"
        );
    }
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("helper-arguments.nc");
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
    // Never run probes: compilation must not speculatively reach their failures.
    for release in [false, true] {
        assert!(compile(source, release).contains("} goto "));
    }
}

#[test]
fn literal_container_projections_supply_ranks() {
    for (ty, argument, projection) in [
        ("int[]", "[1,2]", "steps[0]"),
        ("int[2]", "[1,2]", "steps[0]"),
        ("(int,int)", "(1,2)", "steps[0]"),
        ("[str]int", "[\"step\":1]", "steps[\"step\"]"),
        ("int[][]", "[[1,2],[3]]", "steps[0][0]"),
        (
            "([str]int,int[])",
            "([\"step\":1],[2])",
            "steps[0][\"step\"]",
        ),
    ] {
        compare(
            &format!(
                "mut int i=0;fn advance({ty} steps) {{i=i+{projection}}};while i<3 {{advance({argument})}};@println(i)"
            ),
            "3\n",
            true,
        );
    }
}

#[test]
fn immutable_container_copies_and_uniform_returns_supply_ranks() {
    compare(
        r"
mut int i=0
fn steps() int[] {return [1,2]}
fn advance() {int[] copy=steps();i=i+copy[0]}
while i<3 {advance()}
@println(i)
",
        "3\n",
        true,
    );
}

#[test]
fn aggregate_calls_are_left_to_right_and_snapshot_prior_values() {
    compare(
        r#"
mut int i=0
mut int calls=0
fn next() int {calls=calls+1;@print("n",calls,"|");return calls}
fn use(int[] values, (int,int) pair, [int]int map) {
    @print(values,":",pair,":",map[calls],"|")
    i=i+1
}
while i<2 {use([calls,next(),calls],(calls,next()),[next():calls])}
@println(i)
"#,
        "n1|n2|n3|[0, 1, 1]:(1, 2):3|n4|n5|n6|[3, 4, 4]:(4, 5):6|2\n",
        false,
    );
}

#[test]
fn container_arguments_copy_before_later_mutations() {
    compare(
        r#"
mut int i=0
mut int[] array=[1]
mut (int,int) tuple=(1,2)
mut [str]int map=["k":1]
mut str text="a"
fn change() int {array[0]=9;tuple[0]=9;map["k"]=9;text[0]='z';return 0}
fn use(int[] a,(int,int) t,[str]int m,str s,int ignored) {
    @print(a[0],t[0],m["k"],s,"|")
    i=i+1
}
while i<2 {use(array,tuple,map,text,change())}
@println(array[0],tuple[0],map["k"],text)
"#,
        "111a|999z|999z\n",
        true,
    );
}

#[test]
fn nested_container_operands_copy_before_sibling_effects() {
    compare(
        r#"
mut int i=0
mut int[] values=[1]
fn change() int {values[0]=9;return 2}
fn use((int[][],int) argument) {@print(argument[0][0][0],"|");i=i+1}
while i<2 {use(([values],change()))}
@println(values[0])
"#,
        "1|9|9\n",
        true,
    );
}

#[test]
fn known_shortcuts_skip_unreachable_calls() {
    compare(
        r#"
mut int i=0
bool disabled=false
fn unreachable() bool {i=0;@print("wrong");return true}
fn next() bool {i=i+1;return true}
fn advance() {
    bool a=disabled and unreachable()
    bool b=(not disabled) or unreachable()
    bool c=(disabled==false) and next()
    bool d=false or false
    @print(a,b,c,d,"|")
}
while i<2 {advance()}
@println(i)
"#,
        "falsetruetruefalse|falsetruetruefalse|2\n",
        true,
    );
}

#[test]
fn unknown_shortcuts_keep_rhs_effects_guarded_and_exact_results() {
    compare(
        r#"
mut int i=0
mut int calls=0
fn rhs() bool {calls=calls+1;@print("r|");return false}
fn use(bool a,bool b) {@print(a,b,"|");i=i+1}
while i<3 {use(i==0 and rhs(),i==1 or rhs())}
@println(i,":",calls)
"#,
        "r|r|falsefalse|falsetrue|r|falsefalse|3:3\n",
        true,
    );
}

#[test]
fn closed_projections_of_nonliteral_copies_and_helper_loops_fold() {
    compare(
        "mut int i=0;fn advance() {int[] steps=[1,i];int[] copy=steps;i=i+copy[0]};while i<3 {advance()};@println(i)",
        "3\n",
        true,
    );
    compare(
        r#"
mut int i=0
fn next() int {@print("n|");return 1}
fn use(int[] steps) {int[] copy=steps;mut int j=0;while j<2 {j=j+1;i=i+copy[0]}}
while i<4 {use([next(),i])}
@println(i)
"#,
        "n|n|4\n",
        true,
    );
    compare(
        r#"
mut int i=0
fn use((int,[str]int) steps) {(int,[str]int) copy=steps;i=i+copy[1]["k"]}
while i<3 {use((i,["k":1]))}
@println(i)
"#,
        "3\n",
        true,
    );
}

#[test]
fn literal_string_lengths_and_characters_use_grapheme_boundaries() {
    compare(
        "mut uint i=0;fn use(str text) {i=i+text.len;@print(text[0],\"|\")};while i<4 {use(\"a\\u{301}b\")};@println(i)",
        "a\u{301}|a\u{301}|4\n",
        true,
    );
}

#[test]
fn index_effects_replacing_the_read_object_remain_a_proof_barrier() {
    unproven(
        r#"
mut int i=0
mut int[][] values=[[1],[2]]
fn choose() int {values[0]=[9];return 1}
fn read() int {values[0]=[7];return 0}
fn use(int[] copy) {@print(copy[0],"|");i=i+1}
while i<2 {int probe=1/(i-i);use(values[read()]);values[choose()]=values[0]}
@println(values[0][0],":",values[1][0])
"#,
    );
}

#[test]
fn assignment_rhs_containers_copy_before_target_index_effects() {
    compare(
        r#"
mut int i=0
mut int[][] values=[[1],[2]]
fn choose() int {values[0]=[9];return 1}
fn advance() {values[choose()]=values[0];i=i+1}
while i<2 {advance();@print(values[1][0],"|")}
@println(values[0][0],":",values[1][0])
"#,
        "1|9|9:9\n",
        true,
    );
}

#[test]
fn map_duplicate_literal_keys_take_the_final_value() {
    compare(
        "mut int i=0;fn use([int]int steps) {i=i+steps[1]};while i<4 {use([1:0,1:2])};@println(i)",
        "4\n",
        true,
    );
}

#[test]
fn copied_strings_keep_explicit_character_boundaries() {
    compare(
        "mut int i=0;mut str text=\"a\" <> \"\\u{301}\";fn change() int {text[0]='z';return 0};fn use(str saved,int ignored) {@print(saved.len,\":\",saved[0],\"|\");i=i+1};while i<2 {use(text,change())};@println(text.len)",
        "2:a|2:z|2\n",
        true,
    );
}

#[test]
fn callable_container_arguments_keep_alias_barriers() {
    unproven(
        "mut int i=0;fn next() {i=i+1};fn use(((fn() void),int) values) {values[0]()};while i<3 {int probe=1/(i-i);use((next,1))};@println(i)",
    );
}

#[test]
fn recursive_unreachable_rhs_retains_runtime_order_without_execution() {
    // The broader recursive-call graph is conservative even for these known
    // shortcuts; argument expansion itself must never enter the recursive RHS.
    compare(
        "mut int i=0;fn recursive() bool {return recursive()};fn use(bool value) {i=i+1};while i<2 {use(false and recursive())};@println(i)",
        "2\n",
        false,
    );
}

#[test]
fn conditional_only_progress_and_modified_container_steps_are_unproven() {
    for source in [
        "mut int i=0;fn next() bool {i=i+1;return true};fn use(bool b) {};while i<3 {int probe=1/(i-i);use(i>0 and next())};@println(i)",
        "mut int i=0;mut bool enabled=true;fn next() bool {enabled=false;i=i+1;return true};fn use(bool b) {};while i<3 {int probe=1/(i-i);use(enabled and next())};@println(i)",
        "mut int i=0;fn next() bool {i=i+1;return true};fn use(bool b) {};while i<3 {int probe=1/(i-i);use(i==0 or next())};@println(i)",
        "mut int i=0;fn use(int[] steps) {mut int[] copy=steps;copy[0]=0;i=i+copy[0]};while i<3 {int probe=1/(i-i);use([1])};@println(i)",
        "mut int i=0;mutex int[] values=[1];fn use() {lock values {i=i+values[0]}};while i<3 {int probe=1/(i-i);use()};@println(i)",
        "mut int i=0;mut int[] steps=[1];fn zero() int {steps[0]=0;return 0};fn use(int[] a,int ignored) {i=i+a[0]};while i<3 {int probe=1/(i-i);use(steps,zero())};@println(i)",
    ] {
        unproven(source);
    }
}
