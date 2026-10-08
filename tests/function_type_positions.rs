use std::{fs, path::Path, process::Command};

const SIGNATURE: &str = "(fn(int) int)";
const BARE_SIGNATURE: &str = "fn(int) int";

#[test]
fn signatures_require_parentheses_in_remaining_type_positions() {
    for (position, template) in [
        ("module binding", "CALLBACK callback = increment"),
        (
            "local binding",
            "fn local() { CALLBACK callback = increment }",
        ),
        ("mutable binding", "mut CALLBACK callback = increment"),
        (
            "multiple bindings",
            "CALLBACK first, CALLBACK second = (increment, increment)",
        ),
        ("alias", "type Callback = CALLBACK"),
        ("enum payload", "enum Choice { Value(CALLBACK) }"),
        ("tuple element", "(int, CALLBACK) pair = (1, increment)"),
        (
            "dynamic array element",
            "CALLBACK[] callbacks = [increment]",
        ),
        ("fixed array element", "CALLBACK[1] callbacks = [increment]"),
        (
            "map value",
            "[str]CALLBACK callbacks = [\"one\": increment]",
        ),
        ("map key syntax", "type Lookup = [CALLBACK]int"),
        ("optional payload", "CALLBACK? callback = increment"),
        ("error payload", "CALLBACK! callback = increment"),
        ("future payload", "fut CALLBACK callback = async make()"),
        ("mutex payload", "mutex CALLBACK callback = increment"),
        (
            "generic function argument",
            "CALLBACK callback = identity<CALLBACK>(increment)",
        ),
        (
            "generic struct argument",
            "Holder<CALLBACK> holder = Holder<CALLBACK>{.value = increment}",
        ),
        (
            "generic enum argument",
            "Choice<CALLBACK> choice = Choice.Value(increment)",
        ),
        (
            "conversion target",
            "CALLBACK callback = @as(CALLBACK, increment)",
        ),
        ("signature parameter", "type Consumer = (fn(CALLBACK) int)"),
        ("signature return", "type Factory = (fn() CALLBACK)"),
        (
            "anonymous parameter",
            "fn apply = fn(CALLBACK callback) int { return callback(1) }",
        ),
        (
            "anonymous return",
            "fn factory = fn() CALLBACK { return increment }",
        ),
        (
            "extern parameter syntax",
            "extern \"callback.c\" as raw { fn apply(CALLBACK callback) int = \"apply\" }",
        ),
        (
            "extern return syntax",
            "extern \"callback.c\" as raw { fn make() CALLBACK = \"make\" }",
        ),
    ] {
        let valid = template.replace("CALLBACK", SIGNATURE);
        ncc::parser::parse(ncc::lexer::lex(&valid).unwrap())
            .unwrap_or_else(|error| panic!("{position}: {valid}\n{error}"));
        // Remove one occurrence at a time so a different, earlier malformed type
        // cannot mask a missing parentheses check in the position under test.
        for (offset, _) in valid.match_indices(SIGNATURE) {
            let mut invalid = valid.clone();
            invalid.replace_range(offset..offset + SIGNATURE.len(), BARE_SIGNATURE);
            let error = ncc::parser::parse(ncc::lexer::lex(&invalid).unwrap()).unwrap_err();
            assert!(
                error.to_string().contains("expected"),
                "{position}: {error}"
            );
            for release in [false, true] {
                let compiled = ncc::compile_source_with_options(
                    &invalid,
                    Path::new("function_type_positions.nc"),
                    release,
                )
                .unwrap_err();
                assert_eq!(
                    compiled.to_string(),
                    error.to_string(),
                    "{position}, release={release}"
                );
            }
        }
    }
}

const DECLARATIONS: &str = r"
fn increment(int value) int { return value + 1 }
fn decrement(int value) int { return value - 1 }
fn make() (fn(int) int) { return increment }
fn identity<type T>(T value) T { return value }
struct Holder<type T> { T value }
enum Choice<type T> { Value(T) }
enum DirectChoice { Value((fn(int) int)) }
type Callback = (fn(int) int)
type Consumer = (fn((fn(int) int), int) int)
type Factory = (fn() (fn(int) int))
(fn(int) int) global = increment
";

#[test]
fn parenthesized_declarations_containers_generics_and_conversions_execute() {
    execute(
        r#"
int seed = @as(int, @args().len)
(fn(int) int) local = global
mut (fn(int) int) mutable = local
mutable = decrement
(fn(int) int) first, (fn(int) int) second = (increment, mutable)
assert first(seed) == seed + 1 and second(seed) == seed - 1
Callback nominal = @as(Callback, local)
(fn(int) int) unwrapped = @as((fn(int) int), nominal)
assert unwrapped(seed) == seed + 1
(int, (fn(int) int)) pair = (seed, local)
assert pair[1](pair[0]) == seed + 1
(fn(int) int)[] dynamic = [local, mutable]
(fn(int) int)[2] fixed = [local, mutable]
[str](fn(int) int) table = ["up": local, "down": mutable]
assert dynamic[0](seed) == seed + 1 and fixed[1](seed) == seed - 1
assert table["up"](seed) == seed + 1 and table["down"](seed) == seed - 1
(fn(int) int)? optional = local
(fn(int) int) present = optional else decrement
assert present(seed) == seed + 1
(fn(int) int)! result = local
(fn(int) int) successful = result catch err { break decrement }
assert successful(seed) == seed + 1
DirectChoice direct = DirectChoice.Value(local)
if direct { DirectChoice.Value(callback) -> { assert callback(seed) == seed + 1 } }
(fn(int) int) forwarded = identity<(fn(int) int)>(local)
Holder<(fn(int) int)> holder = Holder<(fn(int) int)>{.value = forwarded}
assert holder.value(seed) == seed + 1
Choice<(fn(int) int)> choice = Choice.Value(local)
if choice { Choice.Value(callback) -> { assert callback(seed) == seed + 1 } }
(fn(int) int) converted = @as((fn(int) int), local)
assert converted(seed) == seed + 1
Consumer consumer = @as(Consumer, fn((fn(int) int) callback, int value) int { return callback(value) })
Factory factory = @as(Factory, fn() (fn(int) int) { return increment })
(fn((fn(int) int), int) int) consume = @as((fn((fn(int) int), int) int), consumer)
(fn() (fn(int) int)) produce = @as((fn() (fn(int) int)), factory)
(fn(int) int) produced = produce()
assert consume(produced, seed) == seed + 1
@println("positions")
"#,
    );
}

#[test]
fn parenthesized_future_and_mutex_payloads_execute() {
    execute(
        r#"
int seed = @as(int, @args().len)
fut (fn(int) int) pending = async make()
(fn(int) int) ready = await pending
assert ready(seed) == seed + 1
mutex (fn(int) int) protected = ready
lock protected {
    assert protected(seed) == seed + 1
    protected = decrement
    assert protected(seed) == seed - 1
}
lock protected { assert protected(seed) == seed - 1 }
@println("positions")
"#,
    );
}

fn execute(body: &str) {
    let temp = ncc::temp::Directory::new().unwrap();
    let input = temp.path().join("main.nc");
    fs::write(
        &input,
        format!("{DECLARATIONS}\ntest \"positions\" {{\n{body}\n}}\n"),
    )
    .unwrap();
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("test");
        if release {
            command.arg("-r");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"positions\n", "release={release}");
        assert_eq!(output.stderr, b"", "release={release}");
    }
}
