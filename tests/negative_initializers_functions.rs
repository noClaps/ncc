use std::path::Path;

fn rejects(source: &str, expected: &str) {
    for release in [false, true] {
        let Err(error) = ncc::compile_source_with_options(
            source,
            Path::new("negative_initializers_functions.nc"),
            release,
        ) else {
            panic!(
                "release={release}, expected diagnostic containing {expected:?}, but compilation succeeded\nsource:\n{source}"
            );
        };
        assert!(
            error.to_string().contains(expected),
            "release={release}, expected diagnostic containing {expected:?}\nsource:\n{source}\ndiagnostic:\n{error}"
        );
    }
}

fn accepts(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(
            source,
            Path::new("initializers_functions_control.nc"),
            release,
        )
        .unwrap_or_else(|error| {
            panic!("release={release}\nsource:\n{source}\ndiagnostic:\n{error}")
        });
    }
}

#[test]
fn array_initializers_require_compatible_elements_at_every_depth() {
    for source in [
        "int[] values = [true]",
        "int[] values = [1, true]",
        "int[2] values = [1, true]",
        "int[][] values = [[1], [true]]",
        "int[2][] values = [[1, 2], [3, true]]",
        "fn bad() int[] { return [1, true] }",
        "fn take(int[] values) {};take([1, true])",
    ] {
        rejects(source, "expected `int`, found `bool`");
    }
    rejects("int[] values = [1, 2.5]", "expected `int`, found `float`");
    rejects("float[] values = [1.5, 2]", "expected `float`, found `int`");
    rejects("int[] values = [1, [2]]", "expected `int`, found `int[1]`");
}

#[test]
fn fixed_array_initializers_require_exact_lengths_including_nested_arrays() {
    for source in [
        "int[0] values = [1]",
        "int[1] values = []",
        "int[1] values = [1, 2]",
        "int[2] values = []",
        "int[2] values = [1]",
        "int[2] values = [1, 2, 3]",
        "int[2][] rows = [[1, 2], [3]]",
        "int[2][] rows = [[1, 2], [3, 4, 5]]",
        "int[2][2] rows = [[1, 2]]",
        "int[2][2] rows = [[1, 2], [3]]",
        "fn bad() int[2] { return [1] }",
        "fn take(int[2] values) {};take([1, 2, 3])",
    ] {
        rejects(
            source,
            "array literal length does not match fixed-size array type",
        );
    }
}

#[test]
fn map_initializers_check_each_key_and_value() {
    for source in [
        "[str]int values = [true: 1]",
        "[str]int values = [\"first\": 1, true: 2]",
        "fn bad() [str]int { return [true: 1] }",
        "fn take([str]int values) {};take([true: 1])",
    ] {
        rejects(source, "expected `str`, found `bool`");
    }
    for source in [
        "[str]int values = [\"first\": true]",
        "[str]int values = [\"first\": 1, \"last\": true]",
        "[str]int[] values = [\"first\": [1], \"last\": [true]]",
        "fn bad() [str]int { return [\"first\": true] }",
        "fn take([str]int values) {};take([\"first\": true])",
    ] {
        rejects(source, "expected `int`, found `bool`");
    }
    rejects(
        "[str]int[2] values = [\"first\": [1]]",
        "array literal length does not match fixed-size array type",
    );
    rejects(
        "[str]int values = [1, 2]",
        "expected `[str]int`, found `int[2]`",
    );
    rejects(
        "int[] values = [\"key\": 1]",
        "expected `int[]`, found `[str]int`",
    );
}

#[test]
fn tuple_initializers_require_declared_member_types_and_arity() {
    for (source, expected) in [
        (
            "(int, str) pair = (true, \"ok\")",
            "expected `int`, found `bool`",
        ),
        (
            "(int, str) pair = (1, false)",
            "expected `str`, found `bool`",
        ),
        (
            "(int, str) pair = (\"wrong\", 1)",
            "expected `int`, found `str`",
        ),
        (
            "(int, str) pair = (1, \"ok\", true)",
            "expected `(int, str)`, found `(int, str, bool)`",
        ),
        (
            "(int, str, bool) triple = (1, \"ok\")",
            "expected `(int, str, bool)`, found `(int, str)`",
        ),
        ("(int, str) pair = 1", "expected `(int, str)`, found `int`"),
        (
            "(int[], str) pair = ([true], \"ok\")",
            "expected `int`, found `bool`",
        ),
        (
            "(int, (str, bool)) pair = (1, (\"ok\", 2))",
            "expected `bool`, found `int`",
        ),
        (
            "int first, str second = (1, false)",
            "expected `str`, found `bool`",
        ),
        (
            "int first, str second = (1, \"ok\", true)",
            "expected `(int, str)`, found `(int, str, bool)`",
        ),
        (
            "fn bad() (int, str) { return 1, false }",
            "expected `str`, found `bool`",
        ),
        (
            "fn take((int, str) pair) {};take((1, false))",
            "expected `str`, found `bool`",
        ),
    ] {
        rejects(source, expected);
    }
}

#[test]
fn struct_initializers_check_field_names_types_and_nominal_identity() {
    for (initializer, expected) in [
        (
            "Record{.count = true, .name = \"ok\", .values = [1]}",
            "expected `int`, found `bool`",
        ),
        (
            "Record{.count = 1, .name = false, .values = [1]}",
            "expected `str`, found `bool`",
        ),
        (
            "Record{.count = 1, .name = \"ok\", .values = [true]}",
            "expected `int`, found `bool`",
        ),
        (
            "Record{.count = 1, .name = \"ok\", .values = [1], .extra = 2}",
            "unknown field `extra`",
        ),
    ] {
        let source = format!(
            "struct Record {{ int count str name int[] values }};Record value = {initializer}"
        );
        rejects(&source, expected);
    }
    rejects(
        "struct Inner { int count };struct Outer { Inner inner };Outer value = Outer{.inner = Inner{.count = false}}",
        "expected `int`, found `bool`",
    );
    rejects(
        "struct First { int count };struct Second { int count };First value = Second{.count = 1}",
        "expected `First`, found `Second`",
    );
    rejects(
        "struct Record { int count };fn bad() Record { return Record{.count = true} }",
        "expected `int`, found `bool`",
    );
}

#[test]
fn enum_initializers_require_matching_variants_payload_types_and_arity() {
    let declarations = "enum Choice { Empty Number(int) Pair(int, str) Numbers(int[]) };";
    for (initializer, expected) in [
        ("Choice.Missing", "unknown enum variant `Missing`"),
        ("Choice.Number(true)", "expected `int`, found `bool`"),
        ("Choice.Pair(1, false)", "expected `str`, found `bool`"),
        ("Choice.Numbers([1, true])", "expected `int`, found `bool`"),
        ("Choice.Number()", "incorrect number of arguments"),
        ("Choice.Number(1, 2)", "incorrect number of arguments"),
        ("Choice.Pair(1)", "incorrect number of arguments"),
        (
            "Choice.Pair(1, \"ok\", true)",
            "incorrect number of arguments",
        ),
        ("Choice.Empty(1)", "called value is not a function"),
    ] {
        rejects(
            &format!("{declarations}Choice value = {initializer}"),
            expected,
        );
    }
    rejects(
        "enum First { Empty };enum Second { Empty };First value = Second.Empty",
        "expected `First`, found `Second`",
    );
    rejects(
        "enum Choice { Number(int) };fn bad() Choice { return Choice.Number(true) }",
        "expected `int`, found `bool`",
    );
}

#[test]
fn function_signature_types_must_be_parenthesized_in_each_type_position() {
    for (source, expected) in [
        (
            "fn(int) int callback = fn(int value) int { return value }",
            "expected identifier",
        ),
        ("fn take(fn(int) int callback) {}", "expected identifier"),
        (
            "fn make() fn(int) int { return fn(int value) int { return value } }",
            "expected identifier",
        ),
        (
            "struct Holder { fn(int) int callback }",
            "expected identifier",
        ),
        (
            "enum Choice { Callback(fn(int) int) }",
            "expected identifier",
        ),
        ("type Callback = fn(int) int", "expected identifier"),
        (
            "(int, fn(int) int) pair = (1, fn(int value) int { return value })",
            "expected identifier",
        ),
        ("type Callbacks = [str]fn(int) int", "expected identifier"),
        ("type Pending = fut fn(int) int", "expected identifier"),
    ] {
        rejects(source, expected);
    }
}

#[test]
fn malformed_parenthesized_function_types_are_rejected() {
    for (source, expected) in [
        ("type Callback = (fn(int) int", "expected RParen"),
        ("type Callback = (fn int) int)", "expected LParen"),
        ("type Callback = (fn(int int) int)", "expected Comma"),
        ("type Callback = (fn(int) int extra)", "expected RParen"),
        ("type Callback = (fn(int) )", "expected identifier"),
        (
            "struct Holder { (fn(int)) callback }",
            "expected identifier",
        ),
        ("fn take((fn(int)) callback) {}", "expected identifier"),
    ] {
        rejects(source, expected);
    }
}

#[test]
fn value_returning_functions_require_return_annotations() {
    for source in [
        "fn bad() { return 1 }",
        "fn bad() { return 1, 2 }",
        "fn bad() { return [1, 2] }",
        "fn bad(bool choose) { if choose { true -> { return 1 } false -> {} } }",
        "fn outer() { fn bad() { return 1 } }",
        "fn callback = fn() { return 1 }",
        "(fn() int) callback = fn() { return 1 }",
    ] {
        rejects(source, "expected `void`, found");
    }
}

#[test]
fn valid_aggregate_initializers_and_function_type_positions_compile() {
    accepts(
        r#"
fn identity(int value) int { return value }
fn apply((fn(int) int) callback, int value) int { return callback(value) }
fn factory() (fn(int) int) { return identity }
fn notify() { return }
fn callback = fn() { return }
struct Record { int count str name int[] values }
struct Holder { (fn(int) int) callback }
enum Choice { Empty Number(int) Pair(int, str) Numbers(int[]) Callback((fn(int) int)) }
type Callback = (fn(int) int)
int[0] empty = []
int[2] fixed = [1, 2]
int[] dynamic = [1, 2, 3]
int[2][] rows = [[1, 2], [3, 4]]
[str]int[2] table = ["first": [1, 2], "last": [3, 4],]
(int, (str, bool)) nested = (1, ("ok", true))
int first, str second = (1, "ok")
Record record = Record{.name = "ok", .values = [1, 2], .count = 1}
Choice empty_choice = Choice.Empty
Choice number_choice = Choice.Number(1)
Choice pair_choice = Choice.Pair(1, "ok")
Choice numbers_choice = Choice.Numbers([1, 2])
(fn(int) int) operation = identity
Holder holder = Holder{.callback = operation}
Choice callable_choice = Choice.Callback(operation)
(fn(int) int)[] operations = [identity]
[str](fn(int) int) callbacks = ["id": identity]
(int, (fn(int) int)) pair = (1, identity)
(fn(int) int) returned = factory()
int answer = apply(returned, 1)
notify()
callback()
"#,
    );
}
