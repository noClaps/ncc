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
fn byte_to_float_casts_are_rejected_at_the_cast_in_both_modes() {
    for (declarations, cast) in [
        ("byte value = 97", "@as(float, value)"),
        ("mut byte value = 97", "@as(float, value)"),
        ("fn value() byte { return 97 }", "@as(float, value())"),
        ("byte[] values = [97]", "@as(float, values[0])"),
        ("", "@as(float, @as(byte, 97))"),
        (
            "type Octet = byte;Octet value = 97",
            "@as(float, @as(byte, value))",
        ),
        (
            "type Octet = byte;fn value() Octet { return 97 }",
            "@as(float, @as(byte, value()))",
        ),
        (
            "type Octet = byte;type Wrapped = Octet;Wrapped value = 97",
            "@as(float, @as(byte, @as(Octet, value)))",
        ),
        ("fn convert(byte value) float {", "@as(float, value)"),
    ] {
        let suffix = if declarations.starts_with("fn convert") {
            " }"
        } else {
            ""
        };
        let separator = if suffix.is_empty() {
            "\n_ = "
        } else {
            "\nreturn "
        };
        let source = format!("{declarations}{separator}{cast}{suffix}");
        let path = Path::new("negative_builtins.nc");
        for release in [false, true] {
            let error = ncc::compile_source_with_options(&source, path, release)
                .expect_err(&format!("release={release}: {source}"));
            let diagnostic = &error.0[0];
            assert!(
                diagnostic.message.contains("cannot cast `byte` to `float`"),
                "release={release}: {source}\n{error}"
            );
            assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
            assert_eq!(&source[diagnostic.span.clone()], cast, "{error}");
            assert!(
                error
                    .render(&source, path)
                    .contains("negative_builtins.nc:2:"),
                "{error}"
            );
        }
    }
}

#[test]
fn typed_numeric_values_cannot_be_cast_to_byte_in_either_mode() {
    for (ty, literal) in [("int", "1"), ("uint", "1u"), ("float", "1.0")] {
        for (declarations, value, suffix) in [
            (format!("{ty} value = {literal}\n_ = "), "value", ""),
            (format!("mut {ty} value = {literal}\n_ = "), "value", ""),
            (
                format!("fn get() {ty} {{ return {literal} }}\n_ = "),
                "get()",
                "",
            ),
            (
                format!("fn convert({ty} value) byte {{\nreturn "),
                "value",
                "\n}",
            ),
            (
                format!("{ty}[] values = [{literal}]\n_ = "),
                "values[0]",
                "",
            ),
            (
                format!("type Number = {ty};Number value = {literal}\n_ = "),
                "@as(TYPE, value)",
                "",
            ),
            ("\n_ = ".into(), "@as(TYPE, LITERAL)", ""),
            (format!("{ty} value = {literal}\n_ = "), "value + value", ""),
        ] {
            let value = value.replace("TYPE", ty).replace("LITERAL", literal);
            let cast = format!("@as(byte, {value})");
            let source = format!("{declarations}{cast}{suffix}");
            let path = Path::new("negative_builtins.nc");
            for release in [false, true] {
                let error = ncc::compile_source_with_options(&source, path, release)
                    .expect_err(&format!("release={release}: {source}"));
                let diagnostic = &error.0[0];
                assert!(
                    diagnostic
                        .message
                        .contains(&format!("cannot cast `{ty}` to `byte`")),
                    "release={release}: {source}\n{error}"
                );
                assert_eq!(diagnostic.path.as_deref(), Some(path), "{error}");
                assert_eq!(&source[diagnostic.span.clone()], cast, "{error}");
            }
        }
    }
}

#[test]
fn numeric_table_controls_literals_identity_and_nominal_casts_still_compile() {
    let source = r"
byte octet = 1
int signed = 1
uint unsigned = 1u
float real = 1.0
fn byte_identity(byte value) byte { return @as(byte, value) }
byte identity = @as(byte, octet)
byte call_identity = byte_identity(octet)
byte literal = @as(byte, 1)
byte parenthesized = @as(byte, (1))
byte hexadecimal = @as(byte, 0xff)
byte binary = @as(byte, 0b11111111)
byte octal = @as(byte, 0o377)
byte[] contextual = [0, 1, 255]
type Octet = byte
type Wrapped = Octet
type Number = int
type Unsigned = uint
type Real = float
Octet nominal = @as(Octet, octet)
Octet nominal_literal = @as(Octet, 1)
Wrapped wrapped = @as(Wrapped, nominal)
Octet immediate = @as(Octet, wrapped)
byte unwrapped = @as(byte, immediate)
Number number = @as(Number, signed)
int unwrapped_number = @as(int, number)
Unsigned count = @as(Unsigned, unsigned)
uint unwrapped_count = @as(uint, count)
Real fraction = @as(Real, real)
float unwrapped_fraction = @as(float, fraction)
_ = @as(int, signed)
_ = @as(uint, unsigned)
_ = @as(float, real)
_ = @as(int, octet)
_ = @as(uint, octet)
_ = @as(char, octet)
_ = @as(str, octet)
_ = @as(uint, signed)
_ = @as(float, signed)
_ = @as(byte[], signed)
_ = @as(str, signed)
_ = @as(int, unsigned)
_ = @as(float, unsigned)
_ = @as(byte[], unsigned)
_ = @as(str, unsigned)
_ = @as(int, real)
_ = @as(uint, real)
_ = @as(byte[], real)
_ = @as(str, real)
_ = @as(int, true)
_ = @as(uint, false)
_ = @as(str, true)
@println(identity, call_identity, literal, parenthesized, hexadecimal, binary, octal,
    contextual, @as(byte, nominal_literal), unwrapped, unwrapped_number, unwrapped_count, unwrapped_fraction)
";
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("builtins.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}"));
    }
}

#[test]
fn nominal_bytes_cannot_skip_their_immediate_underlying_type() {
    for declarations in [
        "type Octet = byte;Octet value = 97",
        "type Octet = byte;fn get() Octet { return 97 };Octet value = get()",
        "type Octet = byte;type Wrapped = Octet;Wrapped value = 97",
    ] {
        rejects(&format!("{declarations};_ = @as(float, value)"), "cast");
    }
}

#[test]
fn explicit_byte_integer_float_intermediaries_and_nominal_casts_still_compile() {
    let source = r"
byte value = 97
mut byte mutable = 98
byte[] values = [99]
fn get() byte { return 100 }
fn convert(byte argument) float { return @as(float, @as(int, argument)) }
type Octet = byte
type Wrapped = Octet
type Real = float
Octet octet = 101
Wrapped wrapped = 102
byte plain = @as(byte, octet)
Octet rewrapped = @as(Octet, plain)
Wrapped outer = @as(Wrapped, rewrapped)
Octet inner = @as(Octet, outer)
float from_binding = @as(float, @as(int, value))
float from_mutable = @as(float, @as(int, mutable))
float from_element = @as(float, @as(int, values[0]))
float from_call = @as(float, @as(int, get()))
float from_parameter = convert(value)
float from_nominal = @as(float, @as(int, @as(byte, octet)))
float from_wrapped = @as(float, @as(int, @as(byte, @as(Octet, wrapped))))
float from_unsigned = @as(float, @as(uint, value))
Real real = @as(Real, @as(float, @as(int, value)))
float unwrapped_real = @as(float, real)
byte contextual = 103
byte literal = @as(byte, 104)
float integer_literal = @as(float, 105)
float float_literal = 106.0
char character = @as(char, value)
str text = @as(str, value)
@println(from_binding, from_mutable, from_element, from_call, from_parameter,
    from_nominal, from_wrapped, from_unsigned, unwrapped_real,
    contextual, literal, integer_literal, float_literal, character, text, @as(byte, inner))
";
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("builtins.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}"));
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
