use std::{fs, path::Path};

fn rejects(source: &str, path: &Path, expected: &str) {
    for release in [false, true] {
        let Err(error) = ncc::compile_source_with_options(source, path, release) else {
            panic!(
                "release={release}, expected {expected:?}, but compilation succeeded\nsource:\n{source}"
            );
        };
        assert!(
            error.to_string().contains(expected),
            "release={release}, expected {expected:?}\nsource:\n{source}\ndiagnostic:\n{error}"
        );
    }
}

fn accepts(source: &str, path: &Path) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, path, release).unwrap_or_else(|error| {
            panic!("release={release}\nsource:\n{source}\ndiagnostic:\n{error}")
        });
    }
}

#[test]
fn selective_and_wildcard_import_syntax_is_forbidden() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(
        directory.path().join("library.nc"),
        "pub fn value() int { return 7 }",
    )
    .unwrap();
    for source in [
        r#"import { value } from "library""#,
        r#"import { value as selected } from "library""#,
        r#"import { * } from "library""#,
        r#"import { "library" as * }"#,
        r#"import { "library" as { value } }"#,
        r#"from "library" import value"#,
        r#"from "library" import *"#,
    ] {
        rejects(source, &path, "expected");
    }
    // Importing the whole module does not introduce its members as bare names.
    rejects(
        r#"import { "library" as lib };_ = value()"#,
        &path,
        "unknown name `value`",
    );
    accepts(r#"import { "library" as lib };_ = lib.value()"#, &path);
}

#[test]
fn imports_require_paths_aliases_and_readable_parseable_modules() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("library.nc"), "pub int value = 7").unwrap();
    for (source, expected) in [
        ("import { library as lib }", "expected import path"),
        ("import { 42 as lib }", "expected import path"),
        (r#"import { ("library") as lib }"#, "expected import path"),
        (r#"import { "library" }"#, "expected `as` after import path"),
        (r#"import { "library" as }"#, "expected identifier"),
        (r#"import { "absent" as lib }"#, "cannot import"),
        (
            r#"import { "library" as lib "library" as lib }"#,
            "duplicate module alias `lib`",
        ),
    ] {
        rejects(source, &path, expected);
    }
    fs::write(directory.path().join("broken.nc"), "pub fn broken( { }").unwrap();
    rejects(r#"import { "broken" as lib }"#, &path, "expected");
    fs::create_dir(directory.path().join("folder.nc")).unwrap();
    rejects(r#"import { "folder" as lib }"#, &path, "cannot import");
    accepts(
        r#"import { "library" as lib };int value = lib.value"#,
        &path,
    );
}

#[test]
fn private_module_symbols_and_transitive_import_aliases_are_hidden() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("helper.nc"), "pub int value = 7").unwrap();
    fs::write(
        directory.path().join("library.nc"),
        r#"
import { "helper" as helper }
int secret = 7
fn hidden() int { return secret }
type Count = int
struct Record { int value }
enum Choice { Value }
pub fn exposed() int { return hidden() + helper.value }
"#,
    )
    .unwrap();
    for (use_site, expected) in [
        ("_ = lib.secret", "does not export `secret`"),
        ("_ = lib.hidden()", "does not export `hidden`"),
        ("lib.Count value = 7", "unknown type"),
        ("lib.Record value = lib.Record{.value = 7}", "unknown type"),
        ("_ = lib.Choice.Value", "does not export `Choice`"),
        ("_ = lib.helper.value", "does not export `helper`"),
        ("_ = lib.absent", "does not export `absent`"),
    ] {
        rejects(
            &format!(r#"import {{ "library" as lib }};{use_site}"#),
            &path,
            expected,
        );
    }
    accepts(r#"import { "library" as lib };_ = lib.exposed()"#, &path);
}

#[test]
fn malformed_syntax_on_multi_file_cycle_graphs_keeps_its_source_location() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    let source = r#"import { "a" as a }"#;
    fs::write(&path, source).unwrap();
    fs::write(directory.path().join("a.nc"), r#"import { "b" as b }"#).unwrap();
    for back_edge in ["a", "main"] {
        // The back-edge forms a two- or three-file cycle, but parsing fails
        // before traversing it. This does not specify whether valid cycles work.
        let broken = format!("import {{ \"{back_edge}\" as back }}\npub fn broken( {{ }}");
        let imported = directory.path().join("b.nc");
        fs::write(&imported, &broken).unwrap();
        for release in [false, true] {
            let error = ncc::compile_source_with_options(source, &path, release).unwrap_err();
            assert!(error.to_string().contains("expected"), "{error}");
            assert_eq!(error.0[0].path.as_deref(), Some(imported.as_path()));
            assert!(error.render(source, &path).contains("b.nc:2:"), "{error}");
        }
    }
}

#[test]
fn missing_import_before_multi_file_back_edges_is_diagnosed() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    let source = r#"import { "a" as a }"#;
    fs::write(&path, source).unwrap();
    fs::write(directory.path().join("a.nc"), r#"import { "b" as b }"#).unwrap();
    for back_edge in ["a", "main"] {
        fs::write(
            directory.path().join("b.nc"),
            format!("import {{ \"missing\" as missing \"{back_edge}\" as back }}"),
        )
        .unwrap();
        // Only assert the missing-file error reached before the back-edge.
        for release in [false, true] {
            let error = ncc::compile_source_with_options(source, &path, release).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("cannot import"), "{error}");
            assert!(message.contains("missing.nc"), "{error}");
        }
    }
}

#[test]
fn extern_blocks_and_members_cannot_be_public_or_nested() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("native.c"), "").unwrap();
    for (source, expected) in [
        (
            r#"pub extern "native.c" as native { fn value() int = "value" }"#,
            "expected",
        ),
        (
            r#"extern "native.c" as native { pub fn value() int = "value" }"#,
            "extern blocks may only contain functions",
        ),
        (
            r#"fn local() { extern "native.c" as native { fn value() int = "value" } }"#,
            "expected",
        ),
        (
            r#"extern "native.c" as native { int value = 7 }"#,
            "extern blocks may only contain functions",
        ),
    ] {
        rejects(source, &path, expected);
    }
}

#[test]
fn extern_aliases_do_not_escape_through_imported_modules() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("native.c"), "").unwrap();
    fs::write(
        directory.path().join("library.nc"),
        r#"
extern "native.c" as native { fn value(int n) int = "native_value" }
pub fn value(int n) int { return native.value(n) }
"#,
    )
    .unwrap();
    for (use_site, expected) in [
        ("_ = lib.native.value(7)", "does not export `native`"),
        ("_ = lib.native", "does not export `native`"),
        ("_ = native.value(7)", "unknown name `native`"),
        ("_ = lib.native_value(7)", "does not export `native_value`"),
    ] {
        rejects(
            &format!(r#"import {{ "library" as lib }};{use_site}"#),
            &path,
            expected,
        );
    }
    accepts(r#"import { "library" as lib };_ = lib.value(7)"#, &path);
}

#[test]
fn extern_paths_and_symbols_must_be_literals_not_computed_strings() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("native.c"), "").unwrap();
    let prefix =
        "str path = \"native.c\"\nstr symbol = \"native_value\"\nfn get() str { return path }\n";
    for expression in ["path", "get()", "42", "true", "(\"native.c\")"] {
        rejects(
            &format!(
                "{prefix}extern {expression} as native {{ fn value() int = \"native_value\" }}"
            ),
            &path,
            "expected external implementation path",
        );
    }
    for expression in ["symbol", "get()", "42", "true", "(\"native_value\")"] {
        rejects(
            &format!("{prefix}extern \"native.c\" as native {{ fn value() int = {expression} }}"),
            &path,
            "expected external symbol string",
        );
    }
    rejects(
        r#"extern "native" <> ".c" as native { fn value() int = "native_value" }"#,
        &path,
        "expected `as` in extern declaration",
    );
    rejects(
        r#"extern "native.c" as native { fn value() int = "native_" <> "value" }"#,
        &path,
        "extern blocks may only contain functions",
    );
    rejects(
        r#"extern "absent.c" as native { fn value() int = "native_value" }"#,
        &path,
        "cannot open external implementation",
    );
    accepts(
        r#"extern "native.c" as native { fn value() int = "native_value" };_ = native.value()"#,
        &path,
    );
}

#[test]
fn extern_calls_obey_arity_argument_result_and_callback_types() {
    let directory = ncc::temp::Directory::new().unwrap();
    let path = directory.path().join("main.nc");
    fs::write(directory.path().join("native.c"), "").unwrap();
    let declarations = r#"
type Count = int
extern "native.c" as native {
    fn zero() int = "zero"
    fn pair(int n, str text) int = "pair"
    fn nominal(Count value) Count = "nominal"
    fn array(int[] values) int[] = "array"
    fn callback((fn(int) int) function) int = "callback"
    fn optional() int? = "optional"
    fn fallible() int! = "fallible"
    fn touch() = "touch"
}
"#;
    for call in [
        "_ = native.zero(1)",
        "_ = native.pair()",
        "_ = native.pair(1)",
        "_ = native.pair(1, \"text\", 2)",
    ] {
        rejects(
            &format!("{declarations}{call}"),
            &path,
            "incorrect number of arguments",
        );
    }
    for (call, expected) in [
        (
            "_ = native.pair(true, \"text\")",
            "expected `int`, found `bool`",
        ),
        ("_ = native.pair(1, 2)", "expected `str`, found `int`"),
        (
            "int n = 1;_ = native.nominal(n)",
            "expected `Count`, found `int`",
        ),
        ("_ = native.array([true])", "expected `int`, found `bool`"),
        (
            "_ = native.callback(fn(str text) int { return 1 })",
            "expected `(fn(int) int)`, found `(fn(str) int)`",
        ),
        (
            "_ = native.callback(fn(int n) bool { return true })",
            "expected `(fn(int) int)`, found `(fn(int) bool)`",
        ),
        ("bool value = native.zero()", "expected `bool`, found `int`"),
        (
            "int value = native.optional()",
            "expected `int`, found `int?`",
        ),
        (
            "int value = native.fallible()",
            "expected `int`, found `int!`",
        ),
        ("int value = native.touch()", "expected `int`, found `void`"),
        ("_ = native.missing()", "does not export `missing`"),
    ] {
        rejects(&format!("{declarations}{call}"), &path, expected);
    }
    accepts(
        &format!(
            "{declarations}\n_ = native.zero()\n_ = native.pair(1, \"text\")\n_ = native.nominal(@as(Count, 1))\n_ = native.array([1, 2])\n_ = native.callback(fn(int n) int {{ return n }})\nint? optional = native.optional()\nint! fallible = native.fallible()\nnative.touch()"
        ),
        &path,
    );
}
