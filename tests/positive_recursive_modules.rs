use std::{fs, path::Path, process::Command};

const NODE_MODULE: &str = r#"
import { "boxes" as boxes "payloads" as payloads }
pub struct Node<type T> { payloads.Data<T> data boxes.Box<Node<T>> children }
pub fn leaf<type T>(T value) Node<T> {
    return Node<T>{.data = payloads.Data<T>{.value = value}, .children = boxes.Box<Node<T>>{.entries = []}}
}
pub fn branch<type T>(T value, Node<T> child) Node<T> {
    return Node<T>{.data = payloads.Data<T>{.value = value}, .children = boxes.Box<Node<T>>{.entries = ["leaf": child]}}
}
pub fn count<type T>(Node<T> node) uint {
    mut uint total = 1
    for key in node.children.entries { total = total + count<T>(node.children.entries[key]) }
    return total
}
pub fn rebuild<type T>(Node<T> node, T value) Node<T> {
    mut Node<T> copy = node
    copy.data.value = value
    for key in copy.children.entries {
        copy.children.entries[key] = rebuild<T>(copy.children.entries[key], value)
    }
    return copy
}
"#;

const CHECKS: &str = r#"
import { "nodes" as nodes }
test "recursive generic module chain" {
    int seed = @as(int, @args().len)
    str key = if seed == 1 { true -> { "leaf" } _ -> { "unused" } }
    mut int[] payload = [INITIAL, 8]
    nodes.Node<int[]> leaf = nodes.leaf<int[]>(payload)
    nodes.Node<int[]> middle = nodes.branch<int[]>([9], leaf)
    mut nodes.Node<int[]> root = nodes.branch<int[]>([10], middle)
    nodes.Node<int[]> snapshot = root
    assert nodes.count<int[]>(root) == 3 and nodes.count<int[]>(leaf) == 1
    payload[0] = 99
    assert leaf.data.value == [7, 8]
    root.children.entries[key].children.entries[key].data.value[0] = 12
    assert snapshot.children.entries[key].children.entries[key].data.value == [7, 8]
    assert middle.children.entries[key].data.value == [7, 8]
    assert leaf.data.value == [7, 8] and root != snapshot
    mut nodes.Node<int[]> extracted = snapshot.children.entries[key]
    extracted.children.entries[key].data.value = [13]
    extracted.children.entries["extra"] = leaf
    assert nodes.count<int[]>(extracted) == 3
    assert nodes.count<int[]>(snapshot) == 3 and nodes.count<int[]>(middle) == 2
    assert snapshot.children.entries[key] == middle
    root.children.entries["extra"] = extracted
    assert nodes.count<int[]>(root) == 6 and nodes.count<int[]>(snapshot) == 3

    mut int[] replacement = [20]
    mut nodes.Node<int[]> rebuilt = nodes.rebuild<int[]>(snapshot, replacement)
    nodes.Node<int[]> expected_leaf = nodes.leaf<int[]>([20])
    nodes.Node<int[]> expected = nodes.branch<int[]>([20], nodes.branch<int[]>([20], expected_leaf))
    assert rebuilt == expected and rebuilt != snapshot
    assert nodes.count<int[]>(rebuilt) == 3
    replacement[0] = 21
    rebuilt.children.entries[key].children.entries[key].data.value[0] = 22
    assert expected.children.entries[key].children.entries[key].data.value == [20]
    assert rebuilt.data.value == [20] and rebuilt.children.entries[key].data.value == [20]
    assert snapshot.children.entries[key].children.entries[key].data.value == [7, 8]
    nodes.Node<int[]> empty_payload = nodes.leaf<int[]>([])
    assert empty_payload.data.value.len == 0 and empty_payload.children.entries.len == 0
    assert nodes.rebuild<int[]>(empty_payload, []) == empty_payload

    nodes.Node<str> text_leaf = nodes.leaf<str>("λ")
    nodes.Node<str> text = nodes.branch<str>("root", text_leaf)
    mut nodes.Node<str> text_copy = text
    text_copy.children.entries[key].data.value = "changed"
    assert text.children.entries[key].data.value == "λ" and text_leaf.data.value == "λ"
    assert text_copy != text and nodes.count<str>(text) == 2
    assert nodes.rebuild<str>(text, "") == nodes.branch<str>("", nodes.leaf<str>(""))
    @println("module chain copies checked")
}
"#;

#[test]
fn recursive_generics_across_acyclic_modules_preserve_rebuilds_and_copies() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("payloads.nc"),
        "pub struct Data<type T> { T value }",
    )
    .unwrap();
    fs::write(
        directory.path().join("boxes.nc"),
        "pub struct Box<type T> { [str]T entries }",
    )
    .unwrap();
    fs::write(directory.path().join("nodes.nc"), NODE_MODULE).unwrap();
    let input = directory.path().join("main.nc");
    for initial in ["7", "seed + 6"] {
        fs::write(&input, CHECKS.replace("INITIAL", initial)).unwrap();
        run_both(&input);
    }
}

fn run_both(input: &Path) {
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
        assert_eq!(output.stdout, b"module chain copies checked\n");
    }
}
