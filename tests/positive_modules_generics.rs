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
