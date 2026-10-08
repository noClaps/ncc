use std::path::Path;

fn rejects(source: &str, expected: &str) {
    for release in [false, true] {
        let Err(error) = ncc::compile_source_with_options(
            source,
            Path::new("negative_function_edges.nc"),
            release,
        ) else {
            panic!(
                "release={release}, expected diagnostic containing {expected:?}, but compilation succeeded\nsource:\n{source}"
            );
        };
        assert!(
            error.to_string().contains(expected),
            "release={release}, expected diagnostic containing {expected:?}\nsource:\n{source}\ndiagnostic:\n{error}"
        );
    }
}

fn accepts(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("function_edges_control.nc"), release)
            .unwrap_or_else(|error| {
                panic!("release={release}\nsource:\n{source}\ndiagnostic:\n{error}")
            });
    }
}

#[test]
fn named_functions_cannot_return_values_without_annotations() {
    for source in [
        "fn bad() { return true }",
        "fn bad() { return \"value\" }",
        "fn value() int { return 1 };fn bad() { return value() }",
    ] {
        rejects(source, "expected `void`, found");
    }
}

#[test]
fn anonymous_functions_cannot_infer_return_annotations_from_values_or_context() {
    for source in [
        "fn callback = fn() { return \"value\" }",
        "fn callback = fn() { return 1, true }",
        "(fn() str) callback = fn() { return \"value\" }",
    ] {
        rejects(source, "expected `void`, found");
    }
}

#[test]
fn specialized_generic_functions_still_require_return_annotations() {
    for source in [
        "fn bad<type T>(T value) { return value };bad<int>(1)",
        "fn bad<type T>(T value) { return value };bad<str>(\"value\")",
    ] {
        rejects(source, "expected `void`, found");
    }
}

#[test]
fn function_local_bindings_do_not_escape_to_outer_or_sibling_scopes() {
    for (source, name) in [
        ("fn owner() { int local = 1 };_ = local", "local"),
        ("fn owner(int parameter) {};_ = parameter", "parameter"),
        (
            "fn owner(int value) { mut int shadow = value };_ = shadow",
            "shadow",
        ),
        ("fn callback = fn() { int local = 1 };_ = local", "local"),
        (
            "fn owner() { int local = 1 };fn sibling() int { return local }",
            "local",
        ),
        (
            "fn outer() { fn inner() { int local = 1 };_ = local }",
            "local",
        ),
    ] {
        rejects(source, &format!("unknown name `{name}`"));
    }
}

#[test]
fn named_functions_reject_none_for_nonoptional_parameters() {
    for source in [
        "fn take(int value) {};take(none)",
        "fn take(str? first, str second) {};take(\"ok\", none)",
        "fn take(int[] values) {};take(none)",
    ] {
        rejects(source, "cannot infer type of none");
    }
}

#[test]
fn anonymous_functions_reject_none_for_nonoptional_parameters() {
    for source in [
        "fn take = fn(int value) {};take(none)",
        "(fn(str) void) take = fn(str value) {};take(none)",
        "fn take = fn(int? first, int second) {};take(none, none)",
    ] {
        rejects(source, "cannot infer type of none");
    }
}

#[test]
fn explicit_generic_specializations_reject_none_for_nonoptional_parameters() {
    for source in [
        "fn take<type T>(T value) {};take<int>(none)",
        "fn take<type T>(T value) {};take<str>(none)",
        "fn take<type T>(T? first, T second) {};take<int>(none, none)",
    ] {
        rejects(source, "cannot infer type of none");
    }
}

#[test]
fn unparenthesized_function_types_reject_in_representative_positions() {
    for source in [
        "fn take(fn(str) bool callback) {}",
        "fn make() fn(str) bool { return fn(str value) bool { return true } }",
        "struct Holder { fn(str) bool callback }",
    ] {
        rejects(source, "expected identifier");
    }
}

#[test]
fn annotated_returns_optional_parameters_and_parenthesized_types_compile() {
    accepts(
        r#"
fn named() str { return "value" }
fn callback = fn() str { return "value" }
fn generic<type T>(T value) T { return value }
fn notify() { return }
fn anonymous_notify = fn() { return }
fn take(int? value) {}
fn anonymous_take = fn(int? value) {}
fn generic_take<type T>(T value) {}
fn generic_optional_take<type T>(T? value) {}
fn predicate(str value) bool { return true }
fn apply((fn(str) bool) callback) bool { return callback("value") }
fn make() (fn(str) bool) { return predicate }
struct Holder { (fn(str) bool) callback }
_ = named()
_ = callback()
_ = generic<int>(1)
notify()
anonymous_notify()
take(none)
anonymous_take(none)
generic_take<int?>(none)
generic_optional_take<int>(none)
_ = apply(predicate)
(fn(str) bool) returned = make()
Holder holder = Holder{.callback = returned}
"#,
    );
}
