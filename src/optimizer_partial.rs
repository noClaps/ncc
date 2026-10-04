//! Statement-sized transactions preserve storage and effects across runtime barriers.
use super::{
    CheckedModule, Evaluator, Expr, Flow, Function, HashMap, HashSet, Item, Pattern, Stmt, Type,
    Value, VarDecl, materialize,
};

pub(super) fn effectful_index(index: &Expr) -> bool {
    let mut effectful = false;
    crate::visit::expr(index, &mut |e| {
        effectful |= matches!(
            e,
            Expr::Call { .. }
                | Expr::Lambda(_)
                | Expr::Async(_)
                | Expr::If { .. }
                | Expr::Else { .. }
                | Expr::Catch { .. }
                | Expr::Try(_)
        );
    });
    effectful
}

pub(super) fn precompute(
    checked: &CheckedModule,
    functions: &HashMap<String, &Function>,
    prefix: &[(usize, Option<Expr>)],
) -> Vec<(usize, Vec<Item>)> {
    let consumed: HashSet<_> = prefix.iter().map(|(index, _)| *index).collect();
    let mut env = HashMap::new();
    let mut cells = vec![];
    let mut globals = Vec::new();
    let mut replacements = vec![];
    let mut has_async = false;
    for item in &checked.module.items {
        crate::visit::item(item, &mut |e| has_async |= matches!(e, Expr::Async(_)));
    }
    let mut concurrent = false;
    for (index, item) in checked.module.items.iter().enumerate() {
        if !matches!(
            item,
            Item::Global(_) | Item::Statement(_) | Item::Test { .. }
        ) {
            continue;
        }
        let mut evaluator = Evaluator::new(functions, checked);
        evaluator.analyse_output = true;
        evaluator.recorded_output = Some(vec![]);
        evaluator.cells.clone_from(&cells);
        if concurrent {
            evaluator.volatile_cells.extend(0..cells.len());
        }
        evaluator.analysis_globals.clone_from(&env);
        let mut next = env.clone();
        let mut initializer = None;
        let flow = match item {
            Item::Global(v) => {
                initializer = evaluator.declaration_snapshot(v, &mut next, &mut vec![]);
                initializer.as_ref().map(|_| Flow::Next)
            }
            Item::Statement(s) => evaluator.statements(std::slice::from_ref(s), &mut next, false),
            Item::Test { body, .. } => evaluator.block(body, &mut next),
            _ => unreachable!(),
        };
        let replacement = matches!(flow, Some(Flow::Next))
            .then(|| {
                replacement(
                    item,
                    &globals,
                    &env,
                    &cells,
                    initializer.as_ref(),
                    &evaluator,
                    checked,
                )
            })
            .flatten();
        if let Some(items) = replacement {
            env = next;
            cells = evaluator.cells;
            if concurrent {
                cells.fill(Value::Unknown);
            }
            if !consumed.contains(&index) && !matches!(item, Item::Test { .. }) {
                replacements.push((index, items));
            }
        } else {
            // Async work may outlive discarded futures and may be hidden in a
            // retained call. Without a join proof, exposure remains permanent.
            concurrent |= has_async;
            // An unknown call may invoke an escaped callback. Forget every shared
            // cell, including those reached only through immutable closures.
            cells.fill(Value::Unknown);

            if let Item::Global(v) = item {
                for name in v.binding_names() {
                    let value = if v.mutable {
                        let cell = cells.len();
                        cells.push(Value::Unknown);
                        Value::Cell(cell)
                    } else {
                        Value::Unknown
                    };
                    env.insert(name.to_owned(), value);
                }
            }
            // A failed assertion must not permit analysis of its continuation.
            if matches!(item, Item::Test { .. })
                || matches!(item, Item::Statement(s) if contains_assertion(s))
                || !item_reaches_next(item, checked)
            {
                break;
            }
        }
        if let Item::Global(v) = item {
            globals.push(v);
        }
    }
    replacements
}

fn item_reaches_next(item: &Item, checked: &CheckedModule) -> bool {
    match item {
        Item::Global(v) => {
            crate::flow::expression_reaches_next(&v.value, &checked.expression_types)
        }
        Item::Statement(s) => crate::flow::statement_reaches_next(s, &checked.expression_types),
        _ => true,
    }
}

fn contains_assertion(statement: &Stmt) -> bool {
    match statement.unlocated() {
        Stmt::Assert(_) => true,
        Stmt::Block(body)
        | Stmt::While { body, .. }
        | Stmt::For { body, .. }
        | Stmt::Lock { body, .. } => body.statements.iter().any(contains_assertion),
        Stmt::Expr(e) | Stmt::LabeledIf { value: e, .. } => {
            let mut found = false;
            crate::visit::expr(e, &mut |e| {
                if let Expr::If { arms, .. } = e {
                    found |= arms
                        .iter()
                        .any(|(_, body)| body.statements.iter().any(contains_assertion));
                }
            });
            found
        }
        _ => false,
    }
}

fn stored<'a>(value: &'a Value, cells: &'a [Value]) -> Option<&'a Value> {
    match value {
        Value::Cell(index) => cells.get(*index),
        value => Some(value),
    }
}

fn replacement(
    item: &Item,
    globals: &[&VarDecl],
    before: &HashMap<String, Value>,
    cells: &[Value],
    initializer: Option<&Value>,
    evaluator: &Evaluator<'_>,
    checked: &CheckedModule,
) -> Option<Vec<Item>> {
    let mut items = evaluator.recorded_output.clone()?;
    let visible_cells: HashSet<_> = before
        .values()
        .filter_map(|value| {
            if let Value::Cell(index) = value {
                Some(*index)
            } else {
                None
            }
        })
        .collect();
    if cells
        .iter()
        .zip(&evaluator.cells)
        .enumerate()
        .any(|(index, (old, new))| old != new && !visible_cells.contains(&index))
    {
        return None;
    }
    let mut seen = HashSet::new();
    for global in globals.iter().rev() {
        for (name, ty) in binding_types(&global.pattern, &global.ty) {
            if !seen.insert(name) {
                continue;
            }
            let binding = before.get(name)?;
            let previous = stored(binding, cells)?;
            let value = stored(binding, &evaluator.cells)?;
            if previous != value {
                if !value.materializable() || !global.mutable || global.mutex {
                    return None;
                }
                let value = super::constants::expression(value.clone(), ty, checked);
                let statement = Stmt::Assign {
                    target: Expr::Name(name.to_owned()),
                    value,
                };
                items.push(Item::Statement(Stmt::Located(
                    Box::new(statement),
                    crate::ast::SourceLocation {
                        path: global.source_path.clone(),
                        span: global.span.clone(),
                    },
                )));
            }
        }
    }
    if let Item::Global(v) = item {
        let mut declaration = v.clone();
        if let Some(value) = initializer.filter(|value| value.materializable()) {
            if checked.expression_types.get(&v.value.id()) == Some(&v.ty) {
                declaration.value = materialize(value.clone(), &v.value, checked);
            } else {
                *declaration.value.unlocated_mut() =
                    super::constants::expression(value.clone(), &v.ty, checked);
            }
        } else if !matches!(v.value.unlocated(), Expr::Lambda(_)) || !items.is_empty() {
            return None;
        }
        items.push(Item::Global(declaration));
    }
    Some(items)
}

fn binding_types<'a>(pattern: &'a Pattern, ty: &'a Type) -> Vec<(&'a str, &'a Type)> {
    match (pattern, ty) {
        (Pattern::Name(name), ty) if name != "_" => vec![(name, ty)],
        (Pattern::Tuple(patterns), Type::Tuple(types)) => patterns
            .iter()
            .zip(types)
            .flat_map(|(p, t)| binding_types(p, t))
            .collect(),
        _ => vec![],
    }
}
