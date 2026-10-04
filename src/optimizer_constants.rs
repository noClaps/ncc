//! Reconstruct constants with constituent types, not just outer context.
use super::{
    CheckedModule, Expr, Stmt, Type, TypeInfo, Value, constant_statement, constant_thunk,
    string_expr,
};

pub(super) fn expression(value: Value, ty: &Type, checked: &CheckedModule) -> Expr {
    if let Type::Named(name, _) = ty
        && let Some(TypeInfo::Alias(base)) = checked.types.get(name)
    {
        return Expr::Cast {
            implicit: false,
            ty: ty.clone(),
            value: Box::new(expression(value, base, checked)),
        };
    }
    let value = match (value, ty) {
        (Value::Void(_), _) => return constant_statement(Stmt::Return(None), Type::void()),
        (Value::Array(values), Type::Array(inner, _)) => Expr::Array(
            values
                .into_iter()
                .map(|v| expression(v, inner, checked))
                .collect(),
        ),
        (Value::Tuple(values), Type::Tuple(types)) => Expr::Tuple(
            values
                .into_iter()
                .zip(types)
                .map(|(v, t)| expression(v, t, checked))
                .collect(),
        ),
        (Value::Map(entries), Type::Map(key, inner)) => Expr::Map(
            entries
                .into_iter()
                .map(|(k, v)| (expression(k, key, checked), expression(v, inner, checked)))
                .collect(),
        ),
        (Value::Struct(name, fields), _) => record(name, fields, checked),
        (Value::Enum(name, variant, values), _) => enumeration(name, variant, values, checked),
        (Value::Optional(_, None), Type::Optional(_)) => Expr::None,
        (Value::Optional(_, Some(value)), Type::Optional(inner))
        | (Value::Success(_, value), Type::ErrorUnion(inner)) => {
            if matches!(ty, Type::ErrorUnion(_))
                && matches!(*value, Value::Void(_))
                && **inner == Type::void()
            {
                return constant_statement(Stmt::Return(None), ty.clone());
            }
            expression(*value, inner, checked)
        }
        (Value::Failure(_, message), Type::ErrorUnion(_)) => {
            return constant_statement(Stmt::Throw(string_expr(message)), ty.clone());
        }
        (value, _) => value.expr(),
    };
    if matches!(ty, Type::Named(name, _) if matches!(name.as_str(), "int" | "uint" | "byte" | "float" | "str" | "char" | "bool"))
        || matches!(ty, Type::Function(..))
    {
        value
    } else {
        constant_thunk(value, ty.clone())
    }
}

fn record(name: String, fields: Vec<(String, Value)>, checked: &CheckedModule) -> Expr {
    let Some(TypeInfo::Struct(declaration)) = checked.types.get(&name) else {
        unreachable!("checked constant record")
    };
    Expr::StructInit {
        name,
        fields: fields
            .into_iter()
            .map(|(name, value)| {
                let field = declaration
                    .fields
                    .iter()
                    .find(|field| field.name == name)
                    .expect("checked constant field");
                (name, expression(value, &field.ty, checked))
            })
            .collect(),
    }
}

fn enumeration(name: String, variant: String, values: Vec<Value>, checked: &CheckedModule) -> Expr {
    let Some(TypeInfo::Enum(declaration)) = checked.types.get(&name) else {
        unreachable!("checked constant enum")
    };
    let types = &declaration
        .variants
        .iter()
        .find(|v| v.name == variant)
        .expect("checked constant variant")
        .values;
    let member = Expr::Member {
        object: Box::new(Expr::Name(name)),
        name: variant,
    };
    if values.is_empty() {
        member
    } else {
        Expr::Call {
            callee: Box::new(member),
            args: values
                .into_iter()
                .zip(types)
                .map(|(v, t)| expression(v, t, checked))
                .collect(),
            generics: vec![],
        }
    }
}
