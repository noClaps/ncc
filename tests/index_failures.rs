use std::{fmt::Write as _, fs, path::Path, process::Command};

#[test]
fn runtime_array_and_string_reads_and_writes_check_signed_and_unsigned_indices() {
    let mut cases = Vec::new();
    for (kind, value, replacement) in [
        ("int[]", "[7, 8]", "9"),
        ("int[2]", "[7, 8]", "9"),
        ("str", "\"e\\u{301}🍪\"", "'z'"),
    ] {
        for (index_kind, index) in [
            ("int", "-1"),
            ("int", "2"),
            ("int", "9223372036854775807"),
            ("uint", "2u"),
            ("uint", "18446744073709551615u"),
        ] {
            for write in [false, true] {
                let operation = if write {
                    "values[index()] = rhs()"
                } else {
                    "_ = values[index()]"
                };
                let element = if kind == "str" { "char" } else { "int" };
                let source = format!(
                    "mut {kind} values = {value}\n\
                     {index_kind}[] indices = [{index}]\n\
                     fn index() {index_kind} {{ @eprintln(\"index\");return indices[@args().len - 2u] }}\n\
                     fn rhs() {element} {{ @eprintln(\"rhs\");return {replacement} }}\n\
                     @eprintln(\"before\")\n{operation}\n@eprintln(\"after\")\n"
                );
                cases.push((
                    source,
                    String::from(if write {
                        "before\nrhs\nindex\n"
                    } else {
                        "before\nindex\n"
                    }),
                    "out of bounds",
                ));
            }
        }
    }
    assert_eq!(cases.len(), 30);
    failure_matrix(&cases);
}

#[test]
fn empty_last_indices_and_excessive_last_offsets_are_bounds_failures() {
    let mut cases = Vec::new();
    for (kind, value, replacement) in [
        ("int[]", "[]", "9"),
        ("int[0]", "[]", "9"),
        ("str", "\"\"", "'z'"),
        ("int[]", "[7, 8]", "9"),
        ("int[2]", "[7, 8]", "9"),
        ("str", "\"e\\u{301}🍪\"", "'z'"),
    ] {
        let empty = value == "[]" || value == "\"\"";
        for index in if empty {
            vec!["$", "$-0u", "$-1u", "$-offset()"]
        } else {
            vec!["$-2u", "$-18446744073709551615u", "$-offset()", "$-1u-1u"]
        } {
            for write in [false, true] {
                let element = if kind == "str" { "char" } else { "int" };
                let operation = if write {
                    format!("values[{index}] = rhs()")
                } else {
                    format!("_ = values[{index}]")
                };
                // Select the container at runtime so even fixed empty inputs reach runtime.
                let source = format!(
                    "{kind}[] inputs = [{value}]\n\
                     mut {kind} values = inputs[@args().len - 2u]\n\
                     uint[] offsets = [2u]\n\
                     fn offset() uint {{ @eprintln(\"offset\");return offsets[@args().len - 2u] }}\n\
                     fn rhs() {element} {{ @eprintln(\"rhs\");return {replacement} }}\n\
                     @eprintln(\"before\")\n{operation}\n@eprintln(\"after\")\n"
                );
                let mut prefix = String::from("before\n");
                if write {
                    prefix.push_str("rhs\n");
                }
                // Empty `$` fails before its subtraction operand is evaluated.
                if !empty && index.contains("offset()") {
                    prefix.push_str("offset\n");
                }
                cases.push((source, prefix, "out of bounds"));
            }
        }
    }
    assert_eq!(cases.len(), 48);
    failure_matrix(&cases);
}

#[test]
fn absent_map_reads_panic_including_optional_values_and_nested_write_paths() {
    let mut cases = Vec::new();
    for (kind, value, operation) in [
        ("[str]int", "[\"present\": 7]", "_ = values[key()]"),
        ("[str]int", "[]", "_ = values[key()]"),
        ("[str]int?", "[\"present\": none]", "_ = values[key()]"),
        (
            "[str]int[]",
            "[\"present\": [7]]",
            "values[key()][0] = rhs()",
        ),
    ] {
        let source = format!(
            "mut {kind} values = {value}\n\
             str[] keys = [\"missing\"]\n\
             fn key() str {{ @eprintln(\"key\");return keys[@args().len - 2u] }}\n\
             fn rhs() int {{ @eprintln(\"rhs\");return 9 }}\n\
             @eprintln(\"before\")\n{operation}\n@eprintln(\"after\")\n"
        );
        cases.push((
            source,
            String::from(if operation.contains("rhs()") {
                "before\nrhs\nkey\n"
            } else {
                "before\nkey\n"
            }),
            "key",
        ));
    }
    assert_eq!(cases.len(), 4);
    failure_matrix(&cases);
}

#[test]
fn indexing_panics_cannot_be_recovered_with_catch() {
    let mut cases = Vec::new();
    for (declarations, operation, detail) in [
        (
            "int[] values = [7]",
            "_ = values[@args().len - 1u]",
            "out of bounds",
        ),
        (
            "str[] inputs = [\"\"]",
            "str value = inputs[@args().len - 2u];_ = value[$]",
            "out of bounds",
        ),
        ("[str]int values = []", "_ = values[@args()[0]]", "key"),
    ] {
        let source = format!(
            "{declarations}\n\
             fn failing() int! {{ @eprintln(\"before\");{operation};return 7 }}\n\
             _ = failing() catch error {{ @eprintln(\"caught\");break 0 }}\n\
             @eprintln(\"after\")\n"
        );
        cases.push((source, String::from("before\n"), detail));
    }
    assert_eq!(cases.len(), 3);
    failure_matrix(&cases);
}

#[test]
fn valid_runtime_last_indices_and_absent_key_insertions_still_succeed() {
    let source = r#"
test "valid boundary controls" {
    uint zero = @args().len - 1u
    mut int[] dynamic = [7, 8]
    mut int[2] fixed = [7, 8]
    mut str text = "e\u{301}🍪"
    assert dynamic[$-zero] == 8 and fixed[$-1u-zero] == 7
    assert text[$-zero] == '🍪' and text[$-1u-zero] == 'e\u{301}'
    dynamic[$-zero] = 9
    fixed[$-1u-zero] = 6
    text[$-zero] = 'z'
    assert dynamic == [7, 9] and fixed == [6, 8] and text == "e\u{301}z"
    mut [str]int? values = ["present": none]
    str key = @args()[0]
    values[key] = 9
    assert values.len == 2 and (values[key] else 0) == 9
    assert (values["present"] else 0) == 0
    @println("checked")
}
"#;
    with_input(source, |input| {
        for release in [false, true] {
            let output = execute(input, "test", release);
            assert!(output.status.success(), "release={release}: {output:?}");
            assert_eq!(output.stdout, b"checked\n");
            assert!(output.stderr.is_empty(), "{output:?}");
        }
    });
}

fn failure_matrix(cases: &[(String, String, &str)]) {
    // The selector adds one argument. Case-local argument-count indices subtract
    // that extra argument without replacing their runtime-unknown inputs.
    let mut source = String::from("if @args()[1] {\n");
    for (case, (case_source, _, _)) in cases.iter().enumerate() {
        writeln!(source, "\"{case}\" -> {{\n{case_source}\n}}").unwrap();
    }
    source.push_str("_ -> {}\n}\n");
    with_input(&source, |input| {
        for release in [false, true] {
            // Preserve unguarded front-end/optimizer coverage as well as the
            // runtime-selected native cases; selection must not hide diagnostics.
            for (case_source, _, _) in cases {
                ncc::compile_source_with_options(case_source, input, release)
                    .unwrap_or_else(|error| panic!("release={release}: {error}\n{case_source}"));
            }
            let executable = input.with_extension(if release { "release" } else { "debug" });
            let build = Command::new(env!("CARGO_BIN_EXE_ncc"))
                .arg("build")
                .arg(if release { "-r" } else { "-d" })
                .arg(input)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                build.status.success(),
                "release={release}: {build:?}\n{source}"
            );
            for (case, (case_source, prefix, detail)) in cases.iter().enumerate() {
                // A panic ends only this child, never the remaining matrix cases.
                let output = Command::new(&executable)
                    .arg(case.to_string())
                    .output()
                    .unwrap();
                assert_failure(&output, prefix, detail, release, case_source);
            }
        }
    });
}

fn assert_failure(
    output: &std::process::Output,
    prefix: &str,
    detail: &str,
    release: bool,
    source: &str,
) {
    assert!(
        !output.status.success(),
        "release={release}: {output:?}\n{source}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with(&format!("{prefix}panic:")),
        "release={release}: {stderr}\n{source}"
    );
    assert!(stderr.contains(detail), "{stderr}");
    assert!(
        !stderr.contains("overflow") && !stderr.contains("after") && !stderr.contains("caught"),
        "{stderr}"
    );
}

fn with_input(source: &str, action: impl FnOnce(&Path)) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("indices.nc");
    fs::write(&input, source).unwrap();
    action(&input);
}

fn execute(input: &Path, mode: &str, release: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
    command.arg(mode);
    if release {
        command.arg("--release");
    }
    command.arg(input).output().unwrap()
}
