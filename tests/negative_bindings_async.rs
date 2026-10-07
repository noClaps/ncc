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

fn compiles(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("bindings_async_control.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {source}\n{error}"));
    }
}

#[test]
fn futures_cannot_be_returned_from_named_or_anonymous_functions() {
    for declaration in [
        "fn escape() fut int { return async one() }",
        "fn escape = fn() fut int { return async one() }",
        "fn outer() { fn escape() fut int { return async one() } }",
    ] {
        rejects(
            &format!("fn one() int {{ return 1 }};{declaration}"),
            "futures cannot be returned from functions",
        );
    }
}

fn mutex_paths() -> [(&'static str, &'static str, &'static str); 9] {
    [
        ("mutex int value = 1", "value", "2"),
        ("mutex int[] value = [1]", "value[0]", "2"),
        ("mutex int[1] value = [1]", "value[0]", "2"),
        ("mutex str value = \"a\"", "value[0]", "'b'"),
        ("mutex [str]int value = [\"a\": 1]", "value[\"a\"]", "2"),
        ("mutex (int, str) value = (1, \"a\")", "value[0]", "2"),
        (
            "struct Record { int n };mutex Record value = Record{.n = 1}",
            "value.n",
            "2",
        ),
        (
            "struct Record { int[] n };mutex Record value = Record{.n = [1]}",
            "value.n[0]",
            "2",
        ),
        ("mutex int[][] value = [[1]]", "value[0][0]", "2"),
    ]
}

#[test]
fn mutex_paths_cannot_be_read_or_written_without_a_lock_or_after_unlocking() {
    for (declaration, target, replacement) in mutex_paths() {
        for prefix in ["", "lock value {};"] {
            rejects(
                &format!("{declaration};{prefix}_ = {target}"),
                "cannot read mutex `value` outside a lock scope",
            );
            rejects(
                &format!("{declaration};{prefix}{target} = {replacement}"),
                "cannot mutate immutable `value`",
            );
        }
    }
}

#[test]
fn mutex_reads_in_value_operations_require_a_lock() {
    for body in [
        "int copy = value",
        "@println(value)",
        "str text = \"{value}\"",
        "str text = @as(str, value)",
        "bool same = value == 1",
        "int sum = value + 1",
        "fn take(int n) {};take(value)",
        "if value { 1 -> {} _ -> {} }",
        "if 1 { value -> {} _ -> {} }",
    ] {
        rejects(
            &format!("mutex int value = 1;{body}"),
            "cannot read mutex `value` outside a lock scope",
        );
        compiles(&format!("mutex int value = 1;lock value {{ {body} }}"));
    }
    for body in [
        "_ = value.len",
        "bool present = 1 in value",
        "for i in value {}",
        "int[] copy = value",
    ] {
        rejects(
            &format!("mutex int[] value = [1];{body}"),
            "cannot read mutex `value` outside a lock scope",
        );
        compiles(&format!("mutex int[] value = [1];lock value {{ {body} }}"));
    }
}

#[test]
fn every_function_requires_its_own_mutex_read_and_write_permission() {
    for (declaration, target, replacement) in mutex_paths() {
        for (body, expected) in [
            (
                format!("_ = {target}"),
                "cannot read mutex `value` outside a lock scope",
            ),
            (
                format!("{target} = {replacement}"),
                "cannot mutate immutable `value`",
            ),
        ] {
            for function in [
                format!("fn access() {{ {body} }}"),
                format!("fn access = fn() {{ {body} }}"),
                format!("fn outer() {{ lock value {{ fn inner() {{ {body} }} }} }}"),
                format!("fn outer = fn() {{ lock value {{ fn inner = fn() {{ {body} }} }} }}"),
            ] {
                for enclosing_lock in [false, true] {
                    let statement = if enclosing_lock {
                        format!("lock value {{ {function} }}")
                    } else {
                        function.clone()
                    };
                    rejects(&format!("{declaration};{statement}"), expected);
                }
            }
        }
    }
}

#[test]
fn locks_do_not_grant_access_to_other_mutexes_or_ordinary_bindings() {
    for body in ["_ = value", "value = 2"] {
        let expected = if body.starts_with('_') {
            "cannot read mutex `value` outside a lock scope"
        } else {
            "cannot mutate immutable `value`"
        };
        rejects(
            &format!("mutex int value = 1;mutex int other = 0;lock other {{ {body} }}"),
            expected,
        );
    }
    for declaration in [
        "int value = 1",
        "mut int value = 1",
        "int[] value = [1]",
        "mut int[] value = [1]",
    ] {
        rejects(
            &format!("{declaration};lock value {{}}"),
            "lock requires a mutex variable",
        );
    }
}

#[test]
fn explicit_locks_allow_mutex_paths_and_independently_locked_closures() {
    for (declaration, target, replacement) in mutex_paths() {
        let body = format!("_ = {target};{target} = {replacement};_ = {target}");
        compiles(&format!("{declaration};lock value {{ {{ {body} }} }}"));
        for function in [
            format!("fn access() {{ lock value {{ {body} }} }}"),
            format!("fn access = fn() {{ lock value {{ {body} }} }}"),
            format!("fn access() {{ lock value {{ fn inner() {{ lock value {{ {body} }} }} }} }}"),
            format!(
                "fn access = fn() {{ lock value {{ fn inner = fn() {{ lock value {{ {body} }} }} }} }}"
            ),
        ] {
            compiles(&format!("{declaration};{function};access()"));
        }
    }
    // Define under a lock, but invoke only after unlocking: capture the mutex,
    // not permission to access its payload from the defining scope.
    compiles(
        r"
mutex int value = 1
mut (fn() void) access = fn() {}
lock value {
    access = fn() { lock value { value = value + 1;_ = value } }
}
access()
fut void task = async access()
await task
",
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
    compiles(source);
}
