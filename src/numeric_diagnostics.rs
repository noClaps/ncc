//! Effect-free arithmetic diagnostics shared by debug and release compilation.
use crate::{
    ast::{BinaryOp, Block, Expr, Item, Pattern, Stmt, Type, UnaryOp},
    diagnostic::Diagnostics,
    flow, optimizer,
    sema::CheckedModule,
    visit,
};
use std::collections::{HashMap, HashSet};

pub(crate) fn check(checked: &CheckedModule) -> Result<Diagnostics, Diagnostics> {
    let mut expressions = HashMap::new();
    for item in &checked.module.items {
        visit::item(item, &mut |expression| {
            expressions.insert(expression.id(), expression);
        });
    }
    let mut analysis = Analysis {
        checked,
        expressions,
        pure: HashMap::new(),
        visiting: HashSet::new(),
        warnings: Diagnostics::default(),
    };
    let mut reached = true;
    for item in &checked.module.items {
        match item {
            Item::Function(function) => analysis.block(&function.body, true)?,
            Item::Global(declaration) => {
                analysis.expression(&declaration.value, reached)?;
                reached &= analysis.reaches_next(&declaration.value);
            }
            Item::Statement(statement) => {
                analysis.statement(statement, reached)?;
                reached &= flow::statement_reaches_next(statement, &checked.expression_types);
            }
            Item::Test { body, .. } => {
                analysis.block(body, reached)?;
                reached &= flow::block_reaches_next(body, &checked.expression_types);
            }
            _ => {}
        }
    }
    analysis.warnings.0.sort_by(|left, right| {
        (&left.path, left.span.start, left.span.end).cmp(&(
            &right.path,
            right.span.start,
            right.span.end,
        ))
    });
    analysis
        .warnings
        .0
        .dedup_by(|left, right| left.path == right.path && left.span == right.span);
    Ok(analysis.warnings)
}

struct Analysis<'a> {
    checked: &'a CheckedModule,
    expressions: HashMap<usize, &'a Expr>,
    pure: HashMap<usize, bool>,
    visiting: HashSet<usize>,
    warnings: Diagnostics,
}

impl Analysis<'_> {
    fn is_pure(&mut self, expression: &Expr) -> bool {
        let id = expression.id();
        if let Some(pure) = self.pure.get(&id) {
            return *pure;
        }
        if !self.visiting.insert(id) {
            return false;
        }
        let pure = match expression.unlocated() {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::Bytes(_)
            | Expr::None => true,
            Expr::Name(name) if name == "$" => true,
            Expr::Name(_) => self
                .checked
                .constant_sources
                .get(&id)
                .copied()
                .flatten()
                .and_then(|source| self.expressions.get(&source).copied())
                .is_some_and(|initializer| self.is_pure(initializer)),
            Expr::Cast { value, .. }
            | Expr::Unary { value, .. }
            | Expr::Member { object: value, .. } => self.is_pure(value),
            Expr::Binary { left, right, .. } => self.is_pure(left) && self.is_pure(right),
            Expr::Index { object, index } => self.is_pure(object) && self.is_pure(index),
            Expr::Array(values) | Expr::Tuple(values) => {
                values.iter().all(|value| self.is_pure(value))
            }
            Expr::Map(entries) => entries
                .iter()
                .all(|(key, value)| self.is_pure(key) && self.is_pure(value)),
            Expr::StructInit { fields, .. } => fields.iter().all(|(_, value)| self.is_pure(value)),
            // Even a pure-looking call may hide effects, captures, or unproved loops.
            _ => false,
        };
        self.visiting.remove(&id);
        self.pure.insert(id, pure);
        pure
    }

    fn value(&mut self, expression: &Expr) -> Result<Option<Expr>, Diagnostics> {
        if self.is_pure(expression) {
            optimizer::evaluate_static_expression(expression, self.checked)
        } else {
            Ok(None)
        }
    }

    fn boolean(&mut self, expression: &Expr) -> Option<bool> {
        self.value(expression).ok().flatten().and_then(|value| {
            if let Expr::Bool(value) = value.unlocated() {
                Some(*value)
            } else {
                None
            }
        })
    }

    fn reaches_next(&self, expression: &Expr) -> bool {
        flow::expression_reaches_next(expression, &self.checked.expression_types)
    }

    fn block(&mut self, block: &Block, mut reached: bool) -> Result<(), Diagnostics> {
        for statement in &block.statements {
            self.statement(statement, reached)?;
            reached &= flow::statement_reaches_next(statement, &self.checked.expression_types);
        }
        Ok(())
    }

    fn statement(&mut self, statement: &Stmt, reached: bool) -> Result<(), Diagnostics> {
        let result = match statement.unlocated() {
            Stmt::Block(block) | Stmt::Lock { body: block, .. } => self.block(block, reached),
            Stmt::Var(declaration) => self.expression(&declaration.value, reached),
            Stmt::Assign { target, value } => {
                self.expression(value, reached)?;
                self.assignment_target(target, reached && self.reaches_next(value))
            }
            Stmt::Expr(value)
            | Stmt::Throw(value)
            | Stmt::Assert(value)
            | Stmt::LabeledIf { value, .. } => self.expression(value, reached),
            Stmt::Return(value) | Stmt::Break(value, _) => {
                if let Some(value) = value {
                    self.expression(value, reached)?;
                }
                Ok(())
            }
            Stmt::While {
                condition, body, ..
            } => {
                self.expression(condition, reached)?;
                let body_reached = reached && self.boolean(condition) == Some(true);
                self.block(body, body_reached)
            }
            Stmt::For { iterable, body, .. } => {
                self.expression(iterable, reached)?;
                // Inspect syntax, but never run an iteration to establish facts.
                self.block(body, false)
            }
            Stmt::Continue(_) => Ok(()),
            Stmt::Located(_, _) => unreachable!(),
        };
        result.map_err(|error| match statement.source() {
            Some((path, span)) => error.at_source(path, span.clone()),
            None => error,
        })
    }

    fn assignment_target(&mut self, target: &Expr, reached: bool) -> Result<(), Diagnostics> {
        // The final map key is an insertion, not a read of an existing entry.
        match target.unlocated() {
            Expr::Index { object, index } => {
                self.expression(object, reached)?;
                self.expression(index, reached && self.reaches_next(object))
            }
            Expr::Member { object, .. } => self.assignment_target(object, reached),
            _ => self.expression(target, reached),
        }
    }

    fn expression(&mut self, expression: &Expr, reached: bool) -> Result<(), Diagnostics> {
        let result = self.expression_inner(expression, reached);
        result.map_err(|error| match expression.location() {
            Some(location) => error.at_source(&location.path, location.span.clone()),
            None => error,
        })
    }

    fn expression_inner(&mut self, expression: &Expr, reached: bool) -> Result<(), Diagnostics> {
        match expression.unlocated() {
            Expr::Binary { left, op, right } => {
                self.expression(left, reached)?;
                let right_reached = reached
                    && self.reaches_next(left)
                    && match op {
                        BinaryOp::And => self.boolean(left) == Some(true),
                        BinaryOp::Or => self.boolean(left) == Some(false),
                        _ => true,
                    };
                self.expression(right, right_reached)?;
                if *op == BinaryOp::Pow {
                    self.power_warning(expression, left, right);
                }
                if right_reached {
                    self.check_right_operand(*op, left, right)?;
                }
                if reached {
                    self.value(expression)?;
                }
            }
            Expr::Unary { value, .. } | Expr::Cast { value, .. } => {
                // The evaluator recognizes -9223372036854775808 as one signed literal.
                if reached {
                    self.value(expression)?;
                }
                self.expression(value, reached)?;
            }
            Expr::Call { callee, args, .. } => {
                self.expression(callee, reached)?;
                self.sequence(args, reached && self.reaches_next(callee))?;
            }
            Expr::Array(values) | Expr::Tuple(values) => self.sequence(values, reached)?,
            Expr::Map(entries) => {
                let mut reached = reached;
                for (key, value) in entries {
                    self.expression(key, reached)?;
                    reached &= self.reaches_next(key);
                    self.expression(value, reached)?;
                    reached &= self.reaches_next(value);
                }
            }
            Expr::StructInit { fields, .. } => {
                let mut reached = reached;
                for (_, value) in fields {
                    self.expression(value, reached)?;
                    reached &= self.reaches_next(value);
                }
            }
            Expr::Index { object, index } => {
                // Evaluate known indexing as a unit so `$` failures precede
                // later arithmetic failures in the index expression.
                if reached {
                    self.value(expression)?;
                }
                self.expression(object, reached)?;
                self.expression(index, reached && self.reaches_next(object))?;
            }
            Expr::Member { object, .. }
            | Expr::Try(object)
            | Expr::Await(object)
            | Expr::Async(object)
            | Expr::Embed { path: object, .. } => {
                self.expression(object, reached)?;
            }
            Expr::Lambda(function) => self.block(&function.body, true)?,
            Expr::If { subject, arms } => self.branches(subject.as_deref(), arms, reached)?,
            Expr::Else { value, fallback }
            | Expr::Catch {
                value,
                body: fallback,
                ..
            } => {
                self.expression(value, reached)?;
                self.block(fallback, false)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn check_right_operand(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostics> {
        if !matches!(
            op,
            BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr
        ) {
            return Ok(());
        }
        let Some(Type::Named(name, _)) = self.checked.expression_types.get(&left.id()) else {
            return Ok(());
        };
        let width = match name.as_str() {
            "byte" => 8,
            "int" | "uint" => 64,
            _ => return Ok(()),
        };
        // Invalid divisors/counts fail for any left value that falls through.
        // Decode only the evaluated right operand; never evaluate the left here.
        let Some(value) = self.value(right)?.as_ref().and_then(integer_value) else {
            return Ok(());
        };
        let message = match op {
            BinaryOp::Div if value == 0 => "integer division by zero".to_owned(),
            BinaryOp::Mod if value == 0 => "integer remainder by zero".to_owned(),
            BinaryOp::Shl | BinaryOp::Shr if !(0..width).contains(&value) => {
                format!("invalid shift count {value}; expected 0..{width}")
            }
            _ => return Ok(()),
        };
        Err(Diagnostics::one(
            format!("constant evaluation failed: {message}"),
            0..0,
        ))
    }

    fn sequence(&mut self, values: &[Expr], mut reached: bool) -> Result<(), Diagnostics> {
        for value in values {
            self.expression(value, reached)?;
            reached &= self.reaches_next(value);
        }
        Ok(())
    }

    fn branches(
        &mut self,
        subject: Option<&Expr>,
        arms: &[(Vec<Pattern>, Block)],
        reached: bool,
    ) -> Result<(), Diagnostics> {
        if let Some(subject) = subject {
            self.expression(subject, reached)?;
        }
        let subjectless = subject.is_none();
        let subject_value = subject.and_then(|subject| self.boolean(subject));
        let mut available = reached;
        for (patterns, body) in arms {
            let mut matches = Some(false);
            for pattern in patterns {
                let alternative = match pattern {
                    Pattern::Wildcard => Some(true),
                    Pattern::Literal(value) => {
                        self.expression(value, available)?;
                        self.boolean(value).and_then(|value| {
                            if subjectless {
                                Some(value)
                            } else {
                                subject_value.map(|subject| subject == value)
                            }
                        })
                    }
                    _ => None,
                };
                matches = match (matches, alternative) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                };
            }
            self.block(body, available && matches == Some(true))?;
            if matches != Some(false) {
                available = false;
            }
        }
        Ok(())
    }

    fn power_warning(&mut self, expression: &Expr, left: &Expr, right: &Expr) {
        if self
            .value(left)
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(is_zero)
            && self
                .value(right)
                .ok()
                .flatten()
                .as_ref()
                .is_some_and(is_zero)
        {
            let mut warning = Diagnostics::one(
                "0 ** 0: zero raised to the power zero is defined to be 1",
                0..0,
            );
            if let Some(location) = expression.location() {
                warning = warning.at_source(&location.path, location.span.clone());
            }
            self.warnings.0.extend(warning.0);
        }
    }
}

fn integer_value(expression: &Expr) -> Option<i128> {
    match expression.unlocated() {
        Expr::Int(value) => crate::lexer::integer(value).ok().map(i128::from),
        Expr::Unary {
            op: UnaryOp::Neg,
            value,
        } => integer_value(value)?.checked_neg(),
        Expr::Cast { value, .. } => integer_value(value),
        _ => None,
    }
}

fn is_zero(expression: &Expr) -> bool {
    match expression.unlocated() {
        Expr::Int(value) => crate::lexer::integer(value).ok() == Some(0),
        Expr::Float(value) => value.parse::<f64>().ok() == Some(0.0),
        Expr::Cast { value, .. } => is_zero(value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn indexing_failures_are_diagnosed_in_both_modes() {
        for (source, message) in [
            ("int[] a = [];_ = a[$]", "out of bounds"),
            ("str a = \"\";_ = a[$]", "out of bounds"),
            ("int[] a = [1];_ = a[$-1]", "out of bounds"),
            ("str a = \"a\";_ = a[$-1]", "out of bounds"),
            ("int[] a = [1, 2];_ = a[$-1-1]", "out of bounds"),
            ("int[] a = [1];_ = a[2]", "out of bounds"),
            ("str a = \"a\";_ = a[2]", "out of bounds"),
            ("int[] a = [1];_ = a[-1]", "out of bounds"),
            ("str a = \"a\";_ = a[-1]", "out of bounds"),
            ("int[] a = [];_ = a[$-(1u/0u)]", "out of bounds"),
            ("[str]int a = [\"a\": 1];_ = a[\"b\"]", "map key not found"),
        ] {
            for release in [false, true] {
                let error =
                    crate::compile_source_with_options(source, Path::new("index.nc"), release)
                        .unwrap_err();
                assert!(error.to_string().contains(message), "{error}\n{source}");
                assert!(!error.to_string().contains("integer overflow"), "{error}");
                assert_eq!(error.0[0].path.as_deref(), Some(Path::new("index.nc")));
                assert!(!error.0[0].span.is_empty());
            }
        }
    }

    #[test]
    fn valid_index_offsets_and_map_insertions_compile() {
        for source in [
            "int[] a = [1, 2, 3];@println(a[$], a[$-1], a[$-1-1])",
            "str a = \"abc\";@println(a[$], a[$-1], a[$-1-1])",
            "int[][] a = [[1, 2], [3, 4]];@println(a[$][a[0][$]-1])",
            "mut [str]int a = [\"a\": 1];a[\"b\"] = 2;@println(a[\"b\"])",
            "mut int[] a = [1, 2];a[$-1] = 3;@println(a)",
            "mut str a = \"ab\";a[$-1] = 'c';@println(a)",
        ] {
            for release in [false, true] {
                crate::compile_source_with_options(source, Path::new("index.nc"), release)
                    .unwrap_or_else(|error| panic!("{error}\n{source}"));
            }
        }
    }

    #[test]
    fn ordinary_index_arithmetic_retains_overflow_diagnostic() {
        for source in [
            "uint a = 0;_ = a - 1",
            "int[] a = [1];_ = a[0u-1u]",
            "int[] a = [1, 2];_ = a[$-(0u-1u)]",
            "int[] a = [1, 2];_ = a[0u-$]",
        ] {
            for release in [false, true] {
                let error =
                    crate::compile_source_with_options(source, Path::new("index.nc"), release)
                        .unwrap_err();
                assert!(
                    error.to_string().contains("integer overflow"),
                    "{error}\n{source}"
                );
            }
        }
    }

    #[test]
    fn release_indexed_writes_report_invalid_storage() {
        for source in [
            "mut int[] a = [1];a[2] = 3",
            "mut str a = \"a\";a[2] = 'b'",
            "mut int[] a = [];a[$] = 3",
            "mut str a = \"\";a[$] = 'b'",
            "mut int[] a = [1];a[$-1] = 3",
            "mut [str]int[] a = [\"a\": [1]];a[\"b\"][0] = 3",
        ] {
            let error = crate::compile_source_with_options(source, Path::new("index.nc"), true)
                .unwrap_err();
            assert!(
                error.to_string().contains("out of bounds")
                    || error.to_string().contains("map key not found"),
                "{error}\n{source}"
            );
        }
    }

    #[test]
    fn indexing_preserves_unreached_paths_and_rhs_first_failures() {
        for release in [false, true] {
            for source in [
                "int[] a = [];_ = false and (a[$] == 0)",
                "str a = \"\";_ = true or (a[$] == 'a')",
                "[str]int a = [\"a\": 1];_ = false and (a[\"b\"] == 0)",
            ] {
                crate::compile_source_with_options(source, Path::new("index.nc"), release)
                    .unwrap_or_else(|error| panic!("{error}\n{source}"));
            }
            let error = crate::compile_source_with_options(
                "mut int[] a = [];a[$] = 1 / 0",
                Path::new("index.nc"),
                release,
            )
            .unwrap_err();
            assert!(error.to_string().contains("division by zero"), "{error}");
            assert!(!error.to_string().contains("out of bounds"), "{error}");
        }
    }

    #[test]
    fn runtime_index_offsets_emit_bounds_checks() {
        let source = "fn read(int[] a, uint n) int {return a[$-n]}";
        let c = crate::compile_source(source, Path::new("index.nc")).unwrap();
        assert!(c.contains("== 0) nc_panic(\"index out of bounds\")"));
        assert!(c.contains("nc_panic(\"index out of bounds\")"));
        // Ordinary subtraction still uses the checked arithmetic operation.
        assert!(c.contains("__builtin_sub_overflow"));
    }

    #[test]
    fn immutable_chains_tuples_and_captures_report_arithmetic_failures() {
        for source in [
            "int first = 2;int second = first - 2;_ = 1 / second",
            "(int, int) pair = (1, 0);int left, int right = pair;_ = left % right",
            "int zero = 0;fn captured = fn() int {return 1 / zero};_ = captured()",
            "_ = 9223372036854775807 + 1",
            "_ = @as(uint, -0.5)",
        ] {
            for release in [false, true] {
                let error =
                    crate::compile_source_with_options(source, Path::new("numeric.nc"), release)
                        .unwrap_err();
                assert!(
                    error.to_string().contains("constant evaluation failed"),
                    "{error}\n{source}"
                );
                assert_eq!(error.0[0].path.as_deref(), Some(Path::new("numeric.nc")));
                assert!(!error.0[0].span.is_empty());
            }
        }
    }

    #[test]
    fn known_invalid_right_operands_reject_unknown_left_values() {
        for (source, expression, detail) in [
            (
                "fn divide(int value) int {return value / 0}",
                "value / 0",
                "division by zero",
            ),
            (
                "fn remainder(int value) int {return value % 0}",
                "value % 0",
                "remainder by zero",
            ),
            (
                "_ = @args().len / 0u",
                "@args().len / 0u",
                "division by zero",
            ),
            (
                "_ = @args().len % 0u",
                "@args().len % 0u",
                "remainder by zero",
            ),
            (
                "int first = 2;int zero = first - 2;fn effect() int {@println(\"effect\");return 1};_ = effect() / zero",
                "effect() / zero",
                "division by zero",
            ),
            (
                "fn shift(int value) int {return value << -1}",
                "value << -1",
                "shift count",
            ),
            (
                "fn shift(int value) int {return value >> -9223372036854775808}",
                "value >> -9223372036854775808",
                "shift count",
            ),
            (
                "fn shift(int value) int {return value << 64}",
                "value << 64",
                "shift count",
            ),
            (
                "_ = @args().len >> 18446744073709551615u",
                "@args().len >> 18446744073709551615u",
                "shift count",
            ),
            (
                "fn shift(byte value) byte {return value << @as(byte, 8)}",
                "value << @as(byte, 8)",
                "shift count",
            ),
            (
                "fn divide(byte value) byte {return value / @as(byte, 0)}",
                "value / @as(byte, 0)",
                "division by zero",
            ),
        ] {
            for release in [false, true] {
                let path = Path::new("numeric.nc");
                let error = crate::compile_source_with_options(source, path, release).unwrap_err();
                assert!(
                    error.to_string().contains(detail),
                    "release={release}: {error}\n{source}"
                );
                assert_eq!(error.0[0].path.as_deref(), Some(path));
                assert_eq!(&source[error.0[0].span.clone()], expression);
            }
        }
    }

    #[test]
    fn right_operand_checks_preserve_float_division_and_valid_shifts() {
        for source in [
            "fn divide(float value) float {return value / 0.0}",
            "_ = @as(float, @args().len) / 0.0",
            "fn shift(int value) int {return value >> 63}",
            "fn shift(byte value) byte {return value << @as(byte, 7)}",
            "_ = @args().len >> 63u",
            "_ = false and (@args().len / 0u == 0u)",
            "_ = true or (@args().len << 64u == 0u)",
            "mut bool unknown = @args().len == 1;_ = unknown and (@args().len % 0u == 0u)",
            "mut bool unknown = @args().len == 1;_ = unknown or (@args().len >> 64u == 0u)",
        ] {
            for release in [false, true] {
                crate::compile_source_with_options(source, Path::new("numeric.nc"), release)
                    .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
            }
        }
    }

    #[test]
    fn short_circuit_and_unreachable_paths_do_not_report_failures() {
        for source in [
            "_ = false and (1 / 0 == 0);_ = true or (1 % 0 == 0)",
            "bool stop = false;bool alias = stop;_ = alias and (1 << 64 == 0)",
            "mut bool unknown = @args().len == 1;_ = unknown and (1 / 0 == 0)",
            "_ = if true {true -> {42} false -> {1 / 0}}",
            "_ = if @args().len == 1 {true -> {1 / 0} false -> {42}}",
            "fn runtime(bool fail) int {return if fail {true -> {1 / 0} false -> {9}}};_ = runtime(false)",
            "while false {_ = 1 / 0}",
            "while true {};_ = 1 / 0",
        ] {
            for release in [false, true] {
                crate::compile_source_with_options(source, Path::new("numeric.nc"), release)
                    .unwrap_or_else(|error| panic!("release={release}: {error}\n{source}"));
            }
        }
    }

    #[test]
    fn transitive_calls_and_loops_are_not_evaluated_by_the_static_pass() {
        for source in [
            "fn zero() int {@println(\"effect\");return 0};int first = zero();int second = first;_ = 1 / second",
            "fn forever(bool spin) int {while spin {};return 0};int first = forever(true);int second = first;_ = second ** second",
            "mut int zero = 0;_ = 1 / zero",
        ] {
            let path = Path::new("numeric.nc");
            let module = crate::parser::parse_at(crate::lexer::lex(source).unwrap(), path).unwrap();
            let checked = crate::sema::check(module, path).unwrap();
            assert!(super::check(&checked).unwrap().0.is_empty(), "{source}");
        }
    }

    #[test]
    fn normal_compilation_ignores_test_arithmetic_and_warnings() {
        let source = "test \"ignored\" {_ = 0 ** 0;_ = 1 / 0}";
        for release in [false, true] {
            let path = Path::new("numeric.nc");
            assert_eq!(
                crate::compile_source_with_diagnostics(source, path, release)
                    .unwrap()
                    .warnings
                    .0,
                []
            );
            assert!(crate::compile_test_source_with_options(source, path, release).is_err());
        }
    }

    #[test]
    fn imported_generic_arithmetic_keeps_original_locations() {
        let directory = crate::temp::Directory::new().unwrap();
        let root = directory.path().join("main.nc");
        let imported = directory.path().join("library.nc");
        for (expression, warning) in [("1 / 0", false), ("0 ** 0", true)] {
            let library =
                format!("// header\npub fn numeric<T>(T ignored) int {{return {expression}}}\n");
            std::fs::write(&imported, &library).unwrap();
            for release in [false, true] {
                let result = crate::compile_source_with_diagnostics(
                    "import {\"library\" as lib};_ = lib.numeric<int>(1)",
                    &root,
                    release,
                );
                let diagnostics = if warning {
                    result.unwrap().warnings
                } else {
                    result.err().unwrap()
                };
                assert_eq!(diagnostics.0[0].path.as_deref(), Some(imported.as_path()));
                assert_eq!(&library[diagnostics.0[0].span.clone()], expression);
            }
        }
    }
}
