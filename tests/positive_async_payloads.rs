use std::{fs, process::Command};

const DECLARATIONS: &str = r"
type Count = int
type Numbers = int[]
type Lookup = [str]int
struct Box<type T> { T value }
enum Choice<type T> { Value(T) Empty }
fn forward<type T>(T value) T { return value }
";

type PayloadCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

#[test]
fn locked_async_payload_snapshots_preserve_nominal_composite_and_optional_copies() {
    for (kind, input, expected, changed, mutate) in payload_cases() {
        let mutate_original = mutate.replace("result", "original");
        let source = format!(
            r#"{DECLARATIONS}
test "locked {kind} snapshots" {{
    int seed = @as(int, @args().len)
    mut {kind} original = {input}
    {kind} expected = {expected}
    {kind} changed = {changed}
    mutex {kind} payload = original
    {mutate_original}
    original = changed
    fn read() {kind} {{
        lock payload {{
            {kind} copy = payload
            return forward<{kind}>(copy)
        }}
    }}
    fut {kind} first = async read()
    fut {kind} second = async read()
    mut {kind} result = await first
    {kind} other = await second
    {kind} snapshot = result
    assert result == expected and other == expected
    lock payload {{
        assert payload == expected
        payload = changed
    }}
    {mutate}
    result = changed
    assert result == changed
    assert snapshot == expected and other == expected
    assert original == changed
    lock payload {{ assert payload == changed }}
    @println("snapshots checked")
}}
"#
        );
        constant_and_runtime(&source, "snapshots checked\n");
    }
}

fn payload_cases() -> [PayloadCase; 13] {
    [
        (
            "Count",
            "@as(Count, seed)",
            "1",
            "9",
            "assert @as(int, result) == 1",
        ),
        (
            "Numbers",
            "@as(Numbers, [seed, 2])",
            "[1, 2]",
            "[9]",
            "mut int[] copy = @as(int[], result); copy[0] = 99; assert copy == [99, 2] and result == expected",
        ),
        (
            "int[2]",
            "[seed, 2]",
            "[1, 2]",
            "[9, 8]",
            "result[0] = 99; assert result == [99, 2]",
        ),
        (
            "int[][]",
            "[[seed, 2], [3]]",
            "[[1, 2], [3]]",
            "[[9]]",
            "result[0][0] = 99; result[1] = result[1] <> [4]; assert result == [[99, 2], [3, 4]]",
        ),
        (
            "Lookup",
            "@as(Lookup, [\"key\": seed])",
            "[\"key\": 1]",
            "[\"other\": 9]",
            "mut [str]int copy = @as([str]int, result); copy[\"key\"] = 99; copy[\"new\"] = 2; assert copy.len == 2 and copy[\"key\"] == 99 and result == expected",
        ),
        (
            "(str, int[])",
            "(\"item-{seed}\", [seed, 2])",
            "(\"item-1\", [1, 2])",
            "(\"changed\", [9])",
            "result[1][0] = 99; assert result[1] == [99, 2]",
        ),
        (
            "Box<Numbers>",
            "Box<Numbers>{.value = @as(Numbers, [seed, 2])}",
            "Box<Numbers>{.value = [1, 2]}",
            "Box<Numbers>{.value = [9]}",
            "mut int[] copy = @as(int[], result.value); copy[0] = 99; assert copy == [99, 2] and result == expected",
        ),
        (
            "Box<Box<int[][]>>",
            "Box<Box<int[][]>>{.value = Box<int[][]>{.value = [[seed, 2]]}}",
            "Box<Box<int[][]>>{.value = Box<int[][]>{.value = [[1, 2]]}}",
            "Box<Box<int[][]>>{.value = Box<int[][]>{.value = [[9]]}}",
            "result.value.value[0][0] = 99; assert result.value.value == [[99, 2]]",
        ),
        (
            "Choice<Box<int[]>>",
            "Choice.Value(Box<int[]>{.value = [seed, 2]})",
            "Choice.Value(Box<int[]>{.value = [1, 2]})",
            "Choice.Empty",
            "if result { Choice.Value(value) -> { mut Box<int[]> copy = value; copy.value[0] = 99; assert copy.value == [99, 2] } Choice.Empty -> { assert false } }; assert result == expected",
        ),
        (
            "Choice<Box<int[]>>",
            "Choice.Empty",
            "Choice.Empty",
            "Choice.Value(Box<int[]>{.value = [9]})",
            "if result { Choice.Value(value) -> { assert false } Choice.Empty -> {} }",
        ),
        (
            "int[]?",
            "[seed, 2]",
            "[1, 2]",
            "none",
            "mut int[] copy = result else { throw \"missing array\" }; copy[0] = 99; assert copy == [99, 2] and result == expected",
        ),
        (
            "int[]?",
            "none",
            "none",
            "[9]",
            "int[] copy = result else [seed, 2]; assert copy == [1, 2] and result == expected",
        ),
        (
            "Box<(Numbers, int[]?)>",
            "Box<(Numbers, int[]?)>{.value = (@as(Numbers, [seed]), [seed, 2])}",
            "Box<(Numbers, int[]?)>{.value = ([1], [1, 2])}",
            "Box<(Numbers, int[]?)>{.value = ([9], none)}",
            "mut int[] copy = result.value[1] else { throw \"missing nested array\" }; copy[0] = 99; assert copy == [99, 2] and result == expected",
        ),
    ]
}

#[test]
fn concurrent_locked_composite_updates_have_schedule_independent_results() {
    for (kind, input, expected, update, mutate) in update_cases() {
        let source = format!(
            r#"{DECLARATIONS}
test "commutative {kind} updates" {{
    int seed = @as(int, @args().len)
    {kind} initial = {input}
    {kind} initial_expected = {input}
    {kind} expected = {expected}
    mutex {kind} payload = initial
    fn add(int delta) int {{
        lock payload {{ {update} }}
        return delta
    }}
    (fn(int) int) worker = fn(int delta) int {{
        lock payload {{ {update} }}
        return delta
    }}
    fut int first = async add(seed)
    fut int second = async worker(seed + 1)
    int a = await first
    int b = await second
    assert a == 1 and b == 2
    assert initial == initial_expected
    lock payload {{
        assert payload == expected
        mut {kind} result = payload
        {mutate}
        assert result != expected
        assert payload == expected
    }}
    lock payload {{ assert payload == expected }}
    @println("updates checked")
}}
"#
        );
        constant_and_runtime(&source, "updates checked\n");
    }
}

fn update_cases() -> [PayloadCase; 5] {
    [
        (
            "int[2]",
            "[seed, 2]",
            "[4, 5]",
            "for i in payload { payload[i] = payload[i] + delta }",
            "result[0] = 99",
        ),
        (
            "(int, int[])",
            "(seed, [seed, 2])",
            "(4, [4, 5])",
            "payload[0] = payload[0] + delta; for i in payload[1] { payload[1][i] = payload[1][i] + delta }",
            "result[1][0] = 99",
        ),
        (
            "Box<int[][]>",
            "Box<int[][]>{.value = [[seed, 2], [3]]}",
            "Box<int[][]>{.value = [[4, 5], [6]]}",
            "for i in payload.value { for j in payload.value[i] { payload.value[i][j] = payload.value[i][j] + delta } }",
            "result.value[0][0] = 99",
        ),
        (
            "Box<[str]int[]>",
            "Box<[str]int[]>{.value = [\"a\": [seed, 2], \"b\": [3]]}",
            "Box<[str]int[]>{.value = [\"a\": [4, 5], \"b\": [6]]}",
            "for key in payload.value { for i in payload.value[key] { payload.value[key][i] = payload.value[key][i] + delta } }",
            "result.value[\"a\"][0] = 99; result.value[\"new\"] = [7]",
        ),
        (
            "Box<int[]?>",
            "Box<int[]?>{.value = [seed, 2]}",
            "Box<int[]?>{.value = [4, 5]}",
            "mut int[] values = payload.value else { return -1 }; for i in values { values[i] = values[i] + delta }; payload.value = values",
            "mut int[] values = result.value else { throw \"missing result\" }; values[0] = 99; result.value = values",
        ),
    ]
}

fn constant_and_runtime(source: &str, stdout: &str) {
    for (phase, source) in [
        (
            "constant",
            source.replace("int seed = @as(int, @args().len)", "int seed = 1"),
        ),
        ("runtime", source.to_owned()),
    ] {
        for release in [false, true] {
            let dir = ncc::temp::Directory::new().unwrap();
            let file = dir.path().join("test.nc");
            fs::write(&file, &source).unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
            command.arg("test");
            if release {
                command.arg("-r");
            }
            let output = command.arg(&file).output().unwrap();
            assert!(
                output.status.success(),
                "phase={phase}, release={release}: {}\n{source}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                stdout,
                "phase={phase}, release={release}"
            );
        }
    }
}
