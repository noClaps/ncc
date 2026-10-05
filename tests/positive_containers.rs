use std::{fs, path::Path, process::Command};

const DECLARATIONS: &str = r"
struct Record { int[] values }
enum Choice { Empty Value(int[]) }
type Count = int
type Name = str
type Numbers = int[]
type Table = [str]int
";

#[test]
fn scalar_element_matrix_in_fixed_dynamic_arrays_maps_and_tuples() {
    for (kind, literal, runtime, replacement) in [
        ("bool", "true", "seed == 1", "false"),
        ("byte", "7", "octets[index]", "9"),
        ("int", "-7", "-seed - 6", "9"),
        ("uint", "7u", "@as(uint, seed + 6)", "9u"),
        ("float", "1.5", "@as(float, seed) + 0.5", "2.5"),
        ("char", "'a'", "@as(char, character_codes[index])", "'z'"),
        ("str", "\"ab\"", "@as(str, seed + 6)", "\"changed\""),
    ] {
        let expected = if kind == "str" { "\"7\"" } else { literal };
        element_matrix(kind, literal, runtime, replacement, expected);
    }
}

#[test]
fn composite_and_nominal_element_matrix_in_fixed_dynamic_arrays_maps_and_tuples() {
    for (kind, literal, runtime, replacement) in [
        ("int[]", "[7, 8]", "[seed + 6, 8]", "[9, 10]"),
        ("int[2]", "[7, 8]", "[seed + 6, 8]", "[9, 10]"),
        (
            "Table",
            "[\"x\": 7]",
            "@as(Table, [\"x\": seed + 6])",
            "[\"x\": 9]",
        ),
        (
            "(int, str)",
            "(7, \"x\")",
            "(seed + 6, \"x\")",
            "(9, \"y\")",
        ),
        (
            "Record",
            "Record{.values = [7]}",
            "Record{.values = [seed + 6]}",
            "Record{.values = [9]}",
        ),
        (
            "Choice",
            "Choice.Value([7])",
            "Choice.Value([seed + 6])",
            "Choice.Empty",
        ),
        ("Count", "7", "@as(Count, seed + 6)", "9"),
        ("Name", "\"7\"", "@as(Name, @as(str, seed + 6))", "\"nine\""),
        (
            "Numbers",
            "[7, 8]",
            "@as(Numbers, [seed + 6, 8])",
            "[9, 10]",
        ),
        ("int?", "7", "seed + 6", "none"),
        ("int[]?", "[7, 8]", "[seed + 6, 8]", "none"),
    ] {
        element_matrix(kind, literal, runtime, replacement, literal);
    }
}

fn element_matrix(kind: &str, literal: &str, runtime: &str, replacement: &str, expected: &str) {
    for (phase, initial, expected) in [
        ("constant", literal, literal),
        ("runtime", runtime, expected),
    ] {
        let source = format!(
            r#"{DECLARATIONS}
test "{phase} {kind} element matrix" {{
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    byte[] octets = [7, 8]
    byte[] character_codes = [97, 98]
    {kind} expected = {expected}
    {kind} changed = {replacement}
    mut {kind}[2] fixed = [{initial}, {initial}]
    mut {kind}[] dynamic = [{initial}, {initial}]
    mut [str]{kind} mapped = ["first": {initial}, "last": {initial},]
    mut ({kind}, {kind}) pair = ({initial}, {initial})
    assert fixed.len == 2 and dynamic.len == 2 and mapped.len == 2
    assert fixed[0] == expected and fixed[$] == expected
    assert dynamic[0] == expected and dynamic[$-1] == expected
    assert mapped["first"] == expected and mapped["last"] == expected
    assert pair[0] == expected and pair[1] == expected
    {kind}[2] fixed_copy = fixed
    {kind}[] dynamic_copy = dynamic
    [str]{kind} map_copy = mapped
    ({kind}, {kind}) pair_copy = pair
    {kind}[] converted = @as({kind}[], fixed)
    fixed[index] = changed
    dynamic[index] = changed
    mapped["first"] = changed
    mapped["new"] = changed
    pair[0] = changed
    assert fixed[index] == changed and fixed[1] == expected
    assert dynamic[index] == changed and dynamic[1] == expected
    assert mapped["first"] == changed and mapped["new"] == changed
    assert mapped["last"] == expected and mapped.len == 3
    assert pair[0] == changed and pair[1] == expected
    assert fixed_copy == [expected, expected]
    assert dynamic_copy == [expected, expected]
    assert map_copy == ["first": expected, "last": expected]
    assert pair_copy == (expected, expected)
    assert converted == [expected, expected]
    {kind}[4] joined = fixed_copy <> fixed_copy
    assert joined.len == 4 and joined[$] == expected
    {kind}[] grown = dynamic_copy <> converted
    assert grown.len == 4 and grown[$] == expected
    {kind} first, {kind} second = pair_copy
    assert first == expected and second == expected
    mut uint visits = 0
    for key in map_copy {{
        assert key == "first" or key == "last"
        assert map_copy[key] == expected
        visits = visits + 1
    }}
    assert visits == 2
    @println("checked")
}}
"#
        );
        success(&source, "checked\n");
    }
}

#[test]
fn nested_container_mutations_preserve_deep_copies() {
    success(
        r#"
struct Record { int[] values }
struct MapBox { [str]int[][] entries }
test "nested copies" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    mut Record[2] fixed = [Record{.values = [seed, 2]}, Record{.values = [3]}]
    Record[2] fixed_copy = fixed
    fixed[index].values[index] = 9
    assert fixed_copy[0].values == [1, 2]
    assert fixed[0].values == [9, 2] and fixed[1].values == [3]
    mut MapBox[] dynamic = [MapBox{.entries = ["x": [[seed, 2]]]}]
    MapBox[] dynamic_copy = dynamic
    dynamic[index].entries["x"][index][index] = 8
    assert dynamic_copy[0].entries["x"] == [[1, 2]]
    assert dynamic[0].entries["x"] == [[8, 2]]
    mut [int](int[], Record) mapped = [seed: ([seed], Record{.values = [2]})]
    [int](int[], Record) map_copy = mapped
    mapped[seed][0][index] = 7
    mapped[seed][1].values[index] = 6
    (int[], Record) original_entry = ([1], Record{.values = [2]})
    (int[], Record) changed_entry = ([7], Record{.values = [6]})
    assert map_copy[1] == original_entry
    assert mapped[1] == changed_entry
    mut (int[2], [str]int[], Record) tuple = ([seed, 2], ["x": [seed]], Record{.values = [seed]})
    (int[2], [str]int[], Record) tuple_copy = tuple
    tuple[0][index] = 5
    tuple[1]["x"][index] = 4
    tuple[2].values[index] = 3
    (int[2], [str]int[], Record) original_tuple = ([1, 2], ["x": [1]], Record{.values = [1]})
    (int[2], [str]int[], Record) changed_tuple = ([5, 2], ["x": [4]], Record{.values = [3]})
    assert tuple_copy == original_tuple
    assert tuple == changed_tuple
    int[2] head, ([str]int[], Record) tail = tuple_copy
    assert head == [1, 2]
    ([str]int[], Record) expected_tail = (["x": [1]], Record{.values = [1]})
    assert tail == expected_tail
    @println("deep copies checked")
}
"#,
        "deep copies checked\n",
    );
}

#[test]
fn nominal_alias_pairs_preserve_identity_through_calls_and_containers() {
    success(
        r#"
type Count = int
type Distance = int
type Name = str
type Title = str
type Counts = Count[]
fn echo(Count value) Count { return value }
fn echo_counts(Count[] values) Count[] { return values }
fn echo_nested([str](Count[2], Count[]) values) [str](Count[2], Count[]) { return values }
test "nominal identity" {
    Count constant = 7
    Distance other = 7
    assert echo(constant) == constant
    assert @as(int, constant) == @as(int, other)
    Name name = "same"
    Title title = "same"
    assert @as(str, name) == @as(str, title)
    Count value = @as(Count, @as(int, @args().len) + 6)
    Distance distance = @as(Distance, @as(int, value))
    assert echo(value) == constant and @as(int, distance) == 7
    mut Count[] values = echo_counts([value, constant])
    Counts wrapped = @as(Counts, values)
    Count[] unwrapped = @as(Count[], wrapped)
    [str](Count[2], Count[]) nested = echo_nested(["x": ([value, constant], values)])
    values[0] = 9
    assert unwrapped == [constant, constant]
    assert nested["x"][0] == [constant, constant]
    assert nested["x"][1] == [constant, constant]
    assert @as(int, values[0]) == 9
    @println("nominal identity checked")
}
"#,
        "nominal identity checked\n",
    );
}

#[test]
fn same_underlying_nominal_types_are_not_interchangeable() {
    for (underlying, initial) in [("int", "7"), ("str", "\"same\""), ("int[]", "[7]")] {
        for statement in [
            "B second = first",
            "mut B second = INITIAL; second = first",
            "take(first)",
            "fn wrong() B { return first }; _ = wrong()",
            "A[] source = [first]; B[] target = source",
            "A[1] source = [first]; B[1] target = source",
            "[str]A source = [\"x\": first]; [str]B target = source",
            "(int, A) source = (1, first); (int, B) target = source",
            "[str]A[][] source = [\"x\": [[first]]]; [str]B[][] target = source",
            "A[] source = [first]; mut B[] target = [INITIAL]; target[0] = source[0]",
            "A[1] source = [first]; take_fixed(source)",
            "[str](A[], int) source = [\"x\": ([first], 1)]; take_nested(source)",
        ] {
            let statement = statement.replace("INITIAL", initial);
            let source = format!(
                "type A = {underlying}\ntype B = {underlying}\n\
                 fn take(B value) {{}}\nfn take_fixed(B[1] values) {{}}\n\
                 fn take_nested([str](B[], int) values) {{}}\n\
                 test \"distinct aliases\" {{ A first = {initial}; {statement} }}\n"
            );
            rejects(&source, "expected");
        }
    }
}

#[test]
fn callable_elements_remain_callable_after_container_copies_and_updates() {
    success(
        r#"
fn increment(int value) int { return value + 1 }
fn double(int value) int { return value * 2 }
test "callable elements" {
    assert increment(3) == 4 and double(3) == 6
    int seed = @as(int, @args().len) + 2
    uint index = @args().len - 1
    mut (fn(int) int)[2] fixed = [increment, double]
    mut (fn(int) int)[] dynamic = [increment, double]
    mut [str](fn(int) int) mapped = ["first": increment, "last": double]
    mut ((fn(int) int), (fn(int) int)) tuple = (increment, double)
    (fn(int) int)[2] fixed_copy = fixed
    (fn(int) int)[] dynamic_copy = dynamic
    [str](fn(int) int) map_copy = mapped
    ((fn(int) int), (fn(int) int)) tuple_copy = tuple
    assert fixed[index](seed) == 4 and fixed[$](seed) == 6
    assert dynamic[index](seed) == 4 and dynamic[$](seed) == 6
    assert mapped["first"](seed) == 4 and mapped["last"](seed) == 6
    assert tuple[0](seed) == 4 and tuple[1](seed) == 6
    fixed[index] = double
    dynamic[index] = double
    mapped["first"] = double
    mapped["new"] = increment
    tuple[0] = double
    assert fixed[index](seed) == 6 and fixed_copy[index](seed) == 4
    assert dynamic[index](seed) == 6 and dynamic_copy[index](seed) == 4
    assert mapped["first"](seed) == 6 and map_copy["first"](seed) == 4
    assert tuple[0](seed) == 6 and tuple_copy[0](seed) == 4
    assert mapped["new"](seed) == 4 and mapped.len == 3
    (fn(int) int) first, (fn(int) int) second = tuple_copy
    assert first(seed) == 4 and second(seed) == 6
    @println("callables checked")
}
"#,
        "callables checked\n",
    );
}

#[test]
fn error_union_elements_preserve_success_failure_and_payload_copies() {
    success(
        r#"
fn result(bool fail, int value) int[]! {
    if fail { true -> { throw "failure" } false -> {} }
    return [value, 2]
}
test "error union elements" {
    mut str last_error = ""
    fn recover(int[]! value) int[] {
        return value catch message { last_error = @as(str, message); break [9] }
    }
    assert recover(result(false, 1)) == [1, 2]
    assert last_error == ""
    assert recover(result(true, 1)) == [9]
    assert last_error == "failure"
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    int[]! good = result(seed != 1, seed)
    int[]! bad = result(seed == 1, seed)
    mut int[]![2] fixed = [good, bad]
    mut int[]![] dynamic = [good, bad]
    mut [str]int[]! mapped = ["good": good, "bad": bad]
    mut (int[]!, int[]!) tuple = (good, bad)
    int[]![2] fixed_copy = fixed
    int[]![] dynamic_copy = dynamic
    [str]int[]! map_copy = mapped
    (int[]!, int[]!) tuple_copy = tuple
    assert recover(fixed[index]) == [1, 2] and recover(fixed[$]) == [9]
    assert recover(dynamic[index]) == [1, 2] and recover(dynamic[$]) == [9]
    assert recover(mapped["good"]) == [1, 2] and recover(mapped["bad"]) == [9]
    assert recover(tuple[0]) == [1, 2] and recover(tuple[1]) == [9]
    fixed[index] = bad
    dynamic[index] = bad
    mapped["good"] = bad
    tuple[0] = bad
    assert recover(fixed[index]) == [9] and recover(fixed_copy[index]) == [1, 2]
    assert recover(dynamic[index]) == [9] and recover(dynamic_copy[index]) == [1, 2]
    assert recover(mapped["good"]) == [9] and recover(map_copy["good"]) == [1, 2]
    assert recover(tuple[0]) == [9] and recover(tuple_copy[0]) == [1, 2]
    mut int[] payload = recover(fixed_copy[index])
    payload[index] = 7
    assert payload == [7, 2] and recover(fixed_copy[index]) == [1, 2]
    assert last_error == "failure"
    @println("error unions checked")
}
"#,
        "error unions checked\n",
    );
}

#[test]
fn void_elements_preserve_construction_and_access_effects() {
    success(
        r#"
test "void elements" {
    mut int effects = 0
    fn unit() { effects = effects + 1 }
    fn consume(void value) { _ = value; effects = effects + 10 }
    uint index = @args().len - 1
    mut void[2] fixed = [unit(), unit()]
    mut void[] dynamic = [unit(), unit()]
    mut [str]void mapped = ["first": unit(), "last": unit()]
    mut (void, void) tuple = (unit(), unit())
    assert effects == 8
    void[2] fixed_copy = fixed
    void[] dynamic_copy = dynamic
    [str]void map_copy = mapped
    (void, void) tuple_copy = tuple
    assert effects == 8
    fixed[index] = unit()
    dynamic[index] = unit()
    mapped["first"] = unit()
    tuple[0] = unit()
    assert effects == 12
    consume(fixed_copy[index])
    consume(dynamic_copy[index])
    consume(map_copy["first"])
    consume(tuple_copy[0])
    assert effects == 52
    assert fixed.len == 2 and dynamic.len == 2 and mapped.len == 2
    @println("void checked")
}
"#,
        "void checked\n",
    );
}

fn success(source: &str, stdout: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("containers.nc");
    fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            stdout,
            "release={release}"
        );
    }
}

fn rejects(source: &str, message: &str) {
    for release in [false, true] {
        let error =
            ncc::compile_test_source_with_options(source, Path::new("containers.nc"), release)
                .unwrap_err();
        assert!(
            error.to_string().contains(message),
            "release={release}: {error}\n{source}"
        );
    }
}
