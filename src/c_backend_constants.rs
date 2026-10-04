//! Read-only literal templates are never exposed as NC storage. Materialization
//! recursively allocates writable containers on every evaluation of a literal.
use super::{
    BinaryOp, Diagnostics, Emitter, Expr, Stmt, Type, TypeInfo, UnaryOp, ValueOperation, c_string,
    integer,
};

impl Emitter<'_> {
    pub(super) fn constant_expression(&self, expression: &Expr) -> Result<bool, Diagnostics> {
        let constant = match expression.unlocated() {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::None
            | Expr::Bytes(_) => true,
            Expr::Array(values) | Expr::Tuple(values) => values
                .iter()
                .map(|value| self.constant_expression(value))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|value| value),
            Expr::Map(entries) => {
                let mut constant = true;
                for (key, value) in entries {
                    constant &=
                        self.constant_expression(key)? && self.constant_expression(value)?;
                }
                constant
            }
            Expr::StructInit { fields, .. } => {
                let mut constant = true;
                for (_, value) in fields {
                    constant &= self.constant_expression(value)?;
                }
                constant
            }
            Expr::Unary {
                op: UnaryOp::Neg,
                value,
            } => {
                matches!(value.unlocated(), Expr::Int(_) | Expr::Float(_))
                    && matches!(self.ty(expression)?, Type::Named(name, _) if matches!(name.as_str(), "int" | "float"))
            }
            Expr::Cast { ty, value, .. } => {
                let from = self.ty(value)?;
                (self.constant_representation_cast(&from, ty)
                    || matches!((ty, value.unlocated()), (Type::Named(name, _), Expr::Int(text)) if name == "byte" && integer(text).is_ok_and(|n| n <= 255)))
                    && self.constant_expression(value)?
            }
            Expr::Binary {
                op: BinaryOp::Concat,
                ..
            } => constant_string_parts(expression).is_some(),
            Expr::Member { object, name } => self
                .constant_variant(object, name)
                .is_some_and(|(_, types)| types.is_empty()),
            Expr::Call { callee, args, .. } => {
                if let Some(statement) = constant_thunk(callee, args) {
                    match statement {
                        Stmt::Return(Some(value)) | Stmt::Throw(value) => {
                            self.constant_expression(value)?
                        }
                        Stmt::Return(None) => true,
                        _ => false,
                    }
                } else if let Expr::Member { object, name } = callee.unlocated() {
                    if self.constant_variant(object, name).is_some() {
                        let mut constant = true;
                        for arg in args {
                            constant &= self.constant_expression(arg)?;
                        }
                        constant
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        Ok(constant)
    }

    fn constant_representation_cast(&self, from: &Type, to: &Type) -> bool {
        if from == to {
            return true;
        }
        if let Type::Named(name, _) = to
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(name)
        {
            return self.constant_representation_cast(from, base);
        }
        if let Type::Named(name, _) = from
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(name)
        {
            return self.constant_representation_cast(base, to);
        }
        matches!((from, to), (Type::Array(left, Some(_)), Type::Array(right, None)) if left == right)
    }

    fn constant_representation_type(&self, ty: &Type) -> Type {
        match ty {
            Type::Named(name, _) => {
                if let Some(TypeInfo::Alias(base)) = self.checked.types.get(name) {
                    self.constant_representation_type(base)
                } else {
                    ty.clone()
                }
            }
            // Fixed lengths constrain NC values, not their C container layout.
            Type::Array(element, _) => {
                Type::Array(Box::new(self.constant_representation_type(element)), None)
            }
            Type::Map(key, value) => Type::Map(
                Box::new(self.constant_representation_type(key)),
                Box::new(self.constant_representation_type(value)),
            ),
            Type::Tuple(types) => Type::Tuple(
                types
                    .iter()
                    .map(|ty| self.constant_representation_type(ty))
                    .collect(),
            ),
            Type::Function(params, result) => Type::Function(
                params
                    .iter()
                    .map(|ty| self.constant_representation_type(ty))
                    .collect(),
                Box::new(self.constant_representation_type(result)),
            ),
            Type::Optional(inner) => {
                Type::Optional(Box::new(self.constant_representation_type(inner)))
            }
            Type::ErrorUnion(inner) => {
                Type::ErrorUnion(Box::new(self.constant_representation_type(inner)))
            }
            Type::Future(inner) => Type::Future(Box::new(self.constant_representation_type(inner))),
        }
    }

    pub(super) fn constant_object(&mut self, ct: &str, initializer: &str) -> String {
        let key = (ct.to_owned(), initializer.to_owned());
        if let Some(name) = self.constant_names.get(&key) {
            return name.clone();
        }
        let name = format!("nc_constant_{}", self.constant_names.len());
        let declaration = if let Some(element) = ct.strip_suffix("[]") {
            format!("static const {element} {name}[] = {initializer};")
        } else {
            format!("static const {ct} {name} = {initializer};")
        };
        self.constant_data.push(declaration);
        self.constant_names.insert(key, name.clone());
        name
    }

    pub(super) fn constant_initializer(
        &mut self,
        expression: &Expr,
        expected: &Type,
    ) -> Result<String, Diagnostics> {
        let expression = expression.unlocated();
        if let Type::Named(name, _) = expected
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(name)
        {
            return self.constant_initializer(expression, &base.clone());
        }
        if matches!(expression, Expr::None) {
            return Ok("{0}".into());
        }
        if let Type::Optional(inner) | Type::ErrorUnion(inner) = expected
            && self.constant_representation_type(&self.ty(expression)?)
                != self.constant_representation_type(expected)
        {
            let value = self.constant_initializer(expression, inner)?;
            let flag = if matches!(expected, Type::Optional(_)) {
                ".present = 1, "
            } else {
                ""
            };
            return Ok(format!("{{{flag}.value = {value}}}"));
        }
        match expression {
            Expr::Cast { value, .. } => self.constant_initializer(value, expected),
            Expr::Binary {
                op: BinaryOp::Concat,
                ..
            } => self.constant_string_initializer(expression),
            Expr::Member { object, name } => self.constant_enum_initializer(object, name, &[]),
            Expr::Call { callee, args, .. } => {
                self.constant_call_initializer(callee, args, expected)
            }
            Expr::Array(values) => self.constant_array_initializer(values, expected),
            Expr::Map(entries) => self.constant_map_initializer(entries, expected),
            Expr::Bytes(bytes) => {
                let values = bytes.iter().map(ToString::to_string).collect::<Vec<_>>();
                self.constant_sequence(expected, &values)
            }
            Expr::Tuple(values) => {
                let Type::Tuple(types) = expected else {
                    unreachable!()
                };
                let mut fields = Vec::new();
                for (index, (value, ty)) in values.iter().zip(types).enumerate() {
                    fields.push(format!(
                        ".f_{index} = {}",
                        self.constant_initializer(value, ty)?
                    ));
                }
                Ok(format!("{{{}}}", fields.join(", ")))
            }
            Expr::StructInit { fields, .. } => self.constant_record_initializer(fields, expected),
            Expr::String(value) | Expr::Char(value) => {
                self.c_type(expected)?;
                Ok(format!("{{{}, {}, 0, NULL}}", value.len(), c_string(value)))
            }
            Expr::Unary { value, .. } => {
                if matches!(value.unlocated(), Expr::Int(text) if integer(text).ok() == Some(1u64 << 63))
                {
                    return Ok("(-9223372036854775807LL - 1LL)".into());
                }
                Ok(format!("(-{})", self.atom_value(value.unlocated())?))
            }
            _ => self.atom_value(expression),
        }
    }

    fn constant_record_initializer(
        &mut self,
        fields: &[(String, Expr)],
        expected: &Type,
    ) -> Result<String, Diagnostics> {
        let types = self.fields(expected).expect("checked struct layout");
        let mut initializers = Vec::new();
        for (name, value) in fields {
            let field = format!("f_{name}");
            let (_, ty) = types.iter().find(|(name, _)| *name == field).unwrap();
            initializers.push(format!(
                ".{field} = {}",
                self.constant_initializer(value, ty)?
            ));
        }
        Ok(format!("{{{}}}", initializers.join(", ")))
    }

    fn constant_array_initializer(
        &mut self,
        values: &[Expr],
        expected: &Type,
    ) -> Result<String, Diagnostics> {
        if values.is_empty() {
            return Ok("{0}".into());
        }
        let Type::Array(element, _) = expected else {
            unreachable!()
        };
        let mut initializers = Vec::new();
        for value in values {
            initializers.push(self.constant_initializer(value, element)?);
        }
        self.constant_sequence(expected, &initializers)
    }

    fn constant_map_initializer(
        &mut self,
        entries: &[(Expr, Expr)],
        expected: &Type,
    ) -> Result<String, Diagnostics> {
        let Type::Map(key, value) = expected else {
            unreachable!()
        };
        let mut initializers = Vec::new();
        for (k, v) in entries {
            let k = self.constant_initializer(k, key)?;
            let v = self.constant_initializer(v, value)?;
            initializers.push(format!("{{{k}, {v}}}"));
        }
        self.constant_sequence(&super::map_array(key, value), &initializers)
    }

    fn constant_sequence(&mut self, ty: &Type, values: &[String]) -> Result<String, Diagnostics> {
        if values.is_empty() {
            return Ok("{0}".into());
        }
        let Type::Array(element, _) = ty else {
            unreachable!()
        };
        let ct = self.c_type(element)?;
        let data = self.constant_object(&format!("{ct}[]"), &format!("{{{}}}", values.join(", ")));
        // The existing container ABI has a writable pointer. Only copy helpers
        // read this cast pointer; no template ever becomes an NC binding.
        Ok(format!("{{{0}, {0}, ({ct} *){data}}}", values.len()))
    }

    fn constant_variant(&self, object: &Expr, name: &str) -> Option<(usize, Vec<Type>)> {
        if !matches!(object.unlocated(), Expr::Name(_)) {
            return None;
        }
        let ty = self.ty(object).ok()?;
        let declaration = self.enum_decl(&ty)?;
        let tag = declaration
            .variants
            .iter()
            .position(|variant| variant.name == name)?;
        Some((tag, declaration.variants[tag].values.clone()))
    }

    fn constant_enum_initializer(
        &mut self,
        object: &Expr,
        name: &str,
        args: &[Expr],
    ) -> Result<String, Diagnostics> {
        let (tag, types) = self
            .constant_variant(object, name)
            .expect("constant enum variant");
        if types.is_empty() {
            return Ok(format!("{{{tag}, 0}}"));
        }
        let mut values = Vec::new();
        for (arg, ty) in args.iter().zip(&types) {
            values.push(self.constant_initializer(arg, ty)?);
        }
        let ct = self.c_type(&Type::Tuple(types))?;
        let data = self.constant_object(&ct, &format!("{{{}}}", values.join(", ")));
        Ok(format!("{{{tag}, (void *)&{data}}}"))
    }

    fn constant_call_initializer(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        expected: &Type,
    ) -> Result<String, Diagnostics> {
        if let Some(statement) = constant_thunk(callee, args) {
            return match statement {
                Stmt::Return(Some(value)) => self.constant_initializer(value, expected),
                Stmt::Return(None) => Ok(if self.constant_representation_type(expected)
                    == Type::void()
                {
                    "0"
                } else {
                    "{0}"
                }
                .into()),
                Stmt::Throw(value) => Ok(format!(
                    "{{.failed = 1, .error = {}}}",
                    self.constant_initializer(value, &Type::Named("str".into(), vec![]))?
                )),
                _ => unreachable!(),
            };
        }
        let Expr::Member { object, name } = callee.unlocated() else {
            unreachable!()
        };
        self.constant_enum_initializer(object, name, args)
    }

    fn constant_string_initializer(&mut self, expression: &Expr) -> Result<String, Diagnostics> {
        let parts = constant_string_parts(expression).expect("constant string concatenation");
        let mut text = String::new();
        let mut ends = Vec::new();
        for part in parts {
            let boundaries = crate::unicode::boundaries(part);
            for end in boundaries
                .iter()
                .skip(1)
                .copied()
                .chain((!part.is_empty()).then_some(part.len()))
            {
                ends.push((text.len() + end).to_string());
            }
            text.push_str(part);
        }
        self.c_type(&Type::Named("str".into(), vec![]))?;
        if ends.is_empty() {
            return Ok(format!("{{0, {}, 0, NULL}}", c_string(&text)));
        }
        let data = self.constant_object("size_t[]", &format!("{{{}}}", ends.join(", ")));
        Ok(format!(
            "{{{}, {}, {}, {data}}}",
            text.len(),
            c_string(&text),
            ends.len()
        ))
    }

    pub(super) fn constant_enum_copy(
        &mut self,
        ty: &Type,
        value: &str,
    ) -> Result<String, Diagnostics> {
        let declaration = self.enum_decl(ty).expect("enum layout").clone();
        let ct = self.c_type(ty)?;
        let result = self.fresh();
        self.line(format!("{ct} {result} = {value};"));
        self.line(format!("switch (({value}).tag) {{"));
        for (tag, variant) in declaration.variants.iter().enumerate() {
            if variant.values.is_empty() {
                continue;
            }
            self.line(format!("case {tag}: {{"));
            let ty = Type::Tuple(variant.values.clone());
            let ct = self.c_type(&ty)?;
            let contents = self.copy_with(
                ValueOperation::ConstantCopy,
                &ty,
                &format!("*({ct} *)({value}).payload"),
            )?;
            self.allocation_support();
            let pointer = self.fresh();
            self.line(format!("{ct} *{pointer} = nc_alloc(1, sizeof({ct})); *{pointer} = {contents}; {result}.payload = {pointer}; break; }}"));
        }
        self.line("}");
        Ok(result)
    }

    pub(super) fn constant_map_copy(
        &mut self,
        key: &Type,
        inner: &Type,
        value: &str,
    ) -> Result<String, Diagnostics> {
        let ct = self.c_type(&Type::Map(Box::new(key.clone()), Box::new(inner.clone())))?;
        let result = self.fresh();
        let index = self.fresh();
        self.allocation_support();
        let entry = self.c_type(&Type::Tuple(vec![key.clone(), inner.clone()]))?;
        self.line(format!(
            "{ct} {result} = {{0, ({value}).len, nc_alloc(({value}).len, sizeof({entry}))}};"
        ));
        self.line(format!(
            "for (uint64_t {index} = 0; {index} < ({value}).len; ++{index}) {{"
        ));
        let key_value = self.copy_with(
            ValueOperation::ConstantCopy,
            key,
            &format!("({value}).vals[{index}].f_0"),
        )?;
        let inner_value = self.copy_with(
            ValueOperation::ConstantCopy,
            inner,
            &format!("({value}).vals[{index}].f_1"),
        )?;
        let found = self.map_find(&result, &key_value, key)?;
        self.line(format!("if ({found} == {result}.len) ++{result}.len;"));
        self.line(format!(
            "{result}.vals[{found}].f_0 = {key_value}; {result}.vals[{found}].f_1 = {inner_value};"
        ));
        self.line("}");
        Ok(result)
    }
}

fn constant_thunk<'a>(callee: &'a Expr, args: &[Expr]) -> Option<&'a Stmt> {
    if !args.is_empty() {
        return None;
    }
    let Expr::Lambda(function) = callee.unlocated() else {
        return None;
    };
    if !function.params.is_empty() {
        return None;
    }
    let [statement] = function.body.statements.as_slice() else {
        return None;
    };
    Some(statement.unlocated())
}

fn constant_string_parts(expression: &Expr) -> Option<Vec<&str>> {
    match expression.unlocated() {
        Expr::String(text) => Some(vec![text]),
        Expr::Binary {
            left,
            op: BinaryOp::Concat,
            right,
        } => {
            let mut parts = constant_string_parts(left)?;
            parts.extend(constant_string_parts(right)?);
            Some(parts)
        }
        _ => None,
    }
}
