use std::{fs, process::Command};

const PAYLOAD_MODULE: &str = r#"
pub struct Box<type T> { T value }
pub fn forward<type T>(T value) T { return value }
pub fn wrapped(bool fail, [str]int[] values) Box<Box<[str]int[]>>! {
    if { fail -> { throw "map failure" } _ -> {} }
    return Box<Box<[str]int[]>>{.value = Box<[str]int[]>{.value = values}}
}
"#;

const NODE_MODULE: &str = r#"
import { "payloads" as payloads }
pub struct Node<type T> { T value [str]Node<T> children }
pub fn leaf<type T>(T value) Node<T> {
    return Node<T>{.value = value, .children = []}
}
pub fn count<type T>(Node<T> node) uint {
    mut uint total = 1
    for key in node.children { total = total + count<T>(node.children[key]) }
    return total
}
pub fn rebuild<type T>(Node<T> node) Node<T> {
    mut [str]Node<T> children = []
    for key in node.children { children[key] = rebuild<T>(node.children[key]) }
    return Node<T>{.value = payloads.forward<T>(node.value), .children = children}
}
pub fn shifted<type T>(Node<T> node, int delta) Node<T> {
    mut Node<T> result = node
    for key in result.value.value.value {
        for i in result.value.value.value[key] {
            result.value.value.value[key][i] = result.value.value.value[key][i] + delta
        }
    }
    for key in result.children { result.children[key] = shifted<T>(result.children[key], delta) }
    return result
}
pub fn checked<type T>(Node<T>? node, bool fail) Node<T>?! {
    if { fail -> { throw "tree failure" } _ -> {} }
    return node
}
pub fn relay<type T>(Node<T>? node, bool fail) Node<T>?! {
    fut Node<T>?! pending = async checked<T>(node, fail)
    return try await pending
}
"#;

const SNAPSHOT_CHECKS: &str = r#"
import { "nodes" as nodes "payloads" as payloads }
fn unpack(KIND value) payloads.Box<[str]int[]>! { return payloads.Box<[str]int[]>{.value = UNPACK} }
fn fallback(KIND value) payloads.Box<[str]int[]>! { return payloads.Box<[str]int[]>{.value = FALLBACK} }
test "imported recursive map snapshots" {
    int seed = @as(int, @args().len)
    [str]int[] expected_values = ["items": [1, 2], "empty": []]
    [str]int[] fallback_values = ["fallback": [7]]
    KIND present = PRESENT
    KIND absent = ABSENT
    nodes.Node<KIND> leaf = nodes.leaf<KIND>(present)
    nodes.Node<KIND> empty = nodes.leaf<KIND>(absent)
    mut nodes.Node<KIND> original = nodes.Node<KIND>{.value = present, .children = [
        "branch": nodes.Node<KIND>{.value = absent, .children = ["leaf": leaf]},
        "empty": empty,
    ]}
    nodes.Node<KIND> expected = nodes.rebuild<KIND>(original)
    mutex nodes.Node<KIND> payload = original
    original.children["branch"].children["leaf"].value = absent
    original.children["new"] = leaf
    fn read() nodes.Node<KIND> {
        lock payload { return nodes.rebuild<KIND>(payload) }
    }
    fut nodes.Node<KIND> first = async read()
    fut nodes.Node<KIND> second = async read()
    mut nodes.Node<KIND> result = await first
    nodes.Node<KIND> other = await second
    nodes.Node<KIND> snapshot = result
    assert result == expected and other == expected
    assert nodes.count<KIND>(result) == 4
    assert (try unpack(result.children["branch"].children["leaf"].value)).value == expected_values
    assert (try fallback(result.children["empty"].value)).value == fallback_values
    mut [str]int[] extracted = (try unpack(result.children["branch"].children["leaf"].value)).value
    extracted["items"][0] = 99
    extracted["new"] = [8]
    assert (try unpack(result.children["branch"].children["leaf"].value)).value == expected_values
    result.children["branch"].children["leaf"].value = absent
    result.children["branch"].children["added"] = leaf
    result.children["empty"].value = present
    assert nodes.count<KIND>(result) == 5
    assert (try fallback(result.children["branch"].children["leaf"].value)).value == fallback_values
    assert (try unpack(result.children["empty"].value)).value == expected_values
    assert snapshot == expected and other == expected
    assert nodes.count<KIND>(original) == 5
    assert (try fallback(original.children["branch"].children["leaf"].value)).value == fallback_values
    lock payload { assert payload == expected; payload = result }
    result.children["branch"].children["added"].value = absent
    result.children["branch"].children["later"] = empty
    lock payload {
        assert nodes.count<KIND>(payload) == 5
        assert (try unpack(payload.children["branch"].children["added"].value)).value == expected_values
        assert payload.children["branch"].children.len == 2
    }
    assert snapshot == expected and other == expected
    @println("imported map snapshots checked")
}
"#;

const UPDATE_CHECKS: &str = r#"
import { "nodes" as nodes "payloads" as payloads }
test "imported recursive map commutative updates" {
    int seed = @as(int, @args().len)
    payloads.Box<payloads.Box<[str]int[]>> data = try payloads.wrapped(false, ["items": [seed, 2], "empty": []])
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> leaf = nodes.leaf<payloads.Box<payloads.Box<[str]int[]>>>(data)
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> original = nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>{.value = data, .children = [
        "branch": nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>{.value = data, .children = ["leaf": leaf]},
        "sibling": leaf,
    ]}
    mut int catches = 0
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>? present = nodes.relay<payloads.Box<payloads.Box<[str]int[]>>>(original, false) catch err { catches = catches + 1; throw err }
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>? absent = nodes.relay<payloads.Box<payloads.Box<[str]int[]>>>(none, false) catch err { catches = catches + 1; throw err }
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>? empty = none
    assert absent == empty and catches == 0
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>? recovered = nodes.relay<payloads.Box<payloads.Box<[str]int[]>>>(original, seed == 1) catch err {
        assert @as(str, err) == "tree failure"
        catches = catches + 1
        break none
    }
    assert recovered == empty and catches == 1
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> snapshot = present else { throw "missing tree" }
    mutex nodes.Node<payloads.Box<payloads.Box<[str]int[]>>>? payload = present
    fn add(int delta) uint {
        lock payload {
            nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> local = payload else { return 0 }
            payload = nodes.shifted<payloads.Box<payloads.Box<[str]int[]>>>(local, delta)
            return nodes.count<payloads.Box<payloads.Box<[str]int[]>>>(local)
        }
    }
    (fn(int) uint) worker = fn(int delta) uint { return add(delta) }
    fut uint first = async add(seed)
    fut uint second = async worker(seed + 1)
    assert await first == 4
    assert await second == 4
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> expected = nodes.shifted<payloads.Box<payloads.Box<[str]int[]>>>(original, 3)
    lock payload {
        mut nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> local = payload else { throw "lost tree" }
        assert local == expected
        assert local.value.value.value["items"] == [4, 5]
        assert local.children["branch"].value.value.value["items"] == [4, 5]
        assert local.children["branch"].children["leaf"].value.value.value["items"] == [4, 5]
        assert local.children["sibling"].value.value.value["items"] == [4, 5]
        assert local.children["sibling"].value.value.value["empty"].len == 0
        local.children["branch"].children["leaf"].value.value.value["items"][0] = 99
        local.children["branch"].children["leaf"].value.value.value["new"] = [8]
        local.children["branch"].children["new"] = leaf
        nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> unchanged = payload else { throw "lost locked snapshot" }
        assert unchanged == expected and local != expected
        payload = local
        local.children["branch"].children["leaf"].value.value.value["new"][0] = 100
        nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> copied = payload else { throw "lost copied tree" }
        assert copied.children["branch"].children["leaf"].value.value.value["new"] == [8]
        assert nodes.count<payloads.Box<payloads.Box<[str]int[]>>>(copied) == 5
        payload = none
    }
    lock payload { assert payload == empty }
    fut uint missing = async add(seed)
    assert await missing == 0
    lock payload { assert payload == empty }
    nodes.Node<payloads.Box<payloads.Box<[str]int[]>>> retained = present else { throw "lost original snapshot" }
    assert retained == original and snapshot == original
    assert original.children["branch"].children["leaf"].value.value.value["items"] == [1, 2]
    [str]int[] expected_values = ["items": [1, 2], "empty": []]
    assert data.value.value == expected_values
    @println("imported map updates checked")
}
"#;

#[test]
fn imported_recursive_optional_map_snapshots_are_independent() {
    let source = SNAPSHOT_CHECKS
        .replace("KIND", "payloads.Box<payloads.Box<[str]int[]>>?")
        .replace(
            "PRESENT",
            "try payloads.wrapped(false, [\"items\": [seed, 2], \"empty\": []])",
        )
        .replace("ABSENT", "none")
        .replace(
            "UNPACK",
            "(value else { throw \"missing map\" }).value.value",
        )
        .replace(
            "FALLBACK",
            "(value else (try payloads.wrapped(false, [\"fallback\": [7]]))).value.value",
        );
    constant_and_runtime(&source, "imported map snapshots checked\n");
}

#[test]
fn imported_recursive_error_map_snapshots_are_independent() {
    let source = SNAPSHOT_CHECKS
        .replace("KIND", "payloads.Box<payloads.Box<[str]int[]>>!")
        .replace(
            "PRESENT",
            "payloads.wrapped(false, [\"items\": [seed, 2], \"empty\": []])",
        )
        .replace("ABSENT", "payloads.wrapped(true, [])")
        .replace("UNPACK", "(value catch err { throw err }).value.value")
        .replace(
            "FALLBACK",
            r#"(value catch err {
                if { @as(str, err) != "map failure" -> { throw "wrong map error" } _ -> {} }
                break try payloads.wrapped(false, ["fallback": [7]])
            }).value.value"#,
        );
    constant_and_runtime(&source, "imported map snapshots checked\n");
}

#[test]
fn imported_recursive_map_locked_updates_and_async_errors_preserve_copies() {
    constant_and_runtime(UPDATE_CHECKS, "imported map updates checked\n");
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
            fs::write(dir.path().join("payloads.nc"), PAYLOAD_MODULE).unwrap();
            fs::write(dir.path().join("nodes.nc"), NODE_MODULE).unwrap();
            let file = dir.path().join("main.nc");
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
            assert!(
                output.stderr.is_empty(),
                "phase={phase}, release={release}: {}",
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
