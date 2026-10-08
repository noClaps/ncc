use std::{fs, path::Path, process::Command};

struct ExpectedLocation<'a> {
    path: &'a Path,
    source: &'a str,
    expression: &'a str,
    line: usize,
    column: usize,
    message: &'a str,
}

fn rejects_in_both_modes(source: &str, root: &Path, expected: &ExpectedLocation<'_>) {
    fs::write(root, source).unwrap();
    let start = expected.source.find(expected.expression).unwrap();
    let excerpt = expected.source.lines().nth(expected.line - 1).unwrap();
    let mut debug_diagnostics = None;
    let mut debug_rendered = None;

    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, root, release).unwrap_err();
        assert_eq!(error.0.len(), 1, "release={release}: {error}");
        let diagnostic = &error.0[0];
        assert_eq!(diagnostic.path.as_deref(), Some(expected.path), "{error}");
        assert_eq!(
            diagnostic.span,
            start..start + expected.expression.len(),
            "release={release}: {error}"
        );
        assert!(diagnostic.message.contains(expected.message), "{error}");

        let rendered = error.render(source, root);
        assert!(
            rendered.contains(&format!(
                "{}:{}:{}: error: {}\n",
                expected.path.display(),
                expected.line,
                expected.column,
                diagnostic.message
            )),
            "release={release}: {rendered}"
        );
        assert!(
            rendered.contains(&format!(
                "{:>2} | {excerpt}\n  | {}^\n",
                expected.line,
                " ".repeat(expected.column - 1)
            )),
            "release={release}: {rendered}"
        );
        if release {
            assert_eq!(Some(&error.0), debug_diagnostics.as_ref());
            assert_eq!(Some(&rendered), debug_rendered.as_ref());
        } else {
            debug_diagnostics = Some(error.0.clone());
            debug_rendered = Some(rendered.clone());
        }

        // Capture the CLI boundary as well: preceding output probes must not run
        // when semantic checking rejects the program, even in release mode.
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("run")
            .arg(root)
            .arg(if release { "--release" } else { "--debug" })
            .current_dir(root.parent().unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("ncc: {rendered}\n"),
            "release={release}"
        );
    }
}

#[test]
fn transitive_imported_generic_failure_keeps_the_definition_location() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let nested = directory.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let helper = nested.join("helper.nc");
    let helper_source = "// original generic definition\n\npub fn double<type T>(T value) T {\n  @println(\"helper effect must not run\")\n  return value + value\n}\n";
    fs::write(&helper, helper_source).unwrap();
    fs::write(
        nested.join("library.nc"),
        "import { \"helper\" as helper }\npub fn forward<type T>(T value) T {\n  return helper.double<T>(value)\n}\n",
    )
    .unwrap();
    let source = "import { \"nested/library\" as lib }\n@println(\"root effect must not run\")\n_ = lib.forward<str>(\"text\")\n";
    rejects_in_both_modes(
        source,
        &root,
        &ExpectedLocation {
            path: &helper,
            source: helper_source,
            expression: "value + value",
            line: 5,
            column: 10,
            message: "arithmetic requires numeric operands",
        },
    );
}

#[test]
fn nested_closure_failures_keep_root_and_imported_expression_locations() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("closures.nc");
    let body = "@println(\"outer effect must not run\")\nfn outer() int {\n  int captured = 7\n  fn middle = fn() int {\n    fn inner = fn() int {\n      @println(\"closure effect must not run\")\n      return captured + missing\n    }\n    return inner()\n  }\n  return middle()\n}\n_ = outer()\n";
    fs::write(&imported, body).unwrap();
    for from_import in [false, true] {
        let source = if from_import {
            "import { \"closures\" as lib }\n@println(\"root effect must not run\")\n"
        } else {
            body
        };
        rejects_in_both_modes(
            source,
            &root,
            &ExpectedLocation {
                path: if from_import { &imported } else { &root },
                source: body,
                expression: "missing",
                line: 7,
                column: 25,
                message: "unknown name `missing`",
            },
        );
    }
}

#[test]
fn nested_closure_in_imported_generic_keeps_its_original_operand_location() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("generic_closures.nc");
    let body = "// closures are checked after specialization\npub fn outer<type T>(T captured) T {\n  fn middle = fn() T {\n    fn inner = fn() T {\n      @println(\"closure effect must not run\")\n      return captured + captured\n    }\n    return inner()\n  }\n  return middle()\n}\n";
    fs::write(&imported, body).unwrap();
    let source = "import { \"generic_closures\" as lib }\n@println(\"root effect must not run\")\n_ = lib.outer<str>(\"text\")\n";
    rejects_in_both_modes(
        source,
        &root,
        &ExpectedLocation {
            path: &imported,
            source: body,
            expression: "captured + captured",
            line: 6,
            column: 14,
            message: "arithmetic requires numeric operands",
        },
    );
}

#[test]
fn escaped_interpolation_failures_keep_the_original_literal_location() {
    let directory = ncc::temp::Directory::new().unwrap();
    let root = directory.path().join("main.nc");
    let imported = directory.path().join("interpolation.nc");
    for literal in [
        r#""🍪\u{41}\n\t\"quoted\" {missing}""#,
        r#""\{not_an_expression} \\ {1 + true}""#,
    ] {
        let body = format!(
            "// original escaped bytes\n@println(\"effect must not run\")\nfn describe() str {{\n  return {literal}\n}}\n_ = describe()\n"
        );
        fs::write(&imported, &body).unwrap();
        for from_import in [false, true] {
            let source = if from_import {
                "import { \"interpolation\" as lib }\n@println(\"root effect must not run\")\n"
            } else {
                &body
            };
            rejects_in_both_modes(
                source,
                &root,
                &ExpectedLocation {
                    path: if from_import { &imported } else { &root },
                    source: &body,
                    expression: literal,
                    line: 4,
                    column: 10,
                    message: if literal.contains("missing") {
                        "unknown name `missing`"
                    } else {
                        "expected `int`, found `bool`"
                    },
                },
            );
        }
    }
}
