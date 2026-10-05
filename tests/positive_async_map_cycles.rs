use std::{fs, process::Command};

const ENUM_DECLARATIONS: &str = r"
enum Tree<type T> { Leaf(T) Branch([str]Link<T>) Empty }
enum Link<type T> { Children([str]Tree<T>) Values([str]T) Empty }
fn children<type T>([str]Tree<T> nodes) Link<T> { return Link.Children(nodes) }
fn values<type T>([str]T data) Link<T> { return Link.Values(data) }
fn empty<type T>() Link<T> { return Link.Empty }
fn shift_link<type T>(Link<T> link, int delta) Link<T> {
    if link {
        Link.Children(nodes) -> {
            mut [str]Tree<T> local = nodes
            for key in local { local[key] = shift_tree<T>(local[key], delta) }
            return Link.Children(local)
        }
        Link.Values(data) -> {
            mut [str]T local = data
            for key in local { local[key] = shift_data<T>(local[key], delta) }
            return Link.Values(local)
        }
        Link.Empty -> { return Link.Empty }
    }
}
";

const MIXED_DECLARATIONS: &str = r"
enum Tree<type T> { Leaf(T) Branch([str]Link<T>) Empty }
struct Link<type T> { [str]T values [str]Tree<T> children }
fn children<type T>([str]Tree<T> nodes) Link<T> { return Link<T>{.values = [], .children = nodes} }
fn values<type T>([str]T data) Link<T> { return Link<T>{.values = data, .children = []} }
fn empty<type T>() Link<T> { return Link<T>{.values = [], .children = []} }
fn shift_link<type T>(Link<T> link, int delta) Link<T> {
    mut Link<T> local = link
    for key in local.values { local.values[key] = shift_data<T>(local.values[key], delta) }
    for key in local.children { local.children[key] = shift_tree<T>(local.children[key], delta) }
    return local
}
";

const TREE_FUNCTIONS: &str = r#"
fn shift_data<type T>(T data, int delta) T {
    mut T local = data
    for key in local { for i in local[key] { local[key][i] = local[key][i] + delta } }
    return local
}
fn shift_tree<type T>(Tree<T> tree, int delta) Tree<T> {
    if tree {
        Tree.Leaf(data) -> { return Tree.Leaf(shift_data<T>(data, delta)) }
        Tree.Branch(links) -> {
            mut [str]Link<T> local = links
            for key in local { local[key] = shift_link<T>(local[key], delta) }
            return Tree.Branch(local)
        }
        Tree.Empty -> { return Tree.Empty }
    }
}
fn sample(int seed, int delta) Tree<[str]int[]> {
    Tree<[str]int[]> leaf = Tree.Leaf(["items": [seed + delta, 2 + delta], "empty": []])
    Tree<[str]int[]> middle = Tree.Branch([
        "inner": children<[str]int[]>(["leaf": leaf, "empty": Tree.Empty, "branch": Tree.Branch([])])
    ])
    return Tree.Branch([
        "nodes": children<[str]int[]>(["middle": middle, "leaf": leaf]),
        "values": values<[str]int[]>(["side": ["items": [3 + delta], "empty": []]]),
        "children": children<[str]int[]>([]), "payloads": values<[str]int[]>([]),
        "empty": empty<[str]int[]>(),
    ])
}
"#;

const SNAPSHOT_CHECKS: &str = r#"
test "locked recursive map cycle snapshots" {
    int seed = @as(int, @args().len)
    mut Tree<[str]int[]> original = sample(seed, 0)
    Tree<[str]int[]> expected = sample(1, 0)
    mutex Tree<[str]int[]> payload = original
    original = shift_tree<[str]int[]>(original, 9)
    fn read() Tree<[str]int[]> { lock payload { return shift_tree<[str]int[]>(payload, 0) } }
    fut Tree<[str]int[]> first = async read()
    fut Tree<[str]int[]> second = async read()
    mut Tree<[str]int[]> result = await first
    Tree<[str]int[]> other = await second
    Tree<[str]int[]> snapshot = result
    assert result == expected and other == expected
    if result {
        Tree.Branch(links) -> {
            mut [str]Link<[str]int[]> extracted = links
            extracted["nodes"] = shift_link<[str]int[]>(extracted["nodes"], 4)
            extracted["added"] = values<[str]int[]>(["new": ["items": [8]]])
            result = Tree.Branch(extracted)
            assert links.len == 5 and extracted.len == 6
            assert links["nodes"] == children<[str]int[]>([
                "middle": Tree.Branch(["inner": children<[str]int[]>([
                    "leaf": Tree.Leaf(["items": [1, 2], "empty": []]),
                    "empty": Tree.Empty, "branch": Tree.Branch([])
                ])]),
                "leaf": Tree.Leaf(["items": [1, 2], "empty": []])
            ])
        }
        Tree.Leaf(data) -> { assert false }
        Tree.Empty -> { assert false }
    }
    assert result != expected and snapshot == expected and other == expected
    lock payload { assert payload == expected; payload = result }
    Tree<[str]int[]> written = result
    result = shift_tree<[str]int[]>(result, 7)
    lock payload { assert payload == written and payload != result }
    assert snapshot == expected and other == expected
    assert original == sample(1, 9)
    @println("map cycle snapshots checked")
}
"#;

const UPDATE_CHECKS: &str = r#"
test "commutative locked recursive map cycle updates" {
    int seed = @as(int, @args().len)
    Tree<[str]int[]> original = sample(seed, 0)
    Tree<[str]int[]> snapshot = original
    mutex Tree<[str]int[]> payload = original
    fn add(int delta) int {
        lock payload { payload = shift_tree<[str]int[]>(payload, delta) }
        return delta
    }
    (fn(int) int) worker = fn(int delta) int {
        lock payload { payload = shift_tree<[str]int[]>(payload, delta) }
        return delta
    }
    fut int first = async add(seed)
    fut int second = async worker(seed + 1)
    assert await first == 1
    assert await second == 2
    Tree<[str]int[]> expected = sample(1, 3)
    fn read() Tree<[str]int[]> { lock payload { return payload } }
    fut Tree<[str]int[]> pending = async read()
    Tree<[str]int[]> retained = await pending
    assert retained == expected
    lock payload {
        assert payload == expected
        mut Tree<[str]int[]> local = payload
        if local {
            Tree.Branch(links) -> {
                mut [str]Link<[str]int[]> extracted = links
                extracted["nodes"] = shift_link<[str]int[]>(extracted["nodes"], 6)
                extracted["new"] = children<[str]int[]>(["leaf": Tree.Leaf(["items": [9]])])
                local = Tree.Branch(extracted)
                assert links.len == 5 and extracted.len == 6
            }
            Tree.Leaf(data) -> { assert false }
            Tree.Empty -> { assert false }
        }
        assert local != expected and payload == expected
        payload = local
        Tree<[str]int[]> written = local
        local = shift_tree<[str]int[]>(local, 5)
        assert payload == written and payload != local
    }
    assert retained == expected and snapshot == sample(1, 0) and original == snapshot
    Tree<[str]int[]> empty_tree = Tree.Empty
    lock payload { payload = empty_tree }
    lock payload { assert payload == empty_tree }
    @println("map cycle updates checked")
}
"#;

#[test]
fn mutually_recursive_enum_maps_preserve_locked_async_copies_and_updates() {
    run_cases(ENUM_DECLARATIONS);
}

#[test]
fn mutually_recursive_struct_enum_maps_preserve_locked_async_copies_and_updates() {
    run_cases(MIXED_DECLARATIONS);
}

fn run_cases(declarations: &str) {
    for (checks, stdout) in [
        (SNAPSHOT_CHECKS, "map cycle snapshots checked\n"),
        (UPDATE_CHECKS, "map cycle updates checked\n"),
    ] {
        let source = format!("{declarations}{TREE_FUNCTIONS}{checks}");
        for (phase, source) in [
            (
                "constant",
                source.replace("int seed = @as(int, @args().len)", "int seed = 1"),
            ),
            ("runtime", source),
        ] {
            for release in [false, true] {
                let directory = ncc::temp::Directory::new().unwrap();
                let input = directory.path().join("test.nc");
                fs::write(&input, &source).unwrap();
                let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
                command.arg("test");
                if release {
                    command.arg("-r");
                }
                let output = command.arg(&input).output().unwrap();
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
}
