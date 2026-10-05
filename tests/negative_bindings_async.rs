use std::path::Path;

fn rejects(source: &str, expected: &str) {
    for release in [false, true] {
        let error = ncc::compile_source_with_options(
            source,
            Path::new("negative_bindings_async.nc"),
            release,
        )
        .expect_err(&format!("release={release}: {source}"));
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\nexpected {expected:?}: {error}"
        );
    }
}

#[test]
fn immutable_container_paths_cannot_be_mutated_directly_or_through_captures() {
    for (declaration, target, replacement) in [
        ("int[] value = [1]", "value[0]", "2"),
        ("int[1] value = [1]", "value[0]", "2"),
        ("str value = \"a\"", "value[0]", "'b'"),
        ("[str]int value = [\"a\": 1]", "value[\"a\"]", "2"),
        ("(int, str) value = (1, \"a\")", "value[0]", "2"),
        (
            "struct Record { int n };Record value = Record{.n = 1}",
            "value.n",
            "2",
        ),
        (
            "struct Record { int[] n };Record value = Record{.n = [1]}",
            "value.n[0]",
            "2",
        ),
        ("int[][] value = [[1]]", "value[0][0]", "2"),
    ] {
        for body in [
            format!("{target} = {replacement}"),
            format!("fn change() {{ {target} = {replacement} }}"),
            format!("fn change = fn() {{ {target} = {replacement} }}"),
        ] {
            rejects(
                &format!("{declaration};{body}"),
                "cannot mutate immutable `value`",
            );
        }
    }
}

#[test]
fn await_requires_a_future_not_an_ordinary_or_already_awaited_value() {
    for expression in ["1", "true", "\"text\"", "[1]", "(1, 2)", "one", "one()"] {
        rejects(
            &format!("fn one() int {{ return 1 }};_ = await {expression}"),
            "await expects a future",
        );
    }
    rejects(
        "fn one() int { return 1 };fut int task = async one();_ = await (await task)",
        "await expects a future",
    );
}

#[test]
fn async_requires_a_call_and_preserves_argument_and_result_types() {
    for expression in ["1", "true", "\"text\"", "one", "[1]"] {
        rejects(
            &format!("fn one() int {{ return 1 }};_ = async {expression}"),
            "async requires a function call",
        );
    }
    rejects(
        "fn one(int n) int { return n };fut int task = async one(true)",
        "expected `int`, found `bool`",
    );
    rejects(
        "fn one(int n) int { return n };fut int task = async one()",
        "incorrect number of arguments",
    );
    rejects(
        "fn one() int { return 1 };fut bool task = async one()",
        "expected `fut bool`, found `fut int`",
    );
}

#[test]
fn future_initializers_cannot_be_synchronous_values_or_other_handles() {
    for initializer in ["1", "one()", "task"] {
        rejects(
            &format!(
                "fn one() int {{ return 1 }};fut int task = async one();fut int other = {initializer}"
            ),
            "future must be initialized",
        );
    }
    rejects(
        "fn one() int { return 1 };mut fut int task = async one()",
        "futures cannot be mutable",
    );
}

#[test]
fn valid_mutable_paths_and_async_calls_still_compile() {
    let source = r"
struct Record { int[] n }
mut Record value = Record{.n = [1]}
fn change() { value.n[0] = 2 }
fn change_again = fn() { value.n[0] = 3 }
change()
change_again()
fn one(int n) int { return n }
fut int task = async one(1)
int result = await task
";
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("bindings_async_control.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}"));
    }
}
