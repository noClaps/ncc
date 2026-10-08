use super::super::super::{EvaluationIndex, Expr, HashMap, Item, Stmt, precompute_output};

fn output(source: &str) -> Option<Vec<String>> {
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let checked = crate::sema::check(module, std::path::Path::new("helper-proof.nc")).unwrap();
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(function) = item {
                Some((function.name.clone(), function))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    precompute_output(&checked, &functions, &EvaluationIndex::new(&checked)).map(|items| {
        items
            .iter()
            .map(|item| {
                let Item::Statement(statement) = item else {
                    panic!("expected output")
                };
                let Stmt::Expr(value) = statement.unlocated() else {
                    panic!("expected call")
                };
                let Expr::Call { args, .. } = value.unlocated() else {
                    panic!("expected call")
                };
                let Expr::String(bytes) = args[0].unlocated() else {
                    panic!("expected snapshot")
                };
                bytes.clone()
            })
            .collect()
    })
}

#[test]
fn early_void_returns_are_local_to_the_helper() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance(int step) {
    i = i + step
    if i { 1 -> { return } _ -> {} }
    i = i + 1
}
while i < 5 { advance(1) }
@println(i)
"
        ),
        Some(vec!["5".into()])
    );
}

#[test]
fn early_scalar_returns_preserve_branch_effects() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    i = i + 1
    if i { 1 -> { return i < 5 } _ -> {} }
    i = i + 1
    return i < 5
}
while condition() {}
@println(i)
"
        ),
        Some(vec!["5".into()])
    );
}

#[test]
fn literal_counted_helper_loop_supplies_outer_progress() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance(int step) {
    mut int j = 0
    while j < 2 {
        j = j + 1
        i = i + step
    }
}
while i < 6 { advance(1) }
@println(i)
"
        ),
        Some(vec!["6".into()])
    );
}

#[test]
fn arguments_have_effects_in_order_and_keep_copies() {
    assert_eq!(
        output(
            r"
mut int i = 0
mut int calls = 0
fn argument() int { calls = calls + 1;return 1 }
fn advance(int step, int other) {
    i = i + step
    { mut int step = 100;step = step + other }
}
while i < 3 { advance(argument(), argument()) }
@println(i)
@println(calls)
"
        ),
        Some(vec!["3".into(), "6".into()])
    );
    // Substituting the first parameter with the shared cell would manufacture
    // progress: its real copied value is zero, and the helper restores i.
    assert!(
        output(
            r"
mut int i = 0
fn argument() int { i = i + 1;return 1 }
fn advance(int saved, int ignored) { i = saved }
while i < 3 { advance(i, argument()) }
@println(i)
"
        )
        .is_none()
    );
}

#[test]
fn uncertified_helper_loops_and_coercions_stay_runtime() {
    assert!(
        output(
            r"
mut int i = 0
fn advance() { while true {};i = i + 1 }
while i < 3 { advance() }
@println(i)
"
        )
        .is_none()
    );
    assert!(
        output(
            r"
mut int i = 0
fn step() int? { return 1 }
fn advance(int? n) { i = i + (n else 0) }
while i < 3 { advance(step()) }
@println(i)
"
        )
        .is_none()
    );
}

#[test]
fn snapshots_precede_later_argument_mutations() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn argument() int { i = i + 1;return 1 }
fn advance(int before, int ignored, int after) {
    @println(before)
    @println(after)
    i = i + 1
}
while i < 4 { advance(i, argument(), i) }
@println(i)
"
        ),
        Some(vec![
            "0".into(),
            "1".into(),
            "2".into(),
            "3".into(),
            "4".into()
        ])
    );
}

#[test]
fn pure_parameterized_calls_remain_usable_inside_comparisons() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn identity(int n) int { return n }
while i < identity(3) { i = i + 1 }
@println(i)
"
        ),
        Some(vec!["3".into()])
    );
}

#[test]
fn nested_argument_calls_do_not_create_a_recursive_body_edge() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn identity(int n) int { return n }
fn advance(int step) { i = i + step }
while i < 3 { advance(identity(identity(1))) }
@println(i)
"
        ),
        Some(vec!["3".into()])
    );
}

#[test]
fn array_helper_loop_supplies_outer_progress() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance() { for key in [10, 20, 30] { i = i + 1 } }
while i < 6 { advance() }
@println(i)
"
        ),
        Some(vec!["6".into()])
    );
}

#[test]
fn early_returns_cross_helper_loops_but_not_the_caller() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    i = i + 1
    mut int j = 0
    while j < 2 {
        j = j + 1
        if j { 1 -> { return i < 3 } _ -> {} }
    }
    return i < 3
}
while condition() {}
@println(i)
"
        ),
        Some(vec!["3".into()])
    );
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance() {
    i = i + 1
    for key in [10, 20] { return }
    i = i + 100
}
while i < 3 { advance() }
@println(i)
"
        ),
        Some(vec!["3".into()])
    );
}

#[test]
fn a_return_path_without_progress_is_not_a_caller_exit() {
    assert!(
        output(
            r"
mut int i = 0
fn advance() {
    if i { 0 -> { return } _ -> {} }
    i = i + 1
}
while i < 3 { advance() }
@println(i)
"
        )
        .is_none()
    );
}

// Parameters are immutable in checked NC today. Build a defensive proof-layer
// fixture by renaming a checked mutable local's references without moving its
// expression nodes or changing types, then ask for a certificate, never execution.
fn writable_parameter_proven(source: &str) -> bool {
    use super::super::super::{Evaluator, Value};
    let module = crate::parser::parse(crate::lexer::lex(source).unwrap()).unwrap();
    let mut checked =
        crate::sema::check(module, std::path::Path::new("writable-proof.nc")).unwrap();
    for item in &mut checked.module.items {
        if matches!(item, Item::Function(function) if function.name == "helper") {
            crate::visit::rewrite(item, &mut |value| {
                if let Expr::Name(name) = value.unlocated_mut()
                    && name == "local"
                {
                    *name = "parameter".into();
                }
            });
        }
    }
    let functions = checked
        .module
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Function(function) = item {
                Some((function.name.clone(), function))
            } else {
                None
            }
        })
        .collect();
    let mut evaluator = Evaluator::new(&functions, &checked);
    evaluator.analyse_output = true;
    let mut env: HashMap<String, Value> = HashMap::new();
    for item in &checked.module.items {
        if let Item::Global(declaration) = item {
            evaluator
                .declaration(declaration, &mut env, &mut Vec::new())
                .unwrap();
        }
    }
    evaluator.analysis_globals.clone_from(&env);
    let (condition, body, label) = checked
        .module
        .items
        .iter()
        .find_map(|item| {
            if let Item::Statement(statement) = item
                && let Stmt::While {
                    condition,
                    body,
                    label,
                } = statement.unlocated()
            {
                return Some((condition, body, label.as_deref()));
            }
            None
        })
        .unwrap();
    let before = evaluator.cells.clone();
    let proven = evaluator.helper_loop_proven(condition, body, label, &env);
    assert_eq!(
        evaluator.cells, before,
        "proof expansion must not execute effects"
    );
    proven
}

#[test]
fn writable_parameter_copies_cannot_supply_caller_progress() {
    assert!(!writable_parameter_proven(
        r"
mut int i = 0
fn helper(int parameter) bool {
    mut int local = parameter
    local = local + 1
    return local < 3
}
while helper(i) {}
"
    ));
    assert!(!writable_parameter_proven(
        r"
mut int i = 0
fn helper(int parameter) {
    mut int local = parameter
    local = local + 1
}
while i < 3 { helper(i) }
"
    ));
    assert!(writable_parameter_proven(
        r"
mut int i = 0
fn helper(int parameter) {
    mut int local = parameter
    local = local + 1
    i = i + 1
}
while i < 3 { helper(0) }
"
    ));
}

#[test]
fn lexical_shadows_do_not_disable_known_parameter_steps() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance(int step) {
    { mut int step = 100;step = step + 1 }
    mut int j = 0
    while j < 2 { j = j + 1;i = i + step }
}
while i < 6 { advance(1) }
@println(i)
"
        ),
        Some(vec!["6".into()])
    );
}

#[test]
fn labeled_conditionals_consume_only_their_own_exits() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn advance() {
    i = i + 1
    branch: if i { 1 -> { return } _ -> { break :branch } }
    i = i + 1
}
while i < 5 { advance() }
@println(i)
"
        ),
        Some(vec!["5".into()])
    );
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    i = i + 1
    branch: if i { 1 -> { return i < 5 } _ -> { break :branch } }
    i = i + 1
    return i < 5
}
while condition() {}
@println(i)
"
        ),
        Some(vec!["5".into()])
    );
}

#[test]
fn varied_monotonic_condition_returns_use_a_safe_envelope() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    if i < 3 { true -> { return i < 5 } false -> {} }
    return i < 6
}
while condition() { i = i + 1 }
@println(i)
"
        ),
        Some(vec!["6".into()])
    );
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    i = i + 1
    branch: if i >= 3 { true -> { return false } false -> { break :branch } }
    return i < 6
}
while condition() {}
@println(i)
"
        ),
        Some(vec!["3".into()])
    );
}

#[test]
fn condition_envelopes_never_replace_exact_call_values() {
    use super::{BinaryOp, ResultUse, helper_result};
    let returns: Vec<_> = [5, 6]
        .into_iter()
        .map(|bound| Expr::Binary {
            left: Box::new(Expr::Name("counter".into())),
            op: BinaryOp::Lt,
            right: Box::new(Expr::Int(bound.to_string())),
        })
        .collect();
    assert!(helper_result(&returns, ResultUse::Exact).is_none());
    let envelope = helper_result(&returns, ResultUse::Condition).unwrap();
    let Expr::Binary { right, .. } = envelope else {
        panic!("expected comparison")
    };
    assert!(matches!(*right, Expr::Int(ref value) if value == "6"));
}

#[test]
fn varied_descending_and_inclusive_conditions_use_the_widest_bound() {
    assert_eq!(
        output(
            r"
mut int i = 6
fn condition() bool {
    if i > 3 { true -> { return i > 1 } false -> {} }
    return i > 0
}
while condition() { i = i - 1 }
@println(i)
"
        ),
        Some(vec!["0".into()])
    );
    assert_eq!(
        output(
            r"
mut int i = 0
fn condition() bool {
    if i < 3 { true -> { return i <= 4 } false -> {} }
    return i <= 5
}
while condition() { i = i + 1 }
@println(i)
"
        ),
        Some(vec!["6".into()])
    );
}

#[test]
fn varied_unbounded_or_different_counter_returns_stay_unproven() {
    for final_return in ["true", "j < 6"] {
        assert!(
            output(&format!(
                r"
mut int i = 0
mut int j = 0
fn condition() bool {{
    if i < 3 {{ true -> {{ return i < 5 }} false -> {{}} }}
    return {final_return}
}}
while condition() {{ i = i + 1 }}
@println(i)
"
            ))
            .is_none()
        );
    }
}

#[test]
fn nested_effectful_calls_supply_assignment_progress() {
    assert_eq!(
        output(
            r"
mut int i = 0
mut int checks = 0
fn argument() int { checks = checks + 1;return i }
fn next(int value) int { return value + 1 }
while i < 3 { i = next(argument()) }
@println(i)
@println(checks)
"
        ),
        Some(vec!["3".into(), "3".into()])
    );
    assert_eq!(
        output(
            r"
mut int i = 0
mut int checks = 0
fn argument() int { checks = checks + 1;return 1 }
fn next(int value) int { return value }
while i < 3 { i = i + next(argument()) }
@println(i)
@println(checks)
"
        ),
        Some(vec!["3".into(), "3".into()])
    );
}

#[test]
fn helper_declarations_and_output_expand_scalar_effects() {
    assert_eq!(
        output(
            r"
mut int i = 0
mut int checks = 0
fn argument() int { checks = checks + 1;return 1 }
fn check() bool { checks = checks + 1;return true }
fn advance() {
    int step = argument()
    { int step = argument() }
    i = i + step
    bool checked = check()
    @println(i, argument())
}
while i < 3 { advance() }
@println(checks)
"
        ),
        Some(vec!["11".into(), "21".into(), "31".into(), "12".into()])
    );
}

#[test]
fn output_arguments_keep_earlier_scalar_snapshots() {
    assert_eq!(
        output(
            r"
mut int i = 0
fn bump() int { i = i + 1;return 1 }
while i < 4 {
    @println(i, bump(), i)
    i = i + 1
}
@println(i)
"
        ),
        Some(vec!["011".into(), "213".into(), "4".into()])
    );
}

#[test]
fn assignment_rhs_precedes_effectful_target_indices() {
    assert_eq!(
        output(
            r"
mut int i = 0
mut int[] values = [99]
fn index() uint { i = i + 1;return 0u }
while i < 3 { values[index()] = i }
@println(i)
@println(values[0])
"
        ),
        Some(vec!["3".into(), "2".into()])
    );
}

#[test]
fn immutable_global_and_capture_loop_inputs_are_proven_without_execution() {
    for callable in [
        "fn advance() { mut int j = start;while j < limit { j = j + step;i = i + 1 } }",
        "fn advance = fn() { mut int j = start;while j < limit { j = j + step;i = i + 1 } }",
    ] {
        assert_eq!(
            output(&format!(
                r"
mut int i = 0
int start = 0
int limit = 2
int step = 1
{callable}
while i < 6 {{ advance() }}
@println(i)
"
            )),
            Some(vec!["6".into()])
        );
    }
    assert!(
        output(
            r"
mut int i = 0
mut int limit = 2
fn advance() {
    mut int j = 0
    while j < limit { j = j + 1;i = i + 1 }
}
while i < 6 { advance() }
@println(i)
"
        )
        .is_none()
    );
}

#[test]
fn known_short_circuit_operands_skip_effects() {
    assert_eq!(
        output(
            r"
mut int i = 0
mut int checks = 0
fn check() bool { checks = checks + 1;return true }
while i < 3 { bool ignored = false and check();i = i + 1 }
@println(i, checks)
"
        ),
        Some(vec!["30".into()])
    );
}

#[test]
fn closed_form_counts_include_inclusive_and_descending_bounds() {
    use super::{BinaryOp, iterations};
    assert_eq!(iterations(0, 5, 2, BinaryOp::Lt), Some(3));
    assert_eq!(iterations(0, 4, 2, BinaryOp::Le), Some(3));
    assert_eq!(iterations(5, 0, -2, BinaryOp::Gt), Some(3));
    assert_eq!(iterations(4, 0, -2, BinaryOp::Ge), Some(3));
    assert_eq!(iterations(0, 5, 2, BinaryOp::Ne), None);
    assert_eq!(iterations(0, 4, 2, BinaryOp::Ne), Some(2));
}
