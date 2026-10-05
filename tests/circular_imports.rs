use std::{fs, path::Path, process::Command};

fn execute(input: &Path, mode: &str, expected: &[u8]) {
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg(mode);
        if release {
            command.arg("-r");
        }
        let output = command.arg(input).output().unwrap();
        assert!(
            output.status.success(),
            "mode={mode}, release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "mode={mode}, release={release}");
        assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    }
}

#[test]
fn two_file_cycles_resolve_mutual_calls_public_types_and_generics() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("even.nc"),
        r#"
import { "odd" as odd }
pub type Count = int
pub struct Record { Count value }
pub fn identity<type T>(T value) T { return value }
pub fn even(int n) bool {
    if n == 0 { true -> { return true } _ -> { return odd.odd(n - 1) } }
}
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("odd.nc"),
        r#"
import { "even" as even }
pub fn odd(int n) bool {
    if n == 0 { true -> { return false } _ -> { return even.even(n - 1) } }
}
pub fn record(even.Count value) even.Record {
    return even.Record{.value = even.identity<even.Count>(value)}
}
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
import { "even" as even "odd" as odd }
int seed = @as(int, @args().len)
even.Record value = odd.record(@as(even.Count, seed + 6))
@println(even.even(8), ":", odd.odd(seed + 6), ":", @as(int, value.value))
test "cyclic exported declarations" {
    assert even.even(8) and odd.odd(seed + 6)
    assert @as(int, value.value) == 7
    @println("cycle checked")
}
"#,
    )
    .unwrap();
    execute(&input, "run", b"true:true:7\n");
    execute(&input, "test", b"cycle checked\n");
}

#[test]
fn three_file_root_cycles_and_self_imports_preserve_root_exports() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("a.nc"),
        r#"
import { "b" as b }
pub fn answer(int n) int { return b.answer(n) + 1 }
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("b.nc"),
        r#"
import { "main" as root }
pub fn answer(int n) int { return root.base(n) + 1 }
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
import { "a" as a "main" as self }
pub fn base(int n) int { return n + 10 }
@println(a.answer(@as(int, @args().len)), ":", self.base(2))
test "root back edge" {
    assert a.answer(@as(int, @args().len)) == 13
    assert self.base(2) == 12
    @println("root checked")
}
"#,
    )
    .unwrap();
    execute(&input, "run", b"13:12\n");
    execute(&input, "test", b"root checked\n");
}

#[test]
fn recursive_generic_types_can_cross_circular_imports() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("a.nc"),
        r#"
import { "b" as b }
pub struct Node<type T> { T value b.Branch<T>[] children }
pub fn leaf<type T>(T value) Node<T> { return Node<T>{.value = value, .children = []} }
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("b.nc"),
        r#"
import { "a" as a }
pub struct Branch<type T> { a.Node<T> node }
pub fn wrap<type T>(a.Node<T> node) Branch<T> { return Branch<T>{.node = node} }
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
import { "a" as a "b" as b }
test "recursive cyclic types" {
    int seed = @as(int, @args().len)
    mut a.Node<int[]> leaf = a.leaf<int[]>([seed, 2])
    mut a.Node<int[]> root = a.Node<int[]>{.value = [3], .children = [b.wrap<int[]>(leaf)]}
    a.Node<int[]> snapshot = root
    leaf.value[0] = 8
    root.children[0].node.value[0] = 9
    assert snapshot.children[0].node.value == [1, 2]
    assert leaf.value == [8, 2]
    assert root.children[0].node.value == [9, 2]
    @println("types checked")
}
"#,
    )
    .unwrap();
    execute(&input, "test", b"types checked\n");
    execute(&input, "run", b"");
}

#[test]
fn back_edges_do_not_hide_private_missing_or_ill_typed_exports() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    let source = "import { \"a\" as a };_ = a.value(1)";
    fs::write(&input, source).unwrap();
    fs::write(
        directory.path().join("a.nc"),
        "import { \"b\" as b };pub fn value(int n) int { return b.value(n) }",
    )
    .unwrap();
    let imported = directory.path().join("b.nc");
    for (body, expected) in [
        ("return a.hidden(n)", "does not export `hidden`"),
        ("return a.absent(n)", "does not export `absent`"),
        ("return a.value(true)", "expected `int`, found `bool`"),
        ("return a.value()", "incorrect number of arguments"),
        ("return true", "expected `int`, found `bool`"),
    ] {
        fs::write(
            &imported,
            format!("import {{ \"a\" as a }}\npub fn value(int n) int {{ {body} }}"),
        )
        .unwrap();
        // A private declaration exists, but the back-edge must not expose it.
        fs::write(
            directory.path().join("a.nc"),
            "import { \"b\" as b }\nfn hidden(int n) int { return n }\npub fn value(int n) int { return b.value(n) }",
        )
        .unwrap();
        for release in [false, true] {
            let error = ncc::compile_source_with_options(source, &input, release).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(error.0[0].path.as_deref(), Some(imported.as_path()));
            assert!(error.render(source, &input).contains("b.nc:2:"), "{error}");
        }
    }
}

#[test]
fn cyclic_imported_tests_keep_dependencies_but_normal_runs_ignore_their_bodies() {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    fs::write(&input, "import { \"a\" as a };@println(\"normal\")").unwrap();
    fs::write(
        directory.path().join("a.nc"),
        r#"
import { "b" as b }
pub fn value(int n) int { return n + 1 }
@println("unrelated")
test "imported cyclic dependency" {
    assert b.value(@as(int, @args().len)) == 3
    @println("imported checked")
}
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("b.nc"),
        "import { \"a\" as a };pub fn value(int n) int { return a.value(n) + 1 }",
    )
    .unwrap();
    execute(&input, "test", b"imported checked\n");
    fs::write(
        directory.path().join("a.nc"),
        "import { \"b\" as b };pub fn value(int n) int { return n };test \"ignored\" { _ = missing }",
    )
    .unwrap();
    execute(&input, "run", b"normal\n");
}
