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

const RECURSIVE_DECLARATIONS: &str = r#"
struct Node<type T> { T value Node<T>[] children }
fn count<type T>(Node<T> node) uint {
    mut uint total = 1
    for i in node.children { total = total + count<T>(node.children[i]) }
    return total
}
fn numbers(bool fail, int seed) int[]! {
    if { fail -> { throw "nested failure" } _ -> {} }
    return [seed, 2]
}
"#;

#[test]
fn recursive_optional_and_error_elements_cross_locked_future_copy_boundaries() {
    for (kind, present, absent, unwrap_present, unwrap_absent) in [
        (
            "int[]?",
            "[seed, 2]",
            "none",
            "value else { throw \"missing value\" }",
            "value else [7]",
        ),
        (
            "int[]!",
            "numbers(false, seed)",
            "numbers(true, seed)",
            "value catch err { throw err }",
            "value catch err { if { @as(str, err) != \"nested failure\" -> { throw \"wrong error\" } _ -> {} }; break [7] }",
        ),
    ] {
        let source = format!(
            r#"{RECURSIVE_DECLARATIONS}
test "recursive wrapped elements" {{
    fn unwrap({kind} value) int[]! {{ return {unwrap_present} }}
    fn fallback({kind} value) int[]! {{ return {unwrap_absent} }}
    int seed = @as(int, @args().len)
    {kind} present = {present}
    {kind} absent = {absent}
    Node<{kind}> leaf = Node<{kind}>{{.value = absent, .children = []}}
    Node<{kind}> original = Node<{kind}>{{.value = present, .children = [leaf]}}
    mutex Node<{kind}> payload = original
    fn read() Node<{kind}> {{ lock payload {{ return payload }} }}
    fut Node<{kind}> first = async read()
    fut Node<{kind}> second = async read()
    mut Node<{kind}> result = await first
    Node<{kind}> other = await second
    assert count<{kind}>(result) == 2
    assert (try unwrap(result.value)) == [1, 2]
    assert (try fallback(result.children[0].value)) == [7]
    mut int[] extracted = try unwrap(result.value)
    extracted[0] = 99
    assert (try unwrap(result.value)) == [1, 2]
    result.children[0].value = present
    result.children = result.children <> [leaf]
    assert count<{kind}>(result) == 3
    assert (try unwrap(result.children[0].value)) == [1, 2]
    assert (try fallback(result.children[1].value)) == [7]
    assert (try fallback(other.children[0].value)) == [7]
    assert (try fallback(original.children[0].value)) == [7]
    lock payload {{
        assert count<{kind}>(payload) == 2
        assert (try fallback(payload.children[0].value)) == [7]
        payload = result
    }}
    result.value = absent
    lock payload {{ assert (try unwrap(payload.value)) == [1, 2] }}
    assert (try unwrap(other.value)) == [1, 2]
    @println("recursive elements checked")
}}
"#
        );
        constant_and_runtime(&source, "recursive elements checked\n");
    }
}

#[test]
fn recursive_optional_async_errors_propagate_and_locked_updates_preserve_snapshots() {
    let source = format!(
        r#"{RECURSIVE_DECLARATIONS}
fn checked(Node<int[]>? node, bool fail) Node<int[]>?! {{
    if {{ fail -> {{ throw "recursive failure" }} _ -> {{}} }}
    return node
}}
fn relay(Node<int[]>? node, bool fail) Node<int[]>?! {{
    fut Node<int[]>?! pending = async checked(node, fail)
    return try await pending
}}
test "recursive optional async error payload" {{
    int seed = @as(int, @args().len)
    Node<int[]> leaf = Node<int[]>{{.value = [seed, 2], .children = []}}
    Node<int[]> original = Node<int[]>{{.value = [3], .children = [leaf]}}
    mut int catches = 0
    Node<int[]>? present = relay(original, false) catch err {{ catches = catches + 1; throw err }}
    Node<int[]>? absent = relay(none, false) catch err {{ catches = catches + 1; throw err }}
    Node<int[]>? empty = none
    assert absent == empty and catches == 0
    Node<int[]>? recovered = relay(original, seed == 1) catch err {{
        assert @as(str, err) == "recursive failure"
        catches = catches + 1
        break none
    }}
    assert recovered == empty and catches == 1
    mut Node<int[]> snapshot = present else {{ throw "missing tree" }}
    mutex Node<int[]>? payload = present
    fn add(int delta) uint {{
        lock payload {{
            mut Node<int[]> local = payload else {{ return 0 }}
            local.children[0].value[0] = local.children[0].value[0] + delta
            payload = local
            return count<int[]>(local)
        }}
    }}
    fut uint first = async add(seed)
    fut uint second = async add(seed + 1)
    assert await first == 2
    assert await second == 2
    lock payload {{
        mut Node<int[]> local = payload else {{ throw "missing updated tree" }}
        assert local.children[0].value == [4, 2]
        local.children[0].value[0] = 99
        Node<int[]> unchanged = payload else {{ throw "lost tree" }}
        assert unchanged.children[0].value == [4, 2]
        payload = none
    }}
    lock payload {{ assert payload == empty }}
    snapshot.children[0].value[0] = 9
    assert original.children[0].value == [1, 2]
    Node<int[]> retained = present else {{ throw "lost snapshot" }}
    assert retained.children[0].value == [1, 2]
    @println("recursive async updates checked")
}}
"#
    );
    constant_and_runtime(&source, "recursive async updates checked\n");
}

const RECURSIVE_MAP_DECLARATIONS: &str = r"
struct MapNode<type T> { T value [str]MapNode<T> children }
fn map_count<type T>(MapNode<T> node) uint {
    mut uint total = 1
    for key in node.children { total = total + map_count<T>(node.children[key]) }
    return total
}
fn rebuild_map<type T>(MapNode<T> node) MapNode<T> {
    mut [str]MapNode<T> children = []
    for key in node.children { children[key] = rebuild_map<T>(node.children[key]) }
    return MapNode<T>{.value = forward<T>(node.value), .children = children}
}
";

#[test]
fn recursive_generic_maps_cross_locked_future_boundaries_with_independent_copies() {
    for (kind, present, absent, unwrap_present, unwrap_absent) in recursive_map_cases() {
        let source = format!(
            r#"{DECLARATIONS}
{RECURSIVE_MAP_DECLARATIONS}
fn wrapped(bool fail, [str]int[] values) Box<Box<[str]int[]>>! {{
    if {{ fail -> {{ throw "map failure" }} _ -> {{}} }}
    return Box<Box<[str]int[]>>{{.value = Box<[str]int[]>{{.value = values}}}}
}}
test "recursive map snapshots" {{
    fn unpack({kind} value) Box<[str]int[]>! {{ return Box<[str]int[]>{{.value = {unwrap_present}}} }}
    fn fallback({kind} value) Box<[str]int[]>! {{ return Box<[str]int[]>{{.value = {unwrap_absent}}} }}
    int seed = @as(int, @args().len)
    [str]int[] expected_values = ["items": [1, 2], "empty": []]
    [str]int[] fallback_values = ["fallback": [7]]
    {kind} present = {present}
    {kind} absent = {absent}
    MapNode<{kind}> empty = MapNode<{kind}>{{.value = absent, .children = []}}
    MapNode<{kind}> leaf = MapNode<{kind}>{{.value = present, .children = []}}
    mut MapNode<{kind}> original = MapNode<{kind}>{{.value = present, .children = [
        "branch": MapNode<{kind}>{{.value = absent, .children = ["leaf": leaf]}},
        "empty": empty,
    ]}}
    MapNode<{kind}> expected = rebuild_map<{kind}>(original)
    mutex MapNode<{kind}> payload = original
    original.children["branch"].children["leaf"].value = absent
    original.children["new"] = leaf
    fn read() MapNode<{kind}> {{
        lock payload {{ return rebuild_map<{kind}>(payload) }}
    }}
    fut MapNode<{kind}> first = async read()
    fut MapNode<{kind}> second = async read()
    mut MapNode<{kind}> result = await first
    MapNode<{kind}> other = await second
    MapNode<{kind}> snapshot = result
    assert result == expected and other == expected
    assert map_count<{kind}>(result) == 4
    assert (try unpack(result.children["branch"].children["leaf"].value)).value == expected_values
    assert (try fallback(result.children["empty"].value)).value == fallback_values
    mut [str]int[] extracted = (try unpack(result.children["branch"].children["leaf"].value)).value
    extracted["items"][0] = 99
    extracted["new"] = [8]
    assert (try unpack(result.children["branch"].children["leaf"].value)).value == expected_values
    result.children["branch"].children["leaf"].value = absent
    result.children["branch"].children["added"] = leaf
    result.children["empty"].value = present
    assert map_count<{kind}>(result) == 5
    assert result.children["branch"].children.len == 2
    assert (try fallback(result.children["branch"].children["leaf"].value)).value == fallback_values
    assert (try unpack(result.children["empty"].value)).value == expected_values
    assert snapshot == expected and other == expected
    assert map_count<{kind}>(original) == 5
    assert (try fallback(original.children["branch"].children["leaf"].value)).value == fallback_values
    lock payload {{ assert payload == expected; payload = result }}
    result.children["branch"].children["added"].value = absent
    result.children["branch"].children["later"] = empty
    lock payload {{
        assert map_count<{kind}>(payload) == 5
        assert (try unpack(payload.children["branch"].children["added"].value)).value == expected_values
        assert payload.children["branch"].children.len == 2
    }}
    assert snapshot == expected and other == expected
    @println("recursive map snapshots checked")
}}
"#
        );
        constant_and_runtime(&source, "recursive map snapshots checked\n");
    }
}

fn recursive_map_cases() -> [PayloadCase; 3] {
    [
        (
            "[str]int[]",
            "[\"items\": [seed, 2], \"empty\": []]",
            "[]",
            "value",
            "if { value.len == 0 -> { break [\"fallback\": [7]] } _ -> { break value } }",
        ),
        (
            "Box<Box<[str]int[]>>?",
            "Box<Box<[str]int[]>>{.value = Box<[str]int[]>{.value = [\"items\": [seed, 2], \"empty\": []]}}",
            "none",
            "(value else { throw \"missing map\" }).value.value",
            "(value else Box<Box<[str]int[]>>{.value = Box<[str]int[]>{.value = [\"fallback\": [7]]}}).value.value",
        ),
        (
            "Box<Box<[str]int[]>>!",
            "wrapped(false, [\"items\": [seed, 2], \"empty\": []])",
            "wrapped(true, [])",
            "(value catch err { throw err }).value.value",
            "(value catch err { if { @as(str, err) != \"map failure\" -> { throw \"wrong map error\" } _ -> {} }; break Box<Box<[str]int[]>>{.value = Box<[str]int[]>{.value = [\"fallback\": [7]]}} }).value.value",
        ),
    ]
}

#[test]
fn recursive_generic_map_updates_and_optional_async_errors_preserve_snapshots() {
    let source = format!(
        r#"{DECLARATIONS}
{RECURSIVE_MAP_DECLARATIONS}
fn shifted<type T>(MapNode<T> node, int delta) MapNode<T> {{
    mut MapNode<T> result = node
    for key in result.value.value.value {{
        for i in result.value.value.value[key] {{
            result.value.value.value[key][i] = result.value.value.value[key][i] + delta
        }}
    }}
    for key in result.children {{ result.children[key] = shifted<T>(result.children[key], delta) }}
    return result
}}
fn checked_map<type T>(MapNode<T>? node, bool fail) MapNode<T>?! {{
    if {{ fail -> {{ throw "tree failure" }} _ -> {{}} }}
    return node
}}
fn relay_map<type T>(MapNode<T>? node, bool fail) MapNode<T>?! {{
    fut MapNode<T>?! pending = async checked_map<T>(node, fail)
    return try await pending
}}
test "recursive map commutative updates" {{
    int seed = @as(int, @args().len)
    Box<Box<[str]int[]>> data = Box<Box<[str]int[]>>{{.value = Box<[str]int[]>{{.value = ["items": [seed, 2], "empty": []]}}}}
    MapNode<Box<Box<[str]int[]>>> leaf = MapNode<Box<Box<[str]int[]>>>{{.value = data, .children = []}}
    MapNode<Box<Box<[str]int[]>>> original = MapNode<Box<Box<[str]int[]>>>{{.value = data, .children = [
        "branch": MapNode<Box<Box<[str]int[]>>>{{.value = data, .children = ["leaf": leaf]}},
        "sibling": leaf,
    ]}}
    mut int catches = 0
    MapNode<Box<Box<[str]int[]>>>? present = relay_map<Box<Box<[str]int[]>>>(original, false) catch err {{ catches = catches + 1; throw err }}
    MapNode<Box<Box<[str]int[]>>>? absent = relay_map<Box<Box<[str]int[]>>>(none, false) catch err {{ catches = catches + 1; throw err }}
    MapNode<Box<Box<[str]int[]>>>? empty = none
    assert absent == empty and catches == 0
    MapNode<Box<Box<[str]int[]>>>? recovered = relay_map<Box<Box<[str]int[]>>>(original, seed == 1) catch err {{
        assert @as(str, err) == "tree failure"
        catches = catches + 1
        break none
    }}
    assert recovered == empty and catches == 1
    MapNode<Box<Box<[str]int[]>>> snapshot = present else {{ throw "missing tree" }}
    mutex MapNode<Box<Box<[str]int[]>>>? payload = present
    fn add(int delta) uint {{
        lock payload {{
            MapNode<Box<Box<[str]int[]>>> local = payload else {{ return 0 }}
            payload = shifted<Box<Box<[str]int[]>>>(local, delta)
            return map_count<Box<Box<[str]int[]>>>(local)
        }}
    }}
    (fn(int) uint) worker = fn(int delta) uint {{ return add(delta) }}
    fut uint first = async add(seed)
    fut uint second = async worker(seed + 1)
    assert await first == 4
    assert await second == 4
    MapNode<Box<Box<[str]int[]>>> expected = shifted<Box<Box<[str]int[]>>>(original, 3)
    lock payload {{
        mut MapNode<Box<Box<[str]int[]>>> local = payload else {{ throw "lost tree" }}
        assert local == expected
        assert local.children["branch"].children["leaf"].value.value.value["items"] == [4, 5]
        assert local.children["sibling"].value.value.value["empty"].len == 0
        local.children["branch"].children["leaf"].value.value.value["items"][0] = 99
        local.children["branch"].children["leaf"].value.value.value["new"] = [8]
        local.children["branch"].children["new"] = leaf
        MapNode<Box<Box<[str]int[]>>> unchanged = payload else {{ throw "lost locked snapshot" }}
        assert unchanged == expected and local != expected
        payload = local
        local.children["branch"].children["leaf"].value.value.value["new"][0] = 100
        MapNode<Box<Box<[str]int[]>>> copied = payload else {{ throw "lost copied tree" }}
        assert copied.children["branch"].children["leaf"].value.value.value["new"] == [8]
        assert map_count<Box<Box<[str]int[]>>>(copied) == 5
        payload = none
    }}
    lock payload {{ assert payload == empty }}
    MapNode<Box<Box<[str]int[]>>> retained = present else {{ throw "lost original snapshot" }}
    assert retained == original and snapshot == original
    assert original.children["branch"].children["leaf"].value.value.value["items"] == [1, 2]
    [str]int[] expected_values = ["items": [1, 2], "empty": []]
    assert data.value.value == expected_values
    @println("recursive map updates checked")
}}
"#
    );
    constant_and_runtime(&source, "recursive map updates checked\n");
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
