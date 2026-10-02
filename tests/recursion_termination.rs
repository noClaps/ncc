use std::path::Path;

fn compile(source: &str, release: bool) -> Result<String, ncc::diagnostic::Diagnostics> {
    ncc::compile_source_with_options(source, Path::new("recursion.nc"), release)
}

fn assert_runtime_output(source: &str, expected: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("recursion.nc");
    std::fs::write(&input, source).unwrap();
    for release in [false, true] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ncc"));
        command.arg("run");
        if release {
            command.arg("--release");
        }
        let output = command.arg(&input).output().unwrap();
        assert!(
            output.status.success(),
            "release={release}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes(), "release={release}");
    }
}

#[test]
fn covering_integer_bases_certify_fibonacci_and_depth() {
    let source = r"
fn fib(int n) int {
    if n { 0, 1 -> { return n } _ -> { return fib(n - 1) + fib(n - 2) } }
}
fn depth(uint n) uint {
    if n { 0u -> { return 0u } _ -> { return 1u + depth(n - 1u) } }
}
@println(fib(10))
@println(depth(20))
";
    let debug = compile(source, false).unwrap();
    let release = compile(source, true).unwrap();
    for name in ["fib", "depth"] {
        assert!(debug.contains(&format!("nc_fn_{name}(")));
        assert!(!release.contains(&format!("nc_fn_{name}(")));
    }
}

#[test]
fn unproven_recursion_is_not_entered_even_before_its_first_recursive_call() {
    let source = r"
fn spin(int n) int {
    int failure = 1 / (n - n)
    return failure + spin(n)
}
@println(spin(1))
";
    // Entering the body would report a reached compile-time division by zero.
    for release in [false, true] {
        let c = compile(source, release).unwrap();
        assert!(c.contains("nc_fn_spin("));
    }
}

#[test]
fn aliases_and_callbacks_cannot_hide_recursive_cycles() {
    for source in [
        r"
fn spin(int n) int {
    int failure = 1 / (n - n)
    (fn(int) int) again = spin
    return failure + again(n)
}
@println(spin(1))
",
        r"
fn invoke((fn(int) int) callback, int n) int {
    int failure = 1 / (n - n)
    return failure + callback(n)
}
fn spin(int n) int { return invoke(spin, n) }
@println(spin(1))
",
    ] {
        for release in [false, true] {
            let c = compile(source, release).unwrap();
            assert!(c.contains("nc_fn_spin("));
        }
    }
}

#[test]
fn closure_entry_checks_its_reachable_recursive_functions() {
    let source = r"
fn spin(int n) int { return spin(n) }
fn closure = fn(int n) int {
    int failure = 1 / (n - n)
    return failure + spin(n)
}
@println(closure(1))
";
    for release in [false, true] {
        assert!(compile(source, release).unwrap().contains("nc_fn_spin("));
    }
}

#[test]
fn certified_recursion_agrees_in_debug_and_release() {
    let source = r"
fn fib(int n) int {
    if n { 0, 1 -> { return n } _ -> { return fib(n - 1) + fib(n - 2) } }
}
fn depth(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return 1 + depth(n - 1) } }
}
@println(fib(10), depth(20))
";
    assert_runtime_output(source, "5520\n");
}

#[test]
fn mutual_recursion_is_conservatively_rejected() {
    let source = r"
fn first(int n) int {
    int failure = 1 / (n - n)
    return failure + second(n)
}
fn second(int n) int { return first(n) }
@println(first(1))
";
    assert!(compile(source, true).unwrap().contains("nc_fn_first("));
}

#[test]
fn missing_bases_and_excessive_decrements_do_not_certify_recursion() {
    for source in [
        r"
fn missing(uint n) uint {
    if n { 1 -> { return 1 } _ -> { return missing(n - 1) } }
}
@println(missing(4))
",
        r"
fn missing(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return missing(n - 2) } }
}
@println(missing(4))
",
    ] {
        assert!(compile(source, true).unwrap().contains("nc_fn_missing("));
    }
}

#[test]
fn signed_rank_requires_a_nonnegative_initial_argument() {
    let source = r"
fn fib(int n) int {
    if n { 0, 1 -> { return n } _ -> { return fib(n - 1) + fib(n - 2) } }
}
@println(fib(-1))
";
    assert!(compile(source, true).unwrap().contains("nc_fn_fib("));
}

#[test]
fn heap_continuations_fold_beyond_the_evaluator_depth_guard() {
    let source = r"
fn depth(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return 1 + depth(n - 1) } }
}
@println(depth(10000))
";
    assert!(!compile(source, true).unwrap().contains("nc_fn_depth("));
}

#[test]
fn acyclic_aliases_returned_callbacks_and_captures_fold() {
    let source = r"
fn increment(int n) int { return n + 1 }
fn apply((fn(int) int) callback, int n) int { return callback(n) }
fn make((fn(int) int) callback) (fn(int) int) {
    return fn(int n) int { return callback(n) }
}
fn compute() int {
    (fn(int) int) alias = increment
    (fn(int) int)[] callbacks = [make(alias)]
    return apply(callbacks[0], 10)
}
@println(compute())
";
    assert!(!compile(source, true).unwrap().contains("nc_fn_compute("));
}

#[test]
fn callable_replacement_through_shared_captures_does_not_hide_a_cycle() {
    let source = r"
fn idle(int n) int { return n }
fn spin(int n) int {
    int failure = 1 / (n - n)
    mut (fn(int) int) callback = idle
    fn replace() { callback = spin }
    replace()
    return failure + callback(n)
}
@println(spin(1))
";
    assert!(compile(source, true).unwrap().contains("nc_fn_spin("));
}

#[test]
fn enum_payload_callbacks_cannot_hide_recursive_cycles() {
    let source = r"
enum Callback { Ready((fn(int) int)) }
fn spin(int n) int {
    int failure = 1 / (n - n)
    Callback holder = Callback.Ready(spin)
    if holder { Callback.Ready(callback) -> { return failure + callback(n) } }
}
@println(spin(1))
";
    assert!(compile(source, true).unwrap().contains("nc_fn_spin("));
}

#[test]
fn nested_factory_returns_are_included_without_executing_the_factory() {
    let source = r"
fn idle(int n) int { return n }
fn spin(int n) int { return spin(n) }
fn choose(bool flag) (fn(int) int) {
    if flag { true -> { { return spin } } false -> { return idle } }
}
fn compute(int n) int {
    int failure = 1 / (n - n)
    return failure + choose(true)(n)
}
@println(compute(1))
";
    assert!(compile(source, true).unwrap().contains("nc_fn_compute("));
}

#[test]
fn certified_recursion_reports_arithmetic_failures_at_the_original_expression() {
    for source in [
        r"
fn failure(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return 1 / failure(n - 1) } }
}
@println(failure(2))
",
        r"
fn failure(byte n) byte {
    if n { 0 -> { return 255 } _ -> { return 1 + failure(n - 1) } }
}
@println(failure(2))
",
    ] {
        compile(source, false).unwrap();
        let error = compile(source, true).unwrap_err();
        let diagnostic = &error.0[0];
        assert!(diagnostic.message.contains("constant evaluation failed"));
        assert_eq!(diagnostic.path.as_deref(), Some(Path::new("recursion.nc")));
        let expression = &source[diagnostic.span.clone()];
        assert!(expression.contains("failure(n - 1)"), "{expression}");
        assert!(!expression.contains("@println"));
    }
}

#[test]
fn heap_tasks_preserve_short_circuiting_and_unary_semantics() {
    let source = r"
fn lazy(uint n) bool {
    if n { 0 -> { return false } _ -> { return lazy(n - 1) and (1 / (n - n) == 0) } }
}
fn flip(uint n) bool {
    if n { 0 -> { return false } _ -> { return not flip(n - 1) } }
}
@println(lazy(1000), flip(1001))
";
    let release = compile(source, true).unwrap();
    assert!(!release.contains("nc_fn_lazy("));
    assert!(!release.contains("nc_fn_flip("));
    assert_runtime_output(source, "falsetrue\n");
}

#[test]
fn float_recursion_requires_exact_integral_non_stalling_ranks() {
    let function = r"
fn depth(float n) float {
    if n { 0.0 -> { return 0.0 } _ -> { return 1.0 + depth(n - 1.0) } }
}
";
    let source = format!("{function}@println(depth(1000.0))");
    assert!(!compile(&source, true).unwrap().contains("nc_fn_depth("));
    assert_runtime_output(&source, "1000.0\n");
    for argument in [
        "-1.0",
        "1.5",
        "18014398509481984.0",
        "1.0 / 0.0",
        "0.0 / 0.0",
    ] {
        let source = format!("{function}@println(depth({argument}))");
        assert!(
            compile(&source, true).unwrap().contains("nc_fn_depth("),
            "{argument}"
        );
    }
}

#[test]
fn certified_recursive_call_evaluates_effectful_arguments_once_in_order() {
    let source = r##"
mut uint visits = 0
fn argument(uint value) uint {
    visits = visits + 1
    @println("arg:", value, "#", visits)
    return value
}
fn depth(uint n, uint value) uint {
    if n { 0 -> { return value } _ -> { return 1 + depth(n - 1, value) } }
}
@println(depth(argument(5), argument(7)), "|", visits)
@println(depth(argument(5), argument(7)), "|", visits)
"##;
    assert_runtime_output(source, "arg:5#1\narg:7#2\n12|2\narg:5#3\narg:7#4\n12|4\n");
}

#[test]
fn recursive_non_rank_argument_failure_keeps_its_original_location() {
    let function = r"
fn failure(uint n, int x) int {
    if n { 0 -> { return x } _ -> { return failure(n - 1, 1 / x) } }
}
";
    let source = format!("{function}@println(failure(2, 2))");
    compile(&source, false).unwrap();
    let error = compile(&source, true).unwrap_err();
    let diagnostic = &error.0[0];
    assert!(diagnostic.message.contains("constant evaluation failed"));
    assert_eq!(diagnostic.path.as_deref(), Some(Path::new("recursion.nc")));
    assert_eq!(&source[diagnostic.span.clone()], "1 / x");

    let source = format!("{function}@println(failure(0, 0), failure(1, 2))");
    assert_runtime_output(&source, "00\n");
}

#[test]
fn negative_zero_rank_preserves_its_sign_in_output_and_string_conversion() {
    let source = r#"
fn depth(float n) float {
    if n { 0.0 -> { return n } _ -> { return 1.0 + depth(n - 1.0) } }
}
@println(depth(-0.0), "|", @as(str, depth(-0.0)), "|", depth(0.0), "|", depth(2.0))
"#;
    assert!(!compile(source, true).unwrap().contains("nc_fn_depth("));
    assert_runtime_output(source, "-0.0|-0.0|0.0|2.0\n");
}

#[test]
fn resolved_callable_alias_still_uses_the_actual_functions_certificate() {
    let source = r"
fn depth(uint n) uint {
    if n { 0 -> { return 0 } _ -> { return 1 + depth(n - 1) } }
}
(fn(uint) uint) alias = depth
@println(alias(10))
";
    let module = ncc::parser::parse(ncc::lexer::lex(source).unwrap()).unwrap();
    let checked = ncc::sema::check(module, Path::new("recursion.nc")).unwrap();
    let optimized = ncc::optimizer::optimize(checked).unwrap();
    let ncc::ast::Item::Statement(statement) = optimized.items.last().unwrap() else {
        panic!("expected output statement");
    };
    let ncc::ast::Stmt::Expr(expression) = statement.unlocated() else {
        panic!("expected output expression");
    };
    let ncc::ast::Expr::Call { args, .. } = expression.unlocated() else {
        panic!("expected output call");
    };
    assert!(matches!(args.as_slice(), [argument]
        if matches!(argument.unlocated(), ncc::ast::Expr::String(value) if value == "10")));
    assert_runtime_output(source, "10\n");
}
