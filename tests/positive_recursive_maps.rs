use std::{fs, path::Path, process::Command};

const ENUM_DEFINITIONS: &str = r"
pub enum Tree<type T> { Leaf(T) Branch([str]Link<T>) Empty }
pub enum Link<type T> { Children([str]Tree<T>) Values([str]T) Empty }
pub fn children_link<type T>([str]Tree<T> nodes) Link<T> { return Link.Children(nodes) }
pub fn values_link<type T>([str]T values) Link<T> { return Link.Values(values) }
pub fn empty_link<type T>() Link<T> { return Link.Empty }
pub fn sum_link(Link<int[]> link) int {
    if link {
        Link.Children(nodes) -> {
            mut int total = 0
            for key in nodes { total = total + sum(nodes[key]) }
            return total
        }
        Link.Values(values) -> {
            mut int total = 0
            for key in values { for i in values[key] { total = total + values[key][i] } }
            return total
        }
        Link.Empty -> { return 0 }
    }
}
pub fn rebuild_link<type T>(Link<T> link, T value) Link<T> {
    if link {
        Link.Children(nodes) -> {
            mut [str]Tree<T> local = nodes
            for key in local { local[key] = rebuild<T>(local[key], value) }
            return Link.Children(local)
        }
        Link.Values(values) -> {
            mut [str]T local = values
            for key in local { local[key] = value }
            return Link.Values(local)
        }
        Link.Empty -> { return Link.Empty }
    }
}
";

const MIXED_DEFINITIONS: &str = r"
pub enum Tree<type T> { Leaf(T) Branch([str]Link<T>) Empty }
pub struct Link<type T> { [str]T values [str]Tree<T> children }
pub fn children_link<type T>([str]Tree<T> nodes) Link<T> {
    return Link<T>{.values = [], .children = nodes}
}
pub fn values_link<type T>([str]T values) Link<T> {
    return Link<T>{.values = values, .children = []}
}
pub fn empty_link<type T>() Link<T> { return Link<T>{.values = [], .children = []} }
pub fn sum_link(Link<int[]> link) int {
    mut int total = 0
    for key in link.values { for i in link.values[key] { total = total + link.values[key][i] } }
    for key in link.children { total = total + sum(link.children[key]) }
    return total
}
pub fn rebuild_link<type T>(Link<T> link, T value) Link<T> {
    mut Link<T> local = link
    for key in local.values { local.values[key] = value }
    for key in local.children { local.children[key] = rebuild<T>(local.children[key], value) }
    return local
}
";

const TREE_FUNCTIONS: &str = r#"
pub fn sum(Tree<int[]> tree) int {
    if tree {
        Tree.Leaf(value) -> {
            mut int total = 0
            for i in value { total = total + value[i] }
            return total
        }
        Tree.Branch(links) -> {
            mut int total = 0
            for key in links { total = total + sum_link(links[key]) }
            return total
        }
        Tree.Empty -> { return 0 }
    }
}
pub fn rebuild<type T>(Tree<T> tree, T value) Tree<T> {
    if tree {
        Tree.Leaf(payload) -> { return Tree.Leaf(value) }
        Tree.Branch(links) -> {
            mut [str]Link<T> local = links
            for key in local { local[key] = rebuild_link<T>(local[key], value) }
            return Tree.Branch(local)
        }
        Tree.Empty -> { return Tree.Empty }
    }
}
pub fn deepen<type T>(Tree<T> tree, uint depth) Tree<T> {
    mut Tree<T> local = tree
    mut uint level = 0
    while level < depth {
        local = Tree.Branch(["next": children_link<T>(["child": local])])
        level = level + 1
    }
    return local
}
"#;

const CHECKS: &str = r#"
test "map cycle traversal rebuilding and deep snapshots" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    mut int[] payload = [INITIAL, 8]
    $Tree<int[]> leaf = $Tree.Leaf(payload)
    $Tree<int[]> empty = $Tree.Empty
    $Tree<int[]> empty_branch = $Tree.Branch([])
    $Link<int[]> empty_children = $children_link<int[]>([])
    $Link<int[]> empty_values = $values_link<int[]>([])
    $Link<int[]> empty_link = $empty_link<int[]>()
    $Tree<int[]> base = $Tree.Branch([
        "nodes": $children_link<int[]>(["leaf": leaf, "empty": empty, "branch": empty_branch]),
        "values": $values_link<int[]>(["one": [9], "two": [10, 11], "zero": []]),
        "children": empty_children, "payloads": empty_values, "empty": empty_link
    ])
    $Tree<int[]> original = $deepen<int[]>(base, 24)
    $Tree<int[]> snapshot = original
    payload[index] = 90
    assert $sum(original) == 45 and $sum(leaf) == 15
    assert $sum(empty) == 0 and $sum(empty_branch) == 0
    assert $sum_link(empty_children) == 0 and $sum_link(empty_values) == 0
    assert $sum_link(empty_link) == 0
    assert $rebuild<int[]>(empty, [13]) == empty
    assert $rebuild<int[]>(empty_branch, [13]) == empty_branch
    assert $rebuild_link<int[]>(empty_children, [13]) == empty_children
    assert $rebuild_link<int[]>(empty_values, [13]) == empty_values
    assert $rebuild_link<int[]>(empty_link, [13]) == empty_link
    mut int[] replacement = [13]
    mut $Tree<int[]> rebuilt = $rebuild<int[]>(original, replacement)
    $Tree<int[]> rebuilt_snapshot = rebuilt
    replacement[index] = 91
    $Tree<int[]> changed_base = $Tree.Branch([
        "nodes": $children_link<int[]>([
            "leaf": $Tree.Leaf([13]), "empty": empty, "branch": empty_branch
        ]),
        "values": $values_link<int[]>(["one": [13], "two": [13], "zero": [13]]),
        "children": empty_children, "payloads": empty_values, "empty": empty_link
    ])
    $Tree<int[]> expected = $deepen<int[]>(changed_base, 24)
    assert rebuilt == expected and $sum(rebuilt) == 52
    if rebuilt {
        $Tree.Branch(links) -> {
            mut [str]$Link<int[]> extracted = links
            extracted["next"] = $rebuild_link<int[]>(extracted["next"], [17])
            extracted["added"] = $values_link<int[]>(["extra": [19]])
            rebuilt = $Tree.Branch(extracted)
            assert links.len == 1 and extracted.len == 2
            assert $sum_link(links["next"]) == 52
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $sum(rebuilt) == 87 and rebuilt != rebuilt_snapshot
    assert rebuilt_snapshot == expected
    assert original == snapshot and $sum(original) == 45
    assert $sum(base) == 45 and replacement == [91]
    mut $Tree<int[]> copy = base
    if copy {
        $Tree.Branch(links) -> {
            mut [str]$Link<int[]> extracted = links
            extracted["values"] = $values_link<int[]>(["replacement": [20]])
            extracted["added"] = $children_link<int[]>(["new": $Tree.Leaf([21])])
            copy = $Tree.Branch(extracted)
            assert links.len == 5 and extracted.len == 6
            assert $sum_link(links["values"]) == 30
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $sum(copy) == 56 and $sum(base) == 45 and copy != base
    @println("map cycle checked")
}
test "map cycle string specialization" {
    mut str payload = "old"
    $Tree<str> base = $Tree.Branch([
        "nodes": $children_link<str>(["leaf": $Tree.Leaf(payload), "empty": $Tree.Empty]),
        "values": $values_link<str>(["side": "side"])
    ])
    $Tree<str> original = $deepen<str>(base, 12)
    payload[0] = 'O'
    mut str replacement = "new"
    $Tree<str> rebuilt = $rebuild<str>(original, replacement)
    replacement[0] = 'N'
    $Tree<str> expected = $deepen<str>($Tree.Branch([
        "nodes": $children_link<str>(["leaf": $Tree.Leaf("new"), "empty": $Tree.Empty]),
        "values": $values_link<str>(["side": "new"])
    ]), 12)
    assert rebuilt == expected and rebuilt != original
    assert original == $deepen<str>(base, 12)
    assert payload == "Old" and replacement == "New"
    @println("str map cycle checked")
}
"#;

const ENUM_CHECKS: &str = r#"
test "map enum extracted payload copies" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    mut [str]int[] payload = ["value": [INITIAL, 8], "empty": []]
    $Link<int[]> original = $Link.Values(payload)
    payload["value"][index] = 90
    if original {
        $Link.Values(values) -> {
            mut [str]int[] extracted = values
            extracted["value"][index] = 20
            extracted["empty"] = extracted["empty"] <> [21]
            extracted["added"] = [22]
            $Link<int[]> copy = $Link.Values(extracted)
            assert values["value"] == [7, 8] and values["empty"].len == 0
            assert values.len == 2 and extracted.len == 3
            assert $sum_link(copy) == 71 and $sum_link(original) == 15
        }
        $Link.Children(nodes) -> { assert false }
        $Link.Empty -> { assert false }
    }
    @println("enum map payload checked")
}
"#;

const MIXED_CHECKS: &str = r#"
test "map mixed nested field copies" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    $Link<int[]> original = $Link<int[]>{
        .values = ["value": [INITIAL, 8]],
        .children = ["nested": $Tree.Branch([
            "link": $Link<int[]>{.values = ["side": [9]], .children = ["leaf": $Tree.Leaf([10])]}
        ])]
    }
    mut $Link<int[]> copy = original
    copy.values["value"][index] = 20
    if copy.children["nested"] {
        $Tree.Branch(links) -> {
            mut [str]$Link<int[]> extracted = links
            extracted["link"].values["side"][index] = 21
            extracted["link"].children["added"] = $Tree.Leaf([22])
            copy.children["nested"] = $Tree.Branch(extracted)
            assert links["link"].values["side"] == [9]
            assert links["link"].children.len == 1
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $sum_link(copy) == 81 and $sum_link(original) == 34
    assert copy != original and original.values["value"] == [7, 8]
    @println("mixed map fields checked")
}
"#;

#[test]
fn local_map_backed_mutually_recursive_generic_enums() {
    run_cases(
        ENUM_DEFINITIONS,
        ENUM_CHECKS,
        false,
        "enum map payload checked\n",
    );
}

#[test]
fn imported_map_backed_mutually_recursive_generic_enums() {
    run_cases(
        ENUM_DEFINITIONS,
        ENUM_CHECKS,
        true,
        "enum map payload checked\n",
    );
}

#[test]
fn local_map_backed_mixed_generic_struct_enum_cycle() {
    run_cases(
        MIXED_DEFINITIONS,
        MIXED_CHECKS,
        false,
        "mixed map fields checked\n",
    );
}

#[test]
fn imported_map_backed_mixed_generic_struct_enum_cycle() {
    run_cases(
        MIXED_DEFINITIONS,
        MIXED_CHECKS,
        true,
        "mixed map fields checked\n",
    );
}

fn run_cases(definitions: &str, extra: &str, imported: bool, extra_output: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    let definitions = format!("{definitions}\n{TREE_FUNCTIONS}");
    let (prefix, prelude) = if imported {
        fs::write(directory.path().join("cycle.nc"), &definitions).unwrap();
        ("cycle.", "import { \"cycle\" as cycle }".to_owned())
    } else {
        ("", definitions)
    };
    let expected = format!("map cycle checked\nstr map cycle checked\n{extra_output}");
    for initial in ["7", "seed + 6"] {
        let checks = format!("{CHECKS}\n{extra}")
            .replace('$', prefix)
            .replace("INITIAL", initial);
        fs::write(&input, format!("{prelude}\n{checks}")).unwrap();
        run_both(&input, expected.as_bytes());
    }
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
            "input={}, release={release}: {}",
            input.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "release={release}");
        assert_eq!(output.stderr, b"", "release={release}");
    }
}
