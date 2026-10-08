use std::{fs, path::Path, process::Command};

#[test]
fn function_scopes_preserve_module_names_nested_shadows_and_generic_hints() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("scopes.nc"),
        r#"
int value = 40
pub enum Choice<T> { Value(T) Empty }
fn identity<T>(T value) T { return value }
pub fn shadow(int value) int {
    {
        str value = "inner"
        _ = identity<str>(value)
    }
    fn nested = fn(str value) str {
        { int value = 9; _ = identity<int>(value) }
        return identity<str>(value)
    }
    _ = nested("nested")
    return identity<int>(value)
}
pub fn global() int { return value }
pub fn choice<T>(T value) Choice<T> {
    { str value = "temporary"; _ = identity<str>(value) }
    fn nested = fn(str value) Choice<str> { return Choice.Value(value) }
    Choice<str> inner = nested("nested")
    if inner {
        Choice.Value(text) -> { _ = identity<str>(text) }
        Choice.Empty -> {}
    }
    return Choice.Value(identity<T>(value))
}
"#,
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
import { "scopes" as scopes }
test "function scopes" {
    int seed = @as(int, @args().len)
    assert scopes.shadow(seed + 6) == 7
    assert scopes.global() == 40
    scopes.Choice<int> number = scopes.choice<int>(seed + 6)
    scopes.Choice<str> text = scopes.choice<str>("outer")
    if number {
        scopes.Choice.Value(value) -> { assert value == 7 }
        scopes.Choice.Empty -> { assert false }
    }
    if text {
        scopes.Choice.Value(value) -> { assert value == "outer" }
        scopes.Choice.Empty -> { assert false }
    }
    assert scopes.shadow(seed + 7) == 8
    assert scopes.global() == 40
    @println("scopes preserved")
}
"#,
    )
    .unwrap();
    run_both(&input, b"scopes preserved\n");
}

#[test]
fn generic_type_parameters_hide_module_types_without_affecting_nongeneric_types() {
    let directory = ncc::temp::Directory::new().unwrap();
    fs::write(
        directory.path().join("types.nc"),
        r"
pub type T = int
pub struct Generic<T> { T value }
pub enum GenericChoice<T> { Value(T) Empty }
pub struct Plain { T value }
pub enum PlainChoice { Value(T) Empty }
pub fn wrap<T>(T value) Generic<T> { return Generic<T>{.value = value} }
pub fn choose<T>(T value) GenericChoice<T> { return GenericChoice.Value(value) }
pub fn plain(T value) Plain { return Plain{.value = value} }
pub fn plain_choice(T value) PlainChoice { return PlainChoice.Value(value) }
",
    )
    .unwrap();
    let input = directory.path().join("main.nc");
    fs::write(
        &input,
        r#"
import { "types" as types }
test "type scopes" {
    types.Generic<str> generic = types.wrap<str>("text")
    assert generic.value == "text"
    types.GenericChoice<str> generic_choice = types.choose<str>("payload")
    if generic_choice {
        types.GenericChoice.Value(value) -> { assert value == "payload" }
        types.GenericChoice.Empty -> { assert false }
    }
    types.T value = @as(types.T, @as(int, @args().len) + 6)
    types.Plain plain = types.plain(value)
    assert plain.value == value
    types.PlainChoice plain_choice = types.plain_choice(value)
    if plain_choice {
        types.PlainChoice.Value(payload) -> { assert payload == value }
        types.PlainChoice.Empty -> { assert false }
    }
    @println("type scopes preserved")
}
"#,
    )
    .unwrap();
    run_both(&input, b"type scopes preserved\n");
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
    }
}
