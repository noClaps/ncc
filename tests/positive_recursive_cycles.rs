use std::{fs, path::Path, process::Command};

const ENUM_DEFINITIONS: &str = r"
pub enum Tree<type T> { Leaf(T) Branch(Link<T>[]) Empty }
pub enum Link<type T> { Children(Tree<T>[]) Values(T[]) Empty }
pub fn children_link<type T>(Tree<T>[] nodes) Link<T> { return Link.Children(nodes) }
pub fn values_link<type T>(T[] values) Link<T> { return Link.Values(values) }
pub fn empty_link<type T>() Link<T> { return Link.Empty }
pub fn flatten_link<type T>(Link<T> link) T[] {
    if link {
        Link.Children(nodes) -> {
            mut T[] out = []
            for i in nodes { out = out <> flatten<T>(nodes[i]) }
            return out
        }
        Link.Values(values) -> { return values }
        Link.Empty -> { return [] }
    }
}
pub fn rebuild_link<type T>(Link<T> link, T value) Link<T> {
    if link {
        Link.Children(nodes) -> {
            mut Tree<T>[] local = nodes
            for i in local { local[i] = rebuild<T>(local[i], value) }
            return Link.Children(local)
        }
        Link.Values(values) -> {
            mut T[] local = values
            for i in local { local[i] = value }
            return Link.Values(local)
        }
        Link.Empty -> { return Link.Empty }
    }
}
pub fn change_child(Link<int[]> link, uint index, int value) Link<int[]> {
    if link {
        Link.Children(nodes) -> {
            mut Tree<int[]>[] local = nodes
            if local[index] {
                Tree.Leaf(data) -> {
                    mut int[] copy = data
                    copy[index] = value
                    local[index] = Tree.Leaf(copy)

                }
                Tree.Branch(links) -> { return link }
                Tree.Empty -> { return link }
            }

            return Link.Children(local)
        }
        Link.Values(values) -> { return link }
        Link.Empty -> { return link }
    }
}
";

const MIXED_DEFINITIONS: &str = r"
pub enum Tree<type T> { Leaf(T) Branch(Link<T>[]) Empty }
pub struct Link<type T> { T[] values Tree<T>[] children }
pub fn children_link<type T>(Tree<T>[] nodes) Link<T> {
    return Link<T>{.values = [], .children = nodes}
}
pub fn values_link<type T>(T[] values) Link<T> {
    return Link<T>{.values = values, .children = []}
}
pub fn empty_link<type T>() Link<T> { return Link<T>{.values = [], .children = []} }
pub fn flatten_link<type T>(Link<T> link) T[] {
    mut T[] out = link.values
    for i in link.children { out = out <> flatten<T>(link.children[i]) }
    return out
}
pub fn rebuild_link<type T>(Link<T> link, T value) Link<T> {
    mut Link<T> local = link
    for i in local.values { local.values[i] = value }
    for i in local.children { local.children[i] = rebuild<T>(local.children[i], value) }
    return local
}
pub fn change_child(Link<int[]> link, uint index, int value) Link<int[]> {
    mut Link<int[]> local = link
    if local.children[index] {
        Tree.Leaf(data) -> {
            mut int[] copy = data
            copy[index] = value
            local.children[index] = Tree.Leaf(copy)

        }
        Tree.Branch(links) -> { return link }
        Tree.Empty -> { return link }
    }
    return local
}
";

const TREE_FUNCTIONS: &str = r"
pub fn flatten<type T>(Tree<T> tree) T[] {
    if tree {
        Tree.Leaf(value) -> { return [value] }
        Tree.Branch(links) -> {
            mut T[] out = []
            for i in links { out = out <> flatten_link<T>(links[i]) }
            return out
        }
        Tree.Empty -> { return [] }
    }
}
pub fn rebuild<type T>(Tree<T> tree, T value) Tree<T> {
    if tree {
        Tree.Leaf(payload) -> { return Tree.Leaf(value) }
        Tree.Branch(links) -> {
            mut Link<T>[] local = links
            for i in local { local[i] = rebuild_link<T>(local[i], value) }
            return Tree.Branch(local)
        }
        Tree.Empty -> { return Tree.Empty }
    }
}
";

const ARRAY_CHECKS: &str = r#"
test "recursive cycle array traversal rebuild and copies" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    mut int[] payload = [INITIAL, 8]
    $Tree<int[]> leaf = $Tree.Leaf(payload)
    $Tree<int[]> empty = $Tree.Empty
    $Tree<int[]> empty_branch = $Tree.Branch([])
    $Link<int[]> empty_children = $children_link<int[]>([])
    $Link<int[]> empty_values = $values_link<int[]>([])
    $Link<int[]> empty_link = $empty_link<int[]>()
    $Link<int[]> children = $children_link<int[]>([leaf, empty, empty_branch])
    $Tree<int[]> middle = $Tree.Branch([children, empty_children, empty_values, empty_link])
    $Tree<int[]> original = $Tree.Branch([
        $children_link<int[]>([middle, leaf]), $values_link<int[]>([[9], [10, 11]])
    ])
    $Tree<int[]> snapshot = original
    mut $Tree<int[]> copy = original
    payload[index] = 12
    int[][] expected = [[7, 8], [7, 8], [9], [10, 11]]
    assert $flatten<int[]>(original) == expected
    assert $flatten<int[]>(leaf) == [[7, 8]]
    assert $flatten<int[]>(empty).len == 0 and $flatten<int[]>(empty_branch).len == 0
    assert $flatten_link<int[]>(empty_children).len == 0
    assert $flatten_link<int[]>(empty_values).len == 0 and $flatten_link<int[]>(empty_link).len == 0
    assert $rebuild<int[]>(empty, [13]) == empty
    assert $rebuild<int[]>(empty_branch, [13]) == empty_branch
    assert $rebuild_link<int[]>(empty_children, [13]) == empty_children
    assert $rebuild_link<int[]>(empty_values, [13]) == empty_values
    assert $rebuild_link<int[]>(empty_link, [13]) == empty_link

    mut int[] replacement = [13]
    mut $Tree<int[]> rebuilt = $rebuild<int[]>(original, replacement)
    $Tree<int[]> rebuilt_snapshot = rebuilt
    replacement[index] = 14
    $Tree<int[]> changed_leaf = $Tree.Leaf([13])
    $Tree<int[]> changed_middle = $Tree.Branch([
        $children_link<int[]>([changed_leaf, empty, empty_branch]),
        empty_children, empty_values, empty_link
    ])
    $Tree<int[]> expected_rebuilt = $Tree.Branch([
        $children_link<int[]>([changed_middle, changed_leaf]), $values_link<int[]>([[13], [13]])
    ])
    assert rebuilt == expected_rebuilt
    assert $flatten<int[]>(rebuilt) == [[13], [13], [13], [13]]
    if middle {
        $Tree.Branch(links) -> {
            mut $Link<int[]>[] extracted = links
            extracted[index] = $change_child(extracted[index], index, 15)
            extracted = extracted <> [$values_link<int[]>([[16]])]
            copy = $Tree.Branch(extracted)
            assert links.len == 4 and extracted.len == 5
            assert $flatten_link<int[]>(links[index]) == [[7, 8]]
            int[][] changed = [[15, 8], [16]]
            assert $flatten<int[]>(copy) == changed
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    if rebuilt {
        $Tree.Branch(links) -> {
            mut $Link<int[]>[] extracted = links
            extracted[index] = $rebuild_link<int[]>(extracted[index], [17])
            rebuilt = $Tree.Branch(extracted)
            assert $flatten_link<int[]>(links[index]) == [[13], [13]]
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $flatten<int[]>(rebuilt) == [[17], [17], [13], [13]]
    assert rebuilt_snapshot == expected_rebuilt
    assert original == snapshot and $flatten<int[]>(original) == expected
    assert copy != original and rebuilt != original
    if leaf {
        $Tree.Leaf(data) -> {
            mut int[] extracted = data
            extracted[index] = 18
            assert data == [7, 8] and extracted == [18, 8]
        }
        $Tree.Branch(links) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $flatten<int[]>(leaf) == [[7, 8]]
    @println("array cycle checked")
}
"#;

const DEEP_TREE_CHECKS: &str = r#"
fn deepen<type T>($Tree<T> base) $Tree<T> {
    mut $Tree<T> tree = base
    mut uint depth = 0
    while depth < 28 {
        tree = $Tree.Branch([$children_link<T>([tree])])
        depth = depth + 1
    }
    return tree
}
test "deep finite recursive tree traversal rebuild and copies" {
    int seed = @as(int, @args().len)
    uint index = @args().len - 1
    mut int[] payload = [INITIAL, 8]
    $Tree<int[]> base = $Tree.Branch([
        $children_link<int[]>([$Tree.Leaf(payload), $Tree.Empty, $Tree.Leaf([9])]),
        $values_link<int[]>([[10, 11]]), $empty_link<int[]>()
    ])
    mut $Tree<int[]> source = deepen<int[]>(base)
    $Tree<int[]> snapshot = source
    payload[index] = 12
    int[][] expected = [[7, 8], [9], [10, 11]]
    assert $flatten<int[]>(source) == expected
    assert source == deepen<int[]>(base)

    mut int[] replacement = [13, 14]
    mut $Tree<int[]> rebuilt = $rebuild<int[]>(source, replacement)
    $Tree<int[]> rebuilt_snapshot = rebuilt
    replacement[index] = 15
    $Tree<int[]> expected_base = $Tree.Branch([
        $children_link<int[]>([$Tree.Leaf([13, 14]), $Tree.Empty, $Tree.Leaf([13, 14])]),
        $values_link<int[]>([[13, 14]]), $empty_link<int[]>()
    ])
    $Tree<int[]> expected_rebuilt = deepen<int[]>(expected_base)
    assert rebuilt == expected_rebuilt and rebuilt != source
    assert $flatten<int[]>(rebuilt) == [[13, 14], [13, 14], [13, 14]]

    if rebuilt {
        $Tree.Branch(links) -> {
            mut $Link<int[]>[] extracted = links
            extracted[index] = $rebuild_link<int[]>(extracted[index], [16])
            extracted = extracted <> [$values_link<int[]>([[17]])]
            rebuilt = $Tree.Branch(extracted)
            assert links.len == 1 and extracted.len == 2
            assert $flatten_link<int[]>(links[index]) == [[13, 14], [13, 14], [13, 14]]
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $flatten<int[]>(rebuilt) == [[16], [16], [16], [17]]
    assert rebuilt_snapshot == expected_rebuilt
    assert source == snapshot and $flatten<int[]>(source) == expected

    mut int[][] extracted = $flatten<int[]>(snapshot)
    extracted[index][index] = 18
    int[][] expected_extracted = [[18, 8], [9], [10, 11]]
    assert extracted == expected_extracted
    source = $rebuild<int[]>(source, [19])
    assert $flatten<int[]>(source) == [[19], [19], [19]]
    assert snapshot == deepen<int[]>(base) and $flatten<int[]>(snapshot) == expected
    assert rebuilt_snapshot == expected_rebuilt
    assert $flatten<int[]>(rebuilt) == [[16], [16], [16], [17]]
    assert payload == [12, 8] and replacement == [15, 14]
    @println("deep tree checked")
}
"#;

const ENUM_CHECKS: &str = r#"
test "mutual enum pattern extracted values preserve nested copies" {
    uint index = @args().len - 1
    int seed = @as(int, @args().len)
    mut int[][] payload = [[INITIAL, 8], [9]]
    $Link<int[]> original = $Link.Values(payload)
    $Link<int[]> snapshot = original
    mut $Link<int[]> copy = original
    $Tree<int[]> tree = $Tree.Branch([$children_link<int[]>([
        $Tree.Branch([original]), $Tree.Empty
    ])])
    payload[index][index] = 19
    if copy {
        $Link.Values(values) -> {
            mut int[][] extracted = values
            extracted[index][index] = 20
            extracted[1] = extracted[1] <> [21]
            extracted = extracted <> [[22]]
            copy = $Link.Values(extracted)
            int[][] expected = [[7, 8], [9]]
            int[][] changed = [[20, 8], [9, 21], [22]]
            assert values == expected and extracted == changed
        }
        $Link.Children(nodes) -> { assert false }
        $Link.Empty -> { assert false }
    }
    int[][] expected = [[7, 8], [9]]
    int[][] changed = [[20, 8], [9, 21], [22]]
    assert $flatten_link<int[]>(copy) == changed
    assert $flatten_link<int[]>(original) == expected
    assert $flatten<int[]>(tree) == expected
    assert original == snapshot and copy != original
    $Link<int[]> rebuilt = $rebuild_link<int[]>(copy, [23])
    assert $flatten_link<int[]>(rebuilt) == [[23], [23], [23]]
    assert $flatten_link<int[]>(copy) == changed
    assert $flatten<int[]>($rebuild<int[]>(tree, [24])) == [[24], [24]]
    assert $flatten<int[]>(tree) == expected
    @println("enum values checked")
}
"#;

const MIXED_CHECKS: &str = r#"
test "mixed struct fields preserve independent nested copies" {
    uint index = @args().len - 1
    int seed = @as(int, @args().len)
    $Link<int[]> original = $Link<int[]>{
        .values = [[INITIAL, 8]], .children = [$Tree.Branch([
            $Link<int[]>{.values = [[9]], .children = [$Tree.Leaf([INITIAL, 8])]}
        ])]
    }
    mut $Link<int[]> copy = original
    $Link<int[]> snapshot = original
    copy.values[index][index] = 19
    if copy.children[index] {
        $Tree.Branch(links) -> {
            mut $Link<int[]>[] extracted = links
            extracted[index].values[index][index] = 20
            extracted[index] = $change_child(extracted[index], index, 21)
            copy.children[index] = $Tree.Branch(extracted)
            assert links[index].values == [[9]]
            assert $flatten<int[]>(links[index].children[index]) == [[7, 8]]
        }
        $Tree.Leaf(data) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    int[][] changed = [[19, 8], [20], [21, 8]]
    int[][] expected = [[7, 8], [9], [7, 8]]
    assert $flatten_link<int[]>(copy) == changed
    assert $flatten_link<int[]>(original) == expected
    assert original == snapshot and copy != original
    $Link<int[]> rebuilt = $rebuild_link<int[]>(original, [22])
    assert $flatten_link<int[]>(rebuilt) == [[22], [22], [22]]
    assert rebuilt.values == [[22]] and original.values == [[7, 8]]
    @println("mixed fields checked")
}
"#;

const STRING_CHECKS: &str = r#"
test "recursive cycle str specialization" {
    uint index = @args().len - 1
    mut str payload = "old"
    $Tree<str> leaf = $Tree.Leaf(payload)
    $Tree<str> empty = $Tree.Empty
    $Tree<str> empty_branch = $Tree.Branch([])
    $Link<str> empty_children = $children_link<str>([])
    $Link<str> empty_values = $values_link<str>([])
    $Link<str> empty_link = $empty_link<str>()
    $Tree<str> middle = $Tree.Branch([
        $children_link<str>([leaf, empty, empty_branch]),
        empty_children, empty_values, empty_link
    ])
    $Tree<str> original = $Tree.Branch([
        $children_link<str>([middle]), $values_link<str>(["side"])
    ])
    payload[index] = 'O'
    assert $flatten<str>(original) == ["old", "side"]
    assert $flatten<str>(leaf) == ["old"] and payload == "Old"
    assert $rebuild<str>(empty, "new") == empty
    assert $rebuild<str>(empty_branch, "new") == empty_branch
    assert $rebuild_link<str>(empty_children, "new") == empty_children
    assert $rebuild_link<str>(empty_values, "new") == empty_values
    assert $rebuild_link<str>(empty_link, "new") == empty_link
    mut str replacement = "new"
    $Tree<str> rebuilt = $rebuild<str>(original, replacement)
    replacement[index] = 'N'
    $Tree<str> changed_middle = $Tree.Branch([
        $children_link<str>([$Tree.Leaf("new"), empty, empty_branch]),
        empty_children, empty_values, empty_link
    ])
    $Tree<str> expected = $Tree.Branch([
        $children_link<str>([changed_middle]), $values_link<str>(["new"])
    ])
    assert rebuilt == expected and $flatten<str>(rebuilt) == ["new", "new"]
    if leaf {
        $Tree.Leaf(data) -> {
            mut str extracted = data
            extracted[index] = 'X'
            assert data == "old" and extracted == "Xld"
        }
        $Tree.Branch(links) -> { assert false }
        $Tree.Empty -> { assert false }
    }
    assert $flatten<str>(original) == ["old", "side"]
    assert original != rebuilt and replacement == "New"
    @println("str cycle checked")
}
"#;

#[test]
fn local_mutually_recursive_generic_enums() {
    run_cases(ENUM_DEFINITIONS, false, false);
}

#[test]
fn imported_mutually_recursive_generic_enums() {
    run_cases(ENUM_DEFINITIONS, true, false);
}

#[test]
fn local_mixed_generic_struct_enum_cycle() {
    run_cases(MIXED_DEFINITIONS, false, true);
}

#[test]
fn imported_mixed_generic_struct_enum_cycle() {
    run_cases(MIXED_DEFINITIONS, true, true);
}

fn run_cases(definitions: &str, imported: bool, mixed: bool) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    let definitions = format!("{definitions}\n{TREE_FUNCTIONS}");
    let (prefix, prelude) = if imported {
        fs::write(directory.path().join("cycle.nc"), &definitions).unwrap();
        ("cycle.", "import { \"cycle\" as cycle }".to_owned())
    } else {
        ("", definitions)
    };
    let extra = if mixed { MIXED_CHECKS } else { ENUM_CHECKS };
    let expected: &[u8] = if mixed {
        b"array cycle checked\nmixed fields checked\nstr cycle checked\ndeep tree checked\n"
    } else {
        b"array cycle checked\nenum values checked\nstr cycle checked\ndeep tree checked\n"
    };
    for initial in ["7", "seed + 6"] {
        let checks = format!("{ARRAY_CHECKS}\n{extra}\n{STRING_CHECKS}\n{DEEP_TREE_CHECKS}")
            .replace('$', prefix)
            .replace("INITIAL", initial);
        fs::write(&input, format!("{prelude}\n{checks}")).unwrap();
        run_both(&input, expected);
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
