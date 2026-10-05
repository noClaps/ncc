use std::path::Path;

fn rejects(source: &str, expected: &str) {
    let path = Path::new("negative_builtins.nc");
    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, path, release)
            .expect_err(&format!("release={release}: {source}"));
        assert!(
            error.to_string().contains(expected),
            "release={release}: {source}\nexpected {expected:?}: {error}"
        );
    }
}

#[test]
fn process_state_builtins_reject_arguments_and_incompatible_result_bindings() {
    for (name, ty, invalid_ty) in [
        ("args", "str[]", "str"),
        ("env", "[str]str", "str[]"),
        ("target", "(str, str)", "str"),
    ] {
        for args in ["1", "\"value\"", "true, false", "[]"] {
            rejects(
                &format!("_ = @{name}({args})"),
                &format!("@{name} expects no arguments"),
            );
        }
        rejects(
            &format!("{invalid_ty} value = @{name}()"),
            &format!("expected `{invalid_ty}`, found `{ty}`"),
        );
    }
}

#[test]
fn unknown_builtins_are_not_ordinary_functions() {
    for name in ["missing", "exit", "len", "Print"] {
        rejects(
            &format!("_ = @{name}()"),
            &format!("unknown name `@{name}`"),
        );
    }
    rejects(
        "fn custom() int { return 1 };_ = @custom()",
        "unknown name `@custom`",
    );
}

#[test]
fn cast_and_embed_calls_require_their_declared_syntax_and_arity() {
    for source in [
        "_ = @as()",
        "_ = @as(int)",
        "_ = @as(int,)",
        "_ = @as(int, 1, 2)",
        "_ = @as(1, 2)",
        "_ = @embed()",
        "_ = @embed(\"missing\", \"extra\")",
    ] {
        rejects(source, "expected");
    }
    rejects("_ = @as(Missing, 1)", "unknown type");
}

#[test]
fn casts_reject_unsupported_source_and_destination_pairs() {
    for (declaration, destination) in [
        ("bool value = true", "float"),
        ("char value = 'a'", "int"),
        ("int value = 1", "bool"),
        ("uint value = 1u", "char"),
        ("float value = 1.0", "bool"),
        ("str value = \"1\"", "int"),
        ("int[] value = [1]", "int[1]"),
        ("int[1] value = [1]", "bool[]"),
        ("[str]int value = [\"a\": 1]", "int[]"),
        ("(int, int) value = (1, 2)", "int[]"),
    ] {
        rejects(
            &format!("{declaration};_ = @as({destination}, value)"),
            "cast",
        );
    }
}

#[test]
fn output_builtins_and_casts_reject_nonconvertible_constituents() {
    let declarations = "fn action() {};fn result() int { return 1 }\n";
    for value in [
        "action",
        "[action]",
        "(1, action)",
        "[\"a\": action]",
        "action()",
    ] {
        for name in ["print", "println", "eprint", "eprintln"] {
            rejects(
                &format!("{declarations}@{name}({value})"),
                "string conversion",
            );
        }
        rejects(
            &format!("{declarations}_ = @as(str, {value})"),
            "string conversion",
        );
    }
    for declaration in [
        "(fn() void)[] values = []",
        "[str](fn() void) values = []",
        "struct Record { (fn() void) callback };Record[] values = []",
        "enum Choice { Number(int) Callback((fn() void)) };Choice values = Choice.Number(1)",
        "fut int values = async result()",
    ] {
        for name in ["print", "println", "eprint", "eprintln"] {
            rejects(
                &format!("{declarations}{declaration};@{name}(values)"),
                "string conversion",
            );
        }
        rejects(
            &format!("{declarations}{declaration};_ = @as(str, values)"),
            "string conversion",
        );
    }
}

#[test]
fn void_output_results_cannot_initialize_values() {
    for name in ["print", "println", "eprint", "eprintln"] {
        rejects(
            &format!("int value = @{name}(\"message\")"),
            "expected `int`, found `void`",
        );
    }
}

#[test]
fn specified_builtin_signatures_and_convertible_arguments_still_compile() {
    let source = r#"
str[] arguments = @args()
[str]str environment = @env()
(str, str) target = @target()
float real = @as(float, 1)
int[] dynamic = @as(int[], [1, 2])
@print()
@println("value", real, dynamic, true)
@eprint()
@eprintln(target)
"#;
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("builtins.nc"), release).unwrap();
    }
}
