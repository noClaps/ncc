use std::{fs, path::Path, process::Command};

#[test]
fn exact_vec2_vec3_field_specializations() {
    success(
        r#"
struct Vec2 {
    int x
    int y
}
struct Vec3 {
    int x
    int y
    int z
}
fn get_x<type T>(T val) int {
    return val.x
}
test "getting a struct field with a generic function" {
    Vec2 v = Vec2{.x = 3, .y = 4}
    assert get_x<Vec2>(v) == 3
    @println(get_x<Vec2>(v))

    Vec3 v = Vec3{.x = 5, .y = 12, .z = 13}
    assert get_x<Vec3>(v) == 5
    @println(get_x<Vec3>(v))

    int seed = @as(int, @args().len)
    Vec2 runtime2 = Vec2{.x = seed + 2, .y = 4}
    Vec3 runtime3 = Vec3{.x = seed + 4, .y = 12, .z = 13}
    assert get_x<Vec2>(runtime2) == 3
    assert get_x<Vec3>(runtime3) == 5
    assert runtime2.y == 4
    assert runtime3.y == 12 and runtime3.z == 13
    @println(get_x<Vec2>(runtime2), ":", get_x<Vec3>(runtime3))
}
"#,
        b"3\n5\n3:5\n",
    );
}

#[test]
fn exact_matrix_generic_struct_example() {
    success(
        r#"
struct Matrix<type T> {
    uint rows
    uint cols
    T[] data
}
test "Matrix int specialization" {
    Matrix<int> mat = Matrix<int>{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
    assert mat.rows == 2
    assert mat.cols == 2
    assert mat.data == [1, 0, 0, 1]
    assert mat.data.len == mat.rows * mat.cols
    @println(mat.rows, ":", mat.cols, ":", mat.data)

    int seed = @as(int, @args().len)
    mut Matrix<int> runtime = Matrix<int>{.rows = 2, .cols = 2, .data = [seed, 0, 0, seed]}
    Matrix<int> snapshot = runtime
    runtime.data[0] = 9
    assert runtime.data == [9, 0, 0, 1]
    assert snapshot.rows == 2 and snapshot.cols == 2
    assert snapshot.data == [1, 0, 0, 1]
    assert mat.data == [1, 0, 0, 1]
    @println(snapshot.data, ":", runtime.data)
}
"#,
        b"2:2:[1, 0, 0, 1]\n[1, 0, 0, 1]:[9, 0, 0, 1]\n",
    );
}

#[test]
fn directly_imported_exported_nominal_alias() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("counts.nc"),
        r"
pub type Count = int
pub Count INITIAL = 7
pub fn echo(Count value) Count { return value }
pub fn next(Count value) Count {
    return @as(Count, @as(int, value) + 1)
}
",
    )
    .unwrap();
    fs::write(
        &input,
        r#"
import { "counts" as counts }
fn local_echo(counts.Count value) counts.Count { return value }
test "direct exported nominal alias" {
    counts.Count literal = 7
    counts.Count exported = counts.INITIAL
    assert literal == exported
    assert counts.echo(literal) == literal
    assert local_echo(exported) == literal

    int raw = @as(int, @args().len) + 6
    counts.Count converted = @as(counts.Count, raw)
    counts.Count result = counts.next(local_echo(counts.echo(converted)))
    assert converted == literal
    assert @as(int, result) == 8
    assert @as(int, exported) == 7
    @println(@as(int, literal), ":", @as(int, converted), ":", @as(int, result))
}
"#,
    )
    .unwrap();
    run_both(&input, b"7:7:8\n");
}

#[test]
fn single_import_repeated_private_module_state_mutation() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("counter.nc"),
        r"
mut int state = 10
fn change(int delta) { state = state + delta }
pub fn advance(int delta) int {
    change(delta)
    return state
}
pub fn current() int { return state }
",
    )
    .unwrap();
    fs::write(
        &input,
        r#"
import { "counter" as counter }
test "one import retains private mutable state" {
    assert counter.current() == 10
    int step = @as(int, @args().len)
    int first = counter.advance(step + 1)
    assert first == 12
    assert counter.current() == 12
    int second = counter.advance(-step)
    assert second == 11
    assert counter.current() == 11
    int third = counter.advance(step + 3)
    assert third == 15
    assert counter.current() == 15
    assert first == 12 and second == 11
    @println(first, ":", second, ":", third, ":", counter.current())
}
"#,
    )
    .unwrap();
    run_both(&input, b"12:11:15:15\n");
}

#[test]
fn public_nc_wrapper_calls_private_c_extern() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        directory.path().join("native.c"),
        r"
nc_abi_adjust_result adjust(nc_abi_adjust_arg0 value) { return value * 3 + 1; }
",
    )
    .unwrap();
    fs::write(
        directory.path().join("adapter.nc"),
        r#"
extern "native.c" as raw {
    fn adjust(int value) int = "adjust"
}
pub fn adjusted(int value) int {
    return raw.adjust(value + 2) - 1
}
"#,
    )
    .unwrap();
    fs::write(
        &input,
        r#"
import { "adapter" as adapter }
test "public NC contract around private C extern" {
    int seed = @as(int, @args().len)
    int first = adapter.adjusted(seed)
    int second = adapter.adjusted(seed + 1)
    int third = adapter.adjusted(-seed)
    assert first == 9
    assert second == 12
    assert third == 3
    @println(first, ":", second, ":", third)
}
"#,
    )
    .unwrap();
    run_both(&input, b"9:12:3\n");
}

#[test]
fn generic_function_type_forwarding_preserves_nominal_container_specializations() {
    for (kind, constant, runtime) in [
        ("Count", "7", "@as(Count, seed + 6)"),
        ("Distance", "7", "@as(Distance, seed + 6)"),
        ("Name", "\"7\"", "@as(Name, @as(str, seed + 6))"),
        ("Count[2][]", "[[7, 8]]", "[[value, 8]]"),
        ("Count[][2]", "[[7], [8, 9]]", "[[value], [8, 9]]"),
        (
            "[str](Count[2], Count[])",
            "[\"x\": ([7, 8], [7])]",
            "[\"x\": ([value, 8], [value])]",
        ),
        ("Count[]?", "[7, 8]", "[value, 8]"),
        ("Count?[]", "[7, none]", "[value, none]"),
    ] {
        let source = format!(
            r#"
type Count = int
type Distance = int
type Name = str
fn identity<type T>(T value) T {{ return value }}
fn forward<type T>(T value) T {{ return identity<T>(value) }}
fn pair<type T>(T value) (T, T) {{ return forward<T>(value), identity<T>(value) }}
fn apply<type T, type U>(T value, (fn(T) U) callback) U {{ return callback(forward<T>(value)) }}
fn relay<type T, type U>(T value, (fn(T) U) callback) U {{ return apply<T, U>(value, callback) }}
test "forward {kind}" {{
    int seed = @as(int, @args().len)
    Count value = @as(Count, seed + 6)
    {kind} expected = {constant}
    {kind} known = forward<{kind}>({constant})
    {kind} input = {runtime}
    {kind} result = forward<{kind}>(input)
    assert known == expected and result == expected
    {kind} left, {kind} right = pair<{kind}>(input)
    assert left == expected and right == expected
    (fn({kind}) {kind}) callback = fn({kind} candidate) {kind} {{ return forward<{kind}>(candidate) }}
    assert relay<{kind}, {kind}>(input, callback) == expected
    (fn({kind}) bool) compare = fn({kind} candidate) bool {{ return candidate == expected }}
    assert relay<{kind}, bool>(known, compare)
    assert relay<{kind}, bool>(input, compare)

    @println("forwarded")
}}
"#
        );
        success(&source, b"forwarded\n");
    }
}

#[test]
fn generic_struct_nested_fixed_dynamic_nominal_arrays_preserve_deep_copies() {
    for initial in ["7", "@as(Count, seed + 6)"] {
        let source = format!(
            r#"
type Count = int
struct Grid<type T> {{ T[2][] rows T[][2] columns }}
struct Box<type T> {{ T value }}
fn wrap<type T>(T value) Box<T> {{ return Box<T>{{.value = value}} }}
fn forward<type T>(T value) Box<T> {{ return wrap<T>(value) }}
fn replace_first<type T>(Grid<T> grid, T replacement) Grid<T> {{
    mut Grid<T> local = grid
    local.rows[0][0] = replacement
    local.columns[0][0] = replacement
    return local
}}
test "nested generic array copies" {{
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    Count value = {initial}
    Grid<Count> original = Grid<Count>{{.rows = [[value, 8], [9, 10]], .columns = [[value], [8, 9]]}}
    mut Box<Grid<Count>> boxed = forward<Grid<Count>>(original)
    Box<Grid<Count>> snapshot = boxed
    Grid<Count> changed = replace_first<Count>(boxed.value, 11)
    assert @as(int, changed.rows[0][0]) == 11
    assert @as(int, changed.columns[0][0]) == 11
    assert changed.rows[1] == original.rows[1]
    assert changed.columns[1] == original.columns[1]
    boxed.value.rows[index][index] = 12
    boxed.value.columns[index][index] = 13
    Count[2][] extra_rows = [[14, 15]]
    Count[] extra_column = [16]
    boxed.value.rows = boxed.value.rows <> extra_rows
    boxed.value.columns[1] = boxed.value.columns[1] <> extra_column
    assert snapshot.value == original
    Count[2][] expected_rows = [[value, 8], [9, 10]]
    Count[][2] expected_columns = [[value], [8, 9]]
    assert original.rows == expected_rows
    assert original.columns == expected_columns
    assert @as(int, boxed.value.rows[0][0]) == 12
    assert @as(int, boxed.value.columns[0][0]) == 13
    assert boxed.value.rows.len == 3 and snapshot.value.rows.len == 2
    Count[] grown_column = [8, 9, 16]
    Count[] unchanged_column = [8, 9]
    assert boxed.value.columns[1] == grown_column
    assert snapshot.value.columns[1] == unchanged_column
    @println("array copies checked")
}}
"#
        );
        success(&source, b"array copies checked\n");
    }
}

#[test]
fn generic_struct_map_tuple_nominal_payloads_preserve_deep_copies() {
    for initial in ["7", "@as(Count, seed + 6)"] {
        let source = format!(
            r#"
type Count = int
struct Table<type K, type V> {{ [K]V entries }}
struct Box<type T> {{ T value }}
fn table<type K, type V>(K key, V value) Table<K, V> {{
    return Table<K, V>{{.entries = [key: value]}}
}}
fn wrap<type T>(T value) Box<T> {{ return Box<T>{{.value = value}} }}
test "generic map tuple copies" {{
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    Count value = {initial}
    mut (Count[2], Count[], Count[]?) payload = ([value, 8], [value, 9], [value, 10])
    mut Box<Table<str, (Count[2], Count[], Count[]?)>> boxed =
        wrap<Table<str, (Count[2], Count[], Count[]?)>>(table<str, (Count[2], Count[], Count[]?)>("x", payload))
    Box<Table<str, (Count[2], Count[], Count[]?)>> snapshot = boxed
    payload[0][index] = 11
    payload[1][index] = 12
    payload[2] = none
    boxed.value.entries["x"][0][index] = 13
    boxed.value.entries["x"][1][index] = 14
    boxed.value.entries["x"][2] = none
    boxed.value.entries["new"] = payload
    assert boxed.value.entries.len == 2 and snapshot.value.entries.len == 1
    (Count[2], Count[], Count[]?) original = ([value, 8], [value, 9], [value, 10])
    assert snapshot.value.entries["x"] == original
    assert boxed.value.entries["new"] == payload
    assert @as(int, boxed.value.entries["x"][0][0]) == 13
    assert @as(int, boxed.value.entries["x"][1][0]) == 14
    Count[]? missing = none
    assert boxed.value.entries["x"][2] == missing
    Count[2] fixed, (Count[], Count[]?) tail = snapshot.value.entries["x"]
    assert fixed == original[0] and tail[0] == original[1]
    mut Count[] extracted = tail[1] else {{ throw "missing optional payload" }}
    extracted[index] = 15
    Count[] changed_payload = [15, 10]
    Count[] fallback = [99]
    assert extracted == changed_payload
    Count[] original_optional = [value, 10]
    assert (snapshot.value.entries["x"][2] else fallback) == original_optional
    @println("map tuple copies checked")
}}
"#
        );
        success(&source, b"map tuple copies checked\n");
    }
}

#[test]
fn generic_enum_optional_nominal_struct_payloads_preserve_variants_and_copies() {
    for (initial, select_value, select_other) in [
        ("7", "false", "true"),
        ("@as(Count, seed + 6)", "seed != 1", "seed == 1"),
    ] {
        let source = format!(
            r#"
type Count = int
struct Box<type T> {{ T value }}
enum Choice<type T, type U> {{ Value(T) Other(U) Empty }}
fn wrap<type T>(T value) Box<T> {{ return Box<T>{{.value = value}} }}
fn choose<type T, type U>(bool other, T value, U alternate) Choice<T, U> {{
    if other {{ true -> {{ return Choice.Other(alternate) }} false -> {{ return Choice.Value(value) }} }}
}}
fn forward<type T, type U>(Choice<T, U> value) Choice<T, U> {{ return value }}
fn extract<type T, type U>(Choice<T, U> choice, T fallback) T {{
    if choice {{
        Choice.Value(value) -> {{ return value }}
        Choice.Other(other) -> {{ return fallback }}
        Choice.Empty -> {{ return fallback }}
    }}
}}
test "generic optional enum copies" {{
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    Count value = {initial}
    mut Box<Count[2][]> source = wrap<Count[2][]>([[value, 8]])
    Box<Count[2][]>? present = source
    Box<Count[2][]>? absent = none
    mut Choice<Box<Count[2][]>?, (Count, Count[])> selected =
        choose<Box<Count[2][]>?, (Count, Count[])>({select_value}, present, (value, [value, 9]))
    Choice<Box<Count[2][]>?, (Count, Count[])> snapshot = forward<Box<Count[2][]>?, (Count, Count[])>(selected)
    source.value[index][index] = 11
    selected = Choice.Empty
    Choice<Box<Count[2][]>?, (Count, Count[])> empty = Choice.Empty
    assert selected == empty
    Box<Count[2][]>? optional = extract<Box<Count[2][]>?, (Count, Count[])>(snapshot, none)
    mut Box<Count[2][]> extracted = optional else {{ throw "missing present enum payload" }}
    Count[2][] original_payload = [[value, 8]]
    assert extracted.value == original_payload
    extracted.value[index][index] = 12
    Box<Count[2][]> again = extract<Box<Count[2][]>?, (Count, Count[])>(snapshot, none) else {{ throw "snapshot changed" }}
    Count[2][] changed_payload = [[12, 8]]
    assert again.value == original_payload and extracted.value == changed_payload
    Choice<Box<Count[2][]>?, (Count, Count[])> empty_payload =
        choose<Box<Count[2][]>?, (Count, Count[])>({select_value}, absent, (value, [value]))
    assert extract<Box<Count[2][]>?, (Count, Count[])>(empty_payload, present) == absent
    Choice<Box<Count[2][]>?, (Count, Count[])> alternate =
        choose<Box<Count[2][]>?, (Count, Count[])>({select_other}, absent, (value, [value, 9]))
    if alternate {{
        Choice.Value(payload) -> {{ assert false }}
        Choice.Other(payload) -> {{
            mut (Count, Count[]) local = payload
            local[1][index] = 13
            Count[] changed = [13, 9]
            Count[] original = [value, 9]
            assert payload[1] == original and local[1] == changed
            assert @as(int, payload[0]) == 7
        }}
        Choice.Empty -> {{ assert false }}
    }}
    assert extract<Box<Count[2][]>?, (Count, Count[])>(selected, present) == present
    assert extract<Box<Count[2][]>?, (Count, Count[])>(alternate, absent) == absent
    @println("enum optional copies checked")
}}
"#
        );
        success(&source, b"enum optional copies checked\n");
    }
}

fn success(source: &str, expected: &[u8]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(&input, source).unwrap();
    run_both(&input, expected);
}

fn run_both(input: &Path, expected: &[u8]) {
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "release={release}");
        assert_eq!(output.stderr, b"", "release={release}");
    }
}
