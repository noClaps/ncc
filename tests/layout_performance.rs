use std::{fmt::Write as _, path::Path};

#[test]
fn shared_alias_and_struct_dags_validate_without_expanding_every_path() {
    let mut source = String::from("type A0 = int\nstruct S0 { int value }\n");
    // Repeated edges at this depth make path-by-path validation exponential.
    // Avoid a wall-clock assertion so the test remains usable on slow machines.
    for depth in 1..=24 {
        let previous = depth - 1;
        writeln!(source, "type A{depth} = (A{previous}, A{previous})").unwrap();
        writeln!(
            source,
            "struct S{depth} {{ S{previous} left S{previous} right }}"
        )
        .unwrap();
    }
    accepts_in_both_modes(&source);
}

#[test]
fn completed_alias_validation_does_not_skip_infinite_layouts() {
    rejects_in_both_modes(
        "// A is valid in alias-only traversal, but not in layout traversal.\ntype A = B\nstruct B { A value }\n",
        "type A = B",
        "recursive type `A` has infinite size; use an array or enum payload to break the cycle",
    );
    rejects_in_both_modes(
        "type A = B[]\nstruct B { B value }\n",
        "struct B { B value }",
        "recursive type `B` has infinite size; use an array or enum payload to break the cycle",
    );
}

#[test]
fn nominal_cycles_remain_errors_through_container_and_callable_edges() {
    for underlying in [
        "Cycle",
        "Cycle[]",
        "Cycle[0]",
        "Cycle?",
        "Cycle!",
        "(int, Cycle)",
        "[str]Cycle",
        "[Cycle]int",
        "(fn(Cycle) int)",
        "(fn() Cycle)",
    ] {
        let declaration = format!("type Cycle = {underlying}");
        rejects_in_both_modes(
            &format!("type Before = int\n{declaration}\n"),
            &declaration,
            "cyclic nominal type definition involving `Cycle`",
        );
    }
    rejects_in_both_modes(
        "type B = A?\ntype A = [str]B\n",
        "type A = [str]B",
        "cyclic nominal type definition involving `A`",
    );
}

#[test]
fn layout_cycles_remain_errors_through_by_value_edges() {
    for field in ["Loop", "Loop?", "Loop!", "(int, Loop)"] {
        let declaration = format!("struct Loop {{ {field} value }}");
        rejects_in_both_modes(
            &format!("type Before = int\n{declaration}\n"),
            &declaration,
            "recursive type `Loop` has infinite size; use an array or enum payload to break the cycle",
        );
    }
    rejects_in_both_modes(
        "struct B { A? value }\nstruct A { B value }\n",
        "struct A { B value }",
        "recursive type `A` has infinite size; use an array or enum payload to break the cycle",
    );
}

#[test]
fn recursive_indirection_still_has_finite_layouts() {
    accepts_in_both_modes(
        "type Children = Node[]\n\
         struct Node { Children children }\n\
         struct Parent { Child[] children }\n\
         struct Child { Parent parent }\n\
         enum Tree { Branch(Link) Empty }\n\
         struct Link { Tree tree }\n\
         struct MapNode { [str]MapNode children }\n\
         struct CallbackNode { (fn() CallbackNode) next }\n",
    );
}

#[test]
fn cycle_diagnostics_keep_sorted_root_and_child_order() {
    rejects_in_both_modes(
        "type ZCycle = ZCycle\n\
         type BCycle = BCycle\n\
         type AGood = int\n\
         type ARoot = (AGood, ZCycle, BCycle)\n",
        "type ARoot = (AGood, ZCycle, BCycle)",
        "cyclic nominal type definition involving `ZCycle`",
    );
    rejects_in_both_modes(
        "struct ZCycle { ZCycle value }\n\
         struct BCycle { BCycle value }\n\
         struct AGood { int value }\n\
         struct ARoot { AGood good ZCycle first BCycle second }\n",
        "struct ARoot { AGood good ZCycle first BCycle second }",
        "recursive type `ZCycle` has infinite size; use an array or enum payload to break the cycle",
    );
}

fn accepts_in_both_modes(source: &str) {
    for release in [false, true] {
        ncc::compile_source_with_options(source, Path::new("layouts.nc"), release)
            .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
    }
}

fn rejects_in_both_modes(source: &str, declaration: &str, message: &str) {
    let path = Path::new("layouts.nc");
    let start = source.find(declaration).unwrap();
    let line = source[..start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    for release in [false, true] {
        let error = ncc::compile_source_with_options(source, path, release).unwrap_err();
        assert_eq!(error.0.len(), 1, "release={release}: {error}");
        let diagnostic = &error.0[0];
        assert_eq!(diagnostic.message, message, "release={release}");
        assert_eq!(diagnostic.path.as_deref(), Some(path), "release={release}");
        assert_eq!(
            diagnostic.span,
            start..start + declaration.len(),
            "release={release}"
        );
        assert_eq!(
            error.render(source, path),
            format!(
                "layouts.nc:{line}:1: error: {message}\n  |\n{line:>2} | {declaration}\n  | ^\n"
            ),
            "release={release}"
        );
    }
}
