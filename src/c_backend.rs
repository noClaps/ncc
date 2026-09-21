//! C emission uses the types recorded during semantic checking. Expressions
//! are evaluated into temporaries to preserve NC's left-to-right evaluation.
use crate::{
    ast::*,
    diagnostic::Diagnostics,
    sema::{CheckedModule, TypeInfo, integer},
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn emit(checked: &CheckedModule) -> Result<String, Diagnostics> {
    let mut e = Emitter {
        checked,
        headers: BTreeSet::new(),
        helpers: BTreeSet::new(),
        out: String::new(),
        scopes: vec![HashMap::new()],
        next: 0,
        loops: vec![],
        array_types: vec![],
        index_context: vec![],
        record_types: vec![],
        value_targets: vec![],
        return_type: Type::void(),
        enum_equalities: HashSet::new(),
        enum_strings: HashSet::new(),
        runtime_prototypes: vec![],
        runtime_functions: vec![],
        function_types: vec![],
        type_definitions: vec![],
        mutexes: HashSet::new(),
        mutex_types: vec![],
        value_helpers: HashMap::new(),
    };
    let mut declarations = String::new();
    let mut global_slots = HashMap::new();
    for item in &checked.module.items {
        match item {
            Item::Function(f) => {
                if !f.generics.is_empty() || f.throws {
                    return unsupported("generic or throwing functions");
                }
                let ret = e.c_type(&f.return_type)?;
                let params = f
                    .params
                    .iter()
                    .map(|p| e.c_type(&p.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                declarations.push_str(&format!(
                    "{ret} nc_fn_{}({});\n",
                    f.name,
                    if params.is_empty() {
                        "void".into()
                    } else {
                        params.join(", ")
                    }
                ));
            }
            Item::Global(v) => {
                let ty = if v.mutex {
                    format!("{} *", e.mutex_type(&v.ty)?)
                } else {
                    e.c_type(&v.ty)?
                };
                let name = e.bind(pattern_name(&v.pattern)?);
                if v.mutex {
                    e.mutexes.insert(name.clone());
                }
                global_slots.insert(v as *const VarDecl as usize, name.clone());
                declarations.push_str(&format!("static {ty} {name};\n"));
            }
            Item::Import { .. } => return unsupported("unresolved import"),
            Item::Extern {
                path, functions, ..
            } => {
                if !path.ends_with(".c") {
                    return unsupported("non-C external implementations");
                }
                for f in functions {
                    if !f.symbol.chars().enumerate().all(|(i, c)| {
                        c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                    }) || f.symbol.is_empty()
                    {
                        return Err(Diagnostics::one(
                            "external symbol must be a C identifier",
                            0..0,
                        ));
                    }
                    let ret = e.c_type(&f.return_type)?;
                    let params = f
                        .params
                        .iter()
                        .map(|p| e.c_type(&p.ty))
                        .collect::<Result<Vec<_>, _>>()?;
                    declarations.push_str(&format!(
                        "extern {ret} {}({});\n",
                        f.symbol,
                        if params.is_empty() {
                            "void".into()
                        } else {
                            params.join(", ")
                        }
                    ));
                }
                declarations.push_str(&format!("#include {}\n", c_string(path)));
            }
            Item::Struct(_) | Item::Enum(_) => {}
            Item::TypeAlias { .. } => {}
            _ => {}
        }
    }
    for item in &checked.module.items {
        if let Item::Function(f) = item {
            e.return_type = f.return_type.clone();
            e.scopes.push(HashMap::new());
            let ret = e.c_type(&f.return_type)?;
            let mut params = vec![];
            for p in &f.params {
                let ty = e.c_type(&p.ty)?;
                let name = e.bind(&p.name);
                params.push(format!("{ty} {name}"));
            }
            e.line(format!(
                "{ret} nc_fn_{}({}) {{",
                f.name,
                if params.is_empty() {
                    "void".into()
                } else {
                    params.join(", ")
                }
            ));
            e.block(&f.body)?;
            if f.return_type == Type::ErrorUnion(Box::new(Type::void())) {
                let ct = e.c_type(&f.return_type)?;
                e.line(format!("return ({ct}){{0}};"));
            }
            e.line("}");
            e.scopes.pop();
        }
    }
    e.scopes[0].clear();
    e.return_type = Type::void();
    e.line("int main(void) {");
    let main_body_start = e.out.len();
    for item in &checked.module.items {
        match item {
            Item::Global(v) => {
                let value = e.expr_as(&v.value, &v.ty)?;
                let value = e.copy(&v.ty, &value)?;
                let n = pattern_name(&v.pattern)?;
                let name = &global_slots[&(v as *const VarDecl as usize)];
                if v.mutex {
                    e.init_mutex(name, &v.ty, &value)?;
                } else {
                    e.line(format!("{name} = {value};"));
                }
                e.scopes[0].insert(n.into(), name.clone());
            }
            Item::Statement(s) => e.statement(s)?,
            Item::Test { body, .. } => {
                e.line("{");
                e.block(body)?;
                e.line("}");
            }
            _ => {}
        }
    }
    e.line("return 0;\n}");
    let mut output = String::from("/* Generated by ncc. */\n");
    for header in &e.headers {
        output.push_str(&format!("#include <{header}>\n"));
    }
    if e.helpers.contains("/* async runtime */") {
        output.push_str(include_str!("runtime_async.h"));
    }
    if e.helpers.contains("/* unicode runtime */") {
        output.push_str(&crate::unicode::c_tables());
        output.push_str(include_str!("runtime_unicode.h"));
    }
    for (_, name, _) in &e.record_types {
        output.push_str(&format!("typedef struct {name} {name};\n"));
    }
    let mut emitted = HashSet::new();
    while emitted.len() < e.type_definitions.len() {
        let previous = emitted.len();
        for definition in &e.type_definitions {
            if !emitted.contains(&definition.name)
                && definition.dependencies.iter().all(|name| {
                    emitted.contains(name)
                        || !e
                            .type_definitions
                            .iter()
                            .any(|definition| &definition.name == name)
                })
            {
                output.push_str(&definition.code);
                output.push('\n');
                emitted.insert(definition.name.clone());
            }
        }
        if previous == emitted.len() {
            return Err(Diagnostics::one("cyclic C type layout", 0..0));
        }
    }
    if e.headers.contains("pthread.h") {
        output.push_str("static pthread_mutex_t nc_allocation_lock = PTHREAD_MUTEX_INITIALIZER;\n");
    }
    for helper in &e.helpers {
        if e.headers.contains("pthread.h") {
            output.push_str(&helper.replace("node->next = nc_allocations; nc_allocations = node;", "pthread_mutex_lock(&nc_allocation_lock); node->next = nc_allocations; nc_allocations = node; pthread_mutex_unlock(&nc_allocation_lock);").replace("exit(1);", "_Exit(1);"));
        } else {
            output.push_str(helper);
        }
        output.push('\n');
    }
    output.push_str(&declarations);
    for prototype in &e.runtime_prototypes {
        output.push_str(prototype);
        output.push('\n');
    }
    for function in &e.runtime_functions {
        output.push_str(function);
        output.push('\n');
    }
    output.push_str(&e.out[..main_body_start]);
    if e.helpers
        .iter()
        .any(|helper| helper.contains("nc_allocations"))
    {
        output.push_str("atexit(nc_cleanup);\n");
    }
    if e.helpers.contains("/* async runtime */") {
        output.push_str("atexit(nc_async_cleanup);\n");
    }
    output.push_str(&e.out[main_body_start..]);
    Ok(output)
}
fn unsupported<T>(feature: &str) -> Result<T, Diagnostics> {
    Err(Diagnostics::one(
        format!("C backend does not yet implement {feature}"),
        0..0,
    ))
}
fn pattern_name(pattern: &Pattern) -> Result<&str, Diagnostics> {
    if let Pattern::Name(name) = pattern {
        Ok(name)
    } else {
        unsupported("destructuring")
    }
}

type RecordType = (Type, String, Vec<(String, String)>);
struct TypeDefinition {
    name: String,
    dependencies: Vec<String>,
    code: String,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ValueOperation {
    Copy,
    Equal,
    String,
}
struct Emitter<'a> {
    checked: &'a CheckedModule,
    headers: BTreeSet<&'static str>,
    helpers: BTreeSet<String>,
    out: String,
    scopes: Vec<HashMap<String, String>>,
    next: usize,
    loops: Vec<(Option<String>, Option<String>, String, bool)>,
    array_types: Vec<(Type, String, String)>,
    index_context: Vec<String>,
    record_types: Vec<RecordType>,
    value_targets: Vec<(String, String, Type)>,
    return_type: Type,
    enum_equalities: HashSet<Type>,
    enum_strings: HashSet<Type>,
    runtime_prototypes: Vec<String>,
    runtime_functions: Vec<String>,
    function_types: Vec<(Type, String)>,
    type_definitions: Vec<TypeDefinition>,
    mutexes: HashSet<String>,
    mutex_types: Vec<(Type, String)>,
    value_helpers: HashMap<(ValueOperation, Type), String>,
}
impl Emitter<'_> {
    /// Generate one recursive runtime helper per operation/type, rather than
    /// recursively expanding type structure in the compiler itself.
    fn value_helper(
        &mut self,
        operation: ValueOperation,
        ty: &Type,
        arguments: &[&str],
    ) -> Result<String, Diagnostics> {
        let key = (operation, ty.clone());
        if let Some(name) = self.value_helpers.get(&key) {
            return Ok(format!("{name}({})", arguments.join(", ")));
        }
        let name = format!("nc_value_{}", self.value_helpers.len());
        self.value_helpers.insert(key, name.clone());
        let ct = self.c_type(ty)?;
        let (ret, params) = match operation {
            ValueOperation::Copy => (ct.clone(), format!("{ct} value")),
            ValueOperation::Equal => ("int".into(), format!("{ct} left, {ct} right")),
            ValueOperation::String => ("const char *".into(), format!("{ct} value")),
        };
        let signature = format!("static {ret} {name}({params})");
        self.runtime_prototypes.push(format!("{signature};"));
        let saved = std::mem::take(&mut self.out);
        self.line(format!("{signature} {{"));
        let result = match operation {
            ValueOperation::Copy => self.copy_body(ty, "value"),
            ValueOperation::Equal => self.equality_body("left", "right", ty),
            ValueOperation::String => self.string_body("value", ty),
        }?;
        self.line(format!("return {result}; }}"));
        let function = std::mem::replace(&mut self.out, saved);
        self.runtime_functions.push(function);
        Ok(format!("{name}({})", arguments.join(", ")))
    }
    fn define_type(&mut self, name: &str, dependencies: Vec<String>, code: String) {
        self.type_definitions.push(TypeDefinition {
            name: name.into(),
            dependencies,
            code,
        });
    }
    fn incomplete_dependencies(&self, types: Vec<String>) -> Vec<String> {
        types
            .into_iter()
            .filter(|name| {
                !self
                    .record_types
                    .iter()
                    .any(|(_, record, _)| record == name)
            })
            .collect()
    }
    fn line(&mut self, text: impl AsRef<str>) {
        self.out.push_str(text.as_ref());
        self.out.push('\n');
    }
    fn fresh(&mut self) -> String {
        self.next += 1;
        format!("nc_v{}", self.next)
    }
    fn bind(&mut self, name: &str) -> String {
        let id = self.fresh();
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.into(), id.clone());
        id
    }
    fn name(&self, name: &str) -> String {
        if let Some(TypeInfo::External(f)) = self.checked.types.get(name) {
            return f.symbol.clone();
        }
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .cloned()
            .unwrap_or_else(|| format!("nc_fn_{name}"))
    }
    fn ty(&self, expr: &Expr) -> Result<Type, Diagnostics> {
        self.checked
            .expression_types
            .get(&(expr as *const Expr as usize))
            .cloned()
            .ok_or_else(|| Diagnostics::one("internal error: missing expression type", 0..0))
    }
    fn c_type(&mut self, ty: &Type) -> Result<String, Diagnostics> {
        if matches!(ty, Type::Future(_)) {
            self.async_support();
            return Ok("nc_future *".into());
        }
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(n)
        {
            return self.c_type(&base.clone());
        }
        if let Type::Map(key, value) = ty {
            return self.c_type(&map_array(key, value));
        }
        if let Type::Function(params, ret) = ty {
            if let Some((_, name)) = self.function_types.iter().find(|(t, _)| t == ty) {
                return Ok(name.clone());
            }
            let name = format!("nc_callable_{}", self.function_types.len());
            self.function_types.push((ty.clone(), name.clone()));
            let ret = self.c_type(ret)?;
            let params = params
                .iter()
                .map(|t| self.c_type(t))
                .collect::<Result<Vec<_>, _>>()?;
            let suffix = if params.is_empty() {
                String::new()
            } else {
                format!(", {}", params.join(", "))
            };
            let dependencies =
                self.incomplete_dependencies(params.iter().cloned().chain([ret.clone()]).collect());
            self.define_type(
                &name,
                dependencies,
                format!("typedef struct {{ {ret} (*call)(void *{suffix}); void *env; }} {name};"),
            );
            return Ok(name);
        }
        if let Some((_, name, _)) = self.record_types.iter().find(|(t, _, _)| t == ty) {
            return Ok(name.clone());
        }
        if let Type::Named(n, _) = ty
            && matches!(self.checked.types.get(n), Some(TypeInfo::Enum(_)))
        {
            let name = format!("nc_enum_{n}");
            self.define_type(
                &name,
                vec![],
                format!("struct {name} {{ int tag; void *payload; }};"),
            );
            self.record_types.push((
                ty.clone(),
                name.clone(),
                vec![
                    ("tag".into(), "int".into()),
                    ("payload".into(), "void *".into()),
                ],
            ));
            return Ok(name);
        }
        if let Some(fields) = self.fields(ty) {
            let slot = self.record_types.len();
            let name = format!("nc_record_{slot}");
            self.record_types.push((ty.clone(), name.clone(), vec![]));
            let mut c_fields = vec![];
            for (field, ty) in fields {
                c_fields.push((field, self.c_type(&ty)?));
            }
            self.define_type(
                &name,
                c_fields.iter().map(|(_, ct)| ct.clone()).collect(),
                format!(
                    "struct {name} {{ {} }};",
                    c_fields
                        .iter()
                        .map(|(field, ct)| format!("{ct} {field};"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            );
            self.record_types[slot].2 = c_fields;
            return Ok(name);
        }
        if let Type::Array(element, _) = ty {
            let key = Type::Array(element.clone(), None);
            if let Some((_, name, _)) = self.array_types.iter().find(|(t, _, _)| *t == key) {
                return Ok(name.clone());
            }
            let slot = self.array_types.len();
            let name = format!("nc_arr_{slot}");
            self.array_types.push((key, name.clone(), String::new()));
            let element_type = self.c_type(element)?;
            self.headers.insert("stdint.h");
            self.define_type(
                &name,
                self.incomplete_dependencies(vec![element_type.clone()]),
                format!("typedef struct {{ uint64_t len, cap; {element_type} *vals; }} {name};"),
            );
            self.array_types[slot].2 = element_type;
            return Ok(name);
        }
        Ok(match ty {
            Type::Named(n, _) => match n.as_str() {
                "void" => "void",
                "float" => "double",
                "str" | "char" | "error" => "const char *",
                "bool" => {
                    self.headers.insert("stdbool.h");
                    "bool"
                }
                "int" => {
                    self.headers.insert("stdint.h");
                    "int64_t"
                }
                "uint" => {
                    self.headers.insert("stdint.h");
                    "uint64_t"
                }
                "byte" => {
                    self.headers.insert("stdint.h");
                    "uint8_t"
                }
                _ => return unsupported("this value type"),
            },
            _ => return unsupported("composite value types"),
        }
        .into())
    }
    fn temp(&mut self, expr: &Expr, value: String) -> Result<String, Diagnostics> {
        let ty = self.ty(expr)?;
        if ty == Type::void() {
            self.line(format!("{value};"));
            return Ok(String::new());
        }
        let ct = self.c_type(&ty)?;
        let id = self.fresh();
        self.line(format!("{ct} {id} = {value};"));
        Ok(id)
    }
    fn panic_support(&mut self) {
        self.headers.extend(["stdio.h", "stdlib.h"]);
        self.helpers.insert("static void nc_panic(const char *message) { fprintf(stderr, \"panic: %s\\n\", message); exit(1); }".into());
    }
    fn allocation_support(&mut self) {
        self.panic_support();
        self.headers.insert("stddef.h");
        self.helpers.insert("static void nc_panic(const char *message);\ntypedef struct nc_allocation { void *data; struct nc_allocation *next; } nc_allocation;\nstatic nc_allocation *nc_allocations;\nstatic void nc_cleanup(void) { while (nc_allocations) { nc_allocation *next = nc_allocations->next; free(nc_allocations->data); free(nc_allocations); nc_allocations = next; } }\nstatic void *nc_alloc(size_t count, size_t size) { if (size && count > (size_t)-1 / size) nc_panic(\"allocation overflow\"); void *data = calloc(count ? count : 1, size); nc_allocation *node = malloc(sizeof(*node)); if (!data || !node) nc_panic(\"out of memory\"); node->data = data; node->next = nc_allocations; nc_allocations = node; return data; }".into());
    }
    fn copy(&mut self, ty: &Type, value: &str) -> Result<String, Diagnostics> {
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(n)
        {
            return self.copy(&base.clone(), value);
        }
        if let Type::Map(key, inner) = ty {
            return self.copy(&map_array(key, inner), value);
        }
        if self.fields(ty).is_some() || matches!(ty, Type::Array(_, _)) {
            let call = self.value_helper(ValueOperation::Copy, ty, &[value])?;
            let ct = self.c_type(ty)?;
            let result = self.fresh();
            self.line(format!("{ct} {result} = {call};"));
            return Ok(result);
        }
        Ok(value.into())
    }
    fn copy_body(&mut self, ty: &Type, value: &str) -> Result<String, Diagnostics> {
        if let Some(fields) = self.fields(ty) {
            let ct = self.c_type(ty)?;
            let copy = self.fresh();
            self.line(format!("{ct} {copy} = {value};"));
            for (field, ty) in fields {
                let v = self.copy(&ty, &format!("({value}).{field}"))?;
                self.line(format!("{copy}.{field} = {v};"));
            }
            return Ok(copy);
        }
        if let Type::Array(element, _) = ty {
            self.allocation_support();
            let ct = self.c_type(ty)?;
            let elem = self.c_type(element)?;
            let copy = self.fresh();
            self.line(format!("{ct} {copy} = {{ ({value}).len, ({value}).len, nc_alloc(({value}).len, sizeof({elem})) }};"));
            let index = self.fresh();
            self.line(format!(
                "for (uint64_t {index} = 0; {index} < ({value}).len; ++{index}) {{"
            ));
            let child = self.copy(element, &format!("({value}).vals[{index}]"))?;
            self.line(format!("{copy}.vals[{index}] = {child};\n}}"));
            Ok(copy)
        } else {
            Ok(value.into())
        }
    }
    fn block(&mut self, body: &Block) -> Result<(), Diagnostics> {
        self.scopes.push(HashMap::new());
        for statement in &body.statements {
            self.statement(statement)?;
        }
        self.scopes.pop();
        Ok(())
    }
    fn statement(&mut self, statement: &Stmt) -> Result<(), Diagnostics> {
        match statement {
            Stmt::LabeledIf { label, value } => {
                let end = self.fresh();
                self.loops
                    .push((Some(label.clone()), None, end.clone(), false));
                let Expr::If { subject, arms } = value else {
                    return unsupported("labelled non-conditional");
                };
                self.conditional(subject.as_deref(), arms, false)?;
                self.loops.pop();
                self.line(format!("{end}:;"));
            }
            Stmt::Block(block) => {
                self.line("{");
                self.block(block)?;
                self.line("}");
            }
            Stmt::Var(v) => {
                if v.mutex {
                    let value = self.expr_as(&v.value, &v.ty)?;
                    let value = self.copy(&v.ty, &value)?;
                    let ty = self.mutex_type(&v.ty)?;
                    let name = self.bind(pattern_name(&v.pattern)?);
                    self.line(format!("{ty} *{name};"));
                    self.init_mutex(&name, &v.ty, &value)?;
                    self.mutexes.insert(name);
                    return Ok(());
                }
                let value = self.expr_as(&v.value, &v.ty)?;
                let value = self.copy(&v.ty, &value)?;
                self.declare_pattern(&v.pattern, &v.ty, &value)?;
            }
            Stmt::Assign { target, value } => {
                if let Expr::Index { object, index } = target
                    && self.ty(object)? == Type::Named("str".into(), vec![])
                {
                    self.unicode_support();
                    let location = self.place(object)?;
                    self.index_context.push(format!("nc_str_len({location})"));
                    let index = self.expr(index)?;
                    self.index_context.pop();
                    let value = self.expr(value)?;
                    self.line(format!(
                        "{location} = nc_str_replace({location},(uint64_t){index},{value});"
                    ));
                    return Ok(());
                }
                if let Expr::Index { object, index } = target
                    && let Type::Map(key, val) = self.ty(object)?
                {
                    let map = self.place(object)?;
                    let k = self.expr(index)?;
                    let v = self.expr_as(value, &val)?;
                    self.map_set(&map, &k, &v, &key, &val)?;
                    return Ok(());
                }
                let target_code = match target {
                    Expr::Index { .. } | Expr::Member { .. } => Some(self.place(target)?),
                    _ => None,
                };
                let value_type = if matches!(target, Expr::Name(n) if n == "_") {
                    self.ty(value)?
                } else {
                    self.ty(target)?
                };
                let value = self.expr_as(value, &value_type)?;
                let value = self.copy(&value_type, &value)?;
                match target {
                    Expr::Name(n) if n == "_" => {}
                    Expr::Name(n) => self.line(format!("{} = {value};", self.name(n))),
                    Expr::Index { .. } | Expr::Member { .. } => {
                        self.line(format!("{} = {value};", target_code.unwrap()))
                    }
                    _ => return unsupported("composite assignment"),
                }
            }
            Stmt::Expr(Expr::If { subject, arms }) => {
                self.conditional(subject.as_deref(), arms, false)?
            }
            Stmt::Expr(value) => {
                self.expr(value)?;
            }
            Stmt::Return(value) => {
                if value.is_none() && matches!(self.return_type, Type::ErrorUnion(_)) {
                    let ct = self.c_type(&self.return_type.clone())?;
                    self.line(format!("return ({ct}){{0}};"));
                    return Ok(());
                }
                let value = value
                    .as_ref()
                    .map(|v| self.expr_as(v, &self.return_type.clone()))
                    .transpose()?
                    .unwrap_or_default();
                self.line(format!("return {value};"));
            }
            Stmt::Throw(value) => {
                let value = self.expr(value)?;
                self.throw_value(&value)?;
            }
            Stmt::Assert(value) => {
                let value = self.expr(value)?;
                self.panic_support();
                self.line(format!("if (!({value})) nc_panic(\"assertion failed\");"));
            }
            Stmt::While {
                label,
                condition,
                body,
            } => {
                let start = self.fresh();
                let end = self.fresh();
                self.loops
                    .push((label.clone(), Some(start.clone()), end.clone(), true));
                self.line(format!("{start}:; {{"));
                let condition = self.expr(condition)?;
                self.line(format!("if (!({condition})) goto {end};"));
                self.block(body)?;
                self.line(format!("}} goto {start};\n{end}:;"));
                self.loops.pop();
            }
            Stmt::Break(value, label) => {
                if let Some(value) = value {
                    let Some((result, end, ty)) = self.value_targets.last().cloned() else {
                        return unsupported("break value without a target");
                    };
                    let value = self.expr_as(value, &ty)?;
                    self.line(format!("{result} = {value}; goto {end};"));
                    return Ok(());
                }
                let end = self.loop_target(label, false)?;
                self.line(format!("goto {end};"));
            }
            Stmt::Continue(label) => {
                let start = self.loop_target(label, true)?;
                self.line(format!("goto {start};"));
            }
            Stmt::For {
                label,
                name,
                iterable,
                body,
            } => {
                let ty = self.ty(iterable)?;
                let string = ty == Type::Named("str".into(), vec![]);
                if !matches!(ty, Type::Array(_, _) | Type::Map(_, _)) && !string {
                    return unsupported("iteration over this type");
                }
                let value = self.expr(iterable)?;
                let ct = self.c_type(&ty)?;
                let snapshot = self.fresh();
                self.line(format!("{ct} {snapshot} = {value};"));
                let length = if string {
                    self.unicode_support();
                    format!("nc_str_len({snapshot})")
                } else {
                    format!("{snapshot}.len")
                };
                self.scopes.push(HashMap::new());
                let index = self.fresh();
                let start = self.fresh();
                let end = self.fresh();
                let next = self.fresh();
                self.loops
                    .push((label.clone(), Some(next.clone()), end.clone(), true));
                self.line(format!(
                    "uint64_t {index} = 0;\n{start}:; {{\nif ({index} >= {length}) goto {end};"
                ));
                let binding = self.bind(name);
                if let Type::Map(key, _) = &ty {
                    let ct = self.c_type(key)?;
                    self.line(format!("{ct} {binding} = {snapshot}.vals[{index}].f_0;"));
                } else {
                    self.line(format!("uint64_t {binding} = {index};"));
                }
                self.block(body)?;
                self.line(format!("}}\n{next}:; ++{index}; goto {start};\n{end}:;"));
                self.loops.pop();
                self.scopes.pop();
            }
            Stmt::Lock { name, body, label } => {
                let mutex = self.name(name);
                let guard = self.fresh();
                let end = self.fresh();
                self.line(format!("{{ pthread_mutex_lock(&({mutex})->lock); nc_lock_guard {guard} __attribute__((cleanup(nc_unlock))) = {{ &({mutex})->lock }};"));
                self.scopes
                    .push(HashMap::from([(name.clone(), format!("({mutex})->value"))]));
                self.loops.push((label.clone(), None, end.clone(), true));
                self.block(body)?;
                self.loops.pop();
                self.scopes.pop();
                self.line(format!("}} {end}:;"));
            }
        }
        Ok(())
    }
    fn loop_target(&self, label: &Option<String>, continuing: bool) -> Result<String, Diagnostics> {
        self.loops
            .iter()
            .rev()
            .find(|(name, start, _, can_break)| {
                (label.is_none() || name == label)
                    && (!continuing || start.is_some())
                    && (label.is_some() || continuing || *can_break)
            })
            .map(|(_, start, end, _)| {
                if continuing {
                    start.as_ref().unwrap().clone()
                } else {
                    end.clone()
                }
            })
            .ok_or_else(|| Diagnostics::one("invalid loop target", 0..0))
    }
    fn conditional(
        &mut self,
        subject: Option<&Expr>,
        arms: &[(Vec<Pattern>, Block)],
        yields: bool,
    ) -> Result<(), Diagnostics> {
        let value = subject
            .map(|x| self.expr(x))
            .transpose()?
            .unwrap_or_else(|| "1".into());
        let ty = subject
            .map(|x| self.ty(x))
            .transpose()?
            .unwrap_or(Type::Named("bool".into(), vec![]));
        let done = self.fresh();
        for (patterns, body) in arms {
            for pattern in patterns {
                let next = self.fresh();
                self.scopes.push(HashMap::new());
                self.line("{");
                self.pattern(pattern, &value, &ty, &next)?;
                if yields {
                    self.value_block(body)?;
                } else {
                    self.block(body)?;
                }
                self.line(format!("}} goto {done};\n{next}:;"));
                self.scopes.pop();
            }
        }
        self.line(format!("{done}:;"));
        Ok(())
    }
    fn equality(&mut self, left: &str, right: &str, ty: &Type) -> Result<String, Diagnostics> {
        if self.fields(ty).is_some() || matches!(ty, Type::Array(_, _) | Type::Map(_, _)) {
            return self.value_helper(ValueOperation::Equal, ty, &[left, right]);
        }
        self.equality_body(left, right, ty)
    }
    fn equality_body(&mut self, left: &str, right: &str, ty: &Type) -> Result<String, Diagnostics> {
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(n)
        {
            return self.equality(left, right, &base.clone());
        }
        if let Some(declaration) = self.enum_decl(ty) {
            let helper = format!("nc_equal_{}", declaration.name);
            let ct = self.c_type(ty)?;
            let call = format!("{helper}({left},{right})");
            if !self.enum_equalities.insert(ty.clone()) {
                return Ok(call);
            }
            self.runtime_prototypes
                .push(format!("static int {helper}({ct} left, {ct} right);"));
            let saved = std::mem::take(&mut self.out);
            self.line(format!("static int {helper}({ct} left, {ct} right) {{"));
            let left = "left";
            let right = "right";
            self.headers.insert("stdbool.h");
            let result = self.fresh();
            self.line(format!("bool {result} = ({left}).tag == ({right}).tag; if ({result}) {{ switch (({left}).tag) {{"));
            for (tag, variant) in declaration.variants.iter().enumerate() {
                if variant.values.is_empty() {
                    continue;
                }
                let payload = Type::Tuple(variant.values.clone());
                let ct = self.c_type(&payload)?;
                self.line(format!("case {tag}: {{"));
                let eq = self.equality(
                    &format!("(*({ct}*)({left}).payload)"),
                    &format!("(*({ct}*)({right}).payload)"),
                    &payload,
                )?;
                self.line(format!("{result} = {eq}; break; }}"));
            }
            self.line("default: break; } }");
            self.line(format!("return {result}; }}"));
            let function = std::mem::replace(&mut self.out, saved);
            self.runtime_functions.push(function);
            return Ok(call);
        }
        if let Type::Map(key, value) = ty {
            self.headers.insert("stdbool.h");
            let result = self.fresh();
            let i = self.fresh();
            self.line(format!("bool {result} = {left}.len == {right}.len; for (uint64_t {i} = 0; {result} && {i} < {left}.len; ++{i}) {{"));
            let found = self.map_find(right, &format!("{left}.vals[{i}].f_0"), key)?;
            self.line(format!(
                "if ({found} == {right}.len) {{ {result} = false; }} else {{"
            ));
            let eq = self.equality(
                &format!("{left}.vals[{i}].f_1"),
                &format!("{right}.vals[{found}].f_1"),
                value,
            )?;
            self.line(format!("{result} = {eq}; }} }}"));
            return Ok(result);
        }
        if let Some(fields) = self.fields(ty) {
            let mut checks = vec![];
            for (field, ty) in fields {
                checks.push(self.equality(
                    &format!("({left}).{field}"),
                    &format!("({right}).{field}"),
                    &ty,
                )?);
            }
            return Ok(if checks.is_empty() {
                "1".into()
            } else {
                checks
                    .into_iter()
                    .map(|c| format!("({c})"))
                    .collect::<Vec<_>>()
                    .join(" && ")
            });
        }
        if let Type::Array(element, _) = ty {
            self.headers.insert("stdbool.h");
            let result = self.fresh();
            let index = self.fresh();
            self.line(format!("bool {result} = ({left}).len == ({right}).len;\nfor (uint64_t {index} = 0; {result} && {index} < ({left}).len; ++{index}) {{"));
            let eq = self.equality(
                &format!("({left}).vals[{index}]"),
                &format!("({right}).vals[{index}]"),
                element,
            )?;
            self.line(format!("{result} = {eq};\n}}"));
            return Ok(result);
        }
        if matches!(ty, Type::Named(n, _) if n == "str" || n == "char") {
            self.headers.insert("string.h");
            Ok(format!("strcmp({left}, {right}) == 0"))
        } else {
            Ok(format!("{left} == {right}"))
        }
    }
    fn expr(&mut self, e: &Expr) -> Result<String, Diagnostics> {
        let value = match e {
            Expr::Map(entries) => {
                let Type::Map(key, value) = self.ty(e)? else {
                    unreachable!()
                };
                let ct = self.c_type(&self.ty(e)?)?;
                let result = self.fresh();
                self.line(format!("{ct} {result} = {{0}};"));
                for (k, v) in entries {
                    let k = self.expr_as(k, &key)?;
                    let v = self.expr_as(v, &value)?;
                    self.map_set(&result, &k, &v, &key, &value)?;
                }
                return Ok(result);
            }
            Expr::Try(value) => {
                let value = self.expr(value)?;
                self.line(format!("if ({value}.failed) {{"));
                self.throw_value(&format!("{value}.error"))?;
                self.line("}");
                if self.ty(e)? == Type::void() {
                    return Ok(String::new());
                }
                format!("{value}.value")
            }
            Expr::Catch { value, name, body } => {
                let ty = self.ty(e)?;
                let void = ty == Type::void();
                let ct = self.c_type(&ty)?;
                let value = self.expr(value)?;
                let result = self.fresh();
                let end = self.fresh();
                if !void {
                    self.line(format!("{ct} {result};"));
                }
                self.line(format!("if (!{value}.failed) {{"));
                if !void {
                    self.line(format!("{result} = {value}.value;"));
                }
                self.line("} else {");
                self.scopes.push(HashMap::new());
                let error = self.bind(name);
                self.line(format!("const char *{error} = {value}.error;"));
                self.value_targets.push((result.clone(), end.clone(), ty));
                if void {
                    self.block(body)?;
                } else {
                    self.value_block(body)?;
                }
                self.value_targets.pop();
                self.scopes.pop();
                self.line(format!("}}\n{end}:;"));
                return Ok(if void { String::new() } else { result });
            }
            Expr::None => {
                let ct = self.c_type(&self.ty(e)?)?;
                format!("({ct}){{0}}")
            }
            Expr::If { subject, arms } => {
                let ty = self.ty(e)?;
                let ct = self.c_type(&ty)?;
                let result = self.fresh();
                let end = self.fresh();
                self.line(format!("{ct} {result};"));
                self.value_targets.push((result.clone(), end.clone(), ty));
                self.conditional(subject.as_deref(), arms, true)?;
                self.value_targets.pop();
                self.line(format!("{end}:;"));
                return Ok(result);
            }
            Expr::Else { value, fallback } => {
                let ty = self.ty(e)?;
                let ct = self.c_type(&ty)?;
                let value = self.expr(value)?;
                let result = self.fresh();
                let end = self.fresh();
                self.line(format!(
                    "{ct} {result};\nif ({value}.present) {{ {result} = {value}.value; }} else {{"
                ));
                self.value_targets.push((result.clone(), end.clone(), ty));
                self.value_block(fallback)?;
                self.value_targets.pop();
                self.line(format!("}}\n{end}:;"));
                return Ok(result);
            }
            Expr::Tuple(values) => {
                let ty = self.ty(e)?;
                let Type::Tuple(types) = &ty else {
                    return unsupported("non-tuple literal type");
                };
                let ct = self.c_type(&ty)?;
                let result = self.fresh();
                self.line(format!("{ct} {result};"));
                for (i, value) in values.iter().enumerate() {
                    let v = self.expr_as(value, &types[i])?;
                    let v = self.copy(&types[i], &v)?;
                    self.line(format!("{result}.f_{i} = {v};"));
                }
                return Ok(result);
            }
            Expr::StructInit { fields, .. } => {
                let ty = self.ty(e)?;
                let types = self
                    .fields(&ty)
                    .ok_or_else(|| Diagnostics::one("missing struct layout", 0..0))?;
                let ct = self.c_type(&ty)?;
                let result = self.fresh();
                self.line(format!("{ct} {result};"));
                for (field, value) in fields {
                    let field_type = &types
                        .iter()
                        .find(|(name, _)| *name == format!("f_{field}"))
                        .unwrap()
                        .1;
                    let v = self.expr_as(value, field_type)?;
                    let v = self.copy(field_type, &v)?;
                    self.line(format!("{result}.f_{field} = {v};"));
                }
                return Ok(result);
            }
            Expr::Array(values) => {
                let ty = self.ty(e)?;
                if matches!(ty, Type::Map(_, _)) && values.is_empty() {
                    let ct = self.c_type(&ty)?;
                    return self.temp(e, format!("({ct}){{0}}"));
                }
                let Type::Array(element, _) = &ty else {
                    return unsupported("array literal in this context");
                };
                let ct = self.c_type(&ty)?;
                let elem = self.c_type(element)?;
                self.allocation_support();
                let name = self.fresh();
                self.line(format!(
                    "{ct} {name} = {{ {}, {}, nc_alloc({}, sizeof({elem})) }};",
                    values.len(),
                    values.len(),
                    values.len()
                ));
                for (index, value) in values.iter().enumerate() {
                    let v = self.expr_as(value, element)?;
                    let v = self.copy(element, &v)?;
                    self.line(format!("{name}.vals[{index}] = {v};"));
                }
                return Ok(name);
            }
            Expr::Index { object, index } => self.index(object, index)?,
            Expr::Member { object, name } => {
                let ty = self.ty(object)?;
                if name == "len" && ty == Type::Named("str".into(), vec![]) {
                    self.unicode_support();
                    let object = self.expr(object)?;
                    return self.temp(e, format!("nc_str_len({object})"));
                }
                if let Some(declaration) = self.enum_decl(&ty) {
                    let tag = declaration
                        .variants
                        .iter()
                        .position(|v| v.name == *name)
                        .unwrap();
                    let ct = self.c_type(&ty)?;
                    return self.temp(e, format!("({ct}){{{tag},0}}"));
                }
                let object = self.expr(object)?;
                format!(
                    "({object}).{}{name}",
                    if self.fields(&ty).is_some() { "f_" } else { "" }
                )
            }
            Expr::Int(n) => {
                let n = integer(n)?;
                if matches!(self.ty(e)?, Type::Named(n, _) if n == "uint") {
                    format!("{n}ULL")
                } else {
                    format!("{n}LL")
                }
            }
            Expr::Float(n) => n.clone(),
            Expr::String(s) | Expr::Char(s) => c_string(s),
            Expr::Bool(b) => {
                self.headers.insert("stdbool.h");
                b.to_string()
            }
            Expr::Name(n) if n == "$" => format!(
                "({}) - 1",
                self.index_context
                    .last()
                    .ok_or_else(|| Diagnostics::one("$ outside indexing", 0..0))?
            ),
            Expr::Name(n) => {
                let name = self.name(n);
                if self.mutexes.contains(&name) {
                    self.line(format!("pthread_mutex_lock(&({name})->lock);"));
                    let value = self.copy(&self.ty(e)?, &format!("({name})->value"))?;
                    let value = self.temp(e, value)?;
                    self.line(format!("pthread_mutex_unlock(&({name})->lock);"));
                    return Ok(value);
                }
                if matches!(self.ty(e)?, Type::Function(_, _))
                    && !self.scopes.iter().any(|s| s.contains_key(n))
                {
                    return self.function_value(e, n);
                }
                self.name(n)
            }
            Expr::Lambda(f) => return self.lambda(e, f),
            Expr::Async(call) => return self.spawn(e, call),
            Expr::Await(future) => {
                let value = self.expr(future)?;
                self.line(format!("nc_wait({value});"));
                let ty = self.ty(e)?;
                if ty == Type::void() {
                    return Ok(String::new());
                }
                let ct = self.c_type(&ty)?;
                format!("*({ct}*)({value}->result)")
            }
            Expr::Cast { ty, value } => {
                let from = self.ty(value)?;
                let value = self.expr(value)?;
                if matches!(ty,Type::Array(element,None) if **element == Type::Named("byte".into(),vec![]))
                    && matches!(&from,Type::Named(n,_) if matches!(n.as_str(),"int"|"uint"|"float"))
                {
                    self.allocation_support();
                    self.headers.insert("stdint.h");
                    let bits = self.fresh();
                    if from == Type::Named("float".into(), vec![]) {
                        self.headers.insert("string.h");
                        self.line(format!("_Static_assert(sizeof(double)==8,\"NC requires 64-bit double\"); uint64_t {bits}; memcpy(&{bits},&{value},8);"));
                    } else {
                        self.line(format!("uint64_t {bits} = (uint64_t){value};"));
                    }
                    let ct = self.c_type(ty)?;
                    let result = self.fresh();
                    let i = self.fresh();
                    self.line(format!("{ct} {result} = {{8,8,nc_alloc(8,1)}}; for (uint64_t {i}=0; {i}<8; ++{i}) {result}.vals[{i}]=(uint8_t)({bits} >> (8*{i}));"));
                    return Ok(result);
                }
                if let Type::Array(element, None) = ty
                    && matches!(&from,Type::Named(n,_) if n == "str" || n == "char")
                {
                    self.unicode_support();
                    let ct = self.c_type(ty)?;
                    let elem = self.c_type(element)?;
                    let bytes = **element == Type::Named("byte".into(), vec![]);
                    let length = if bytes {
                        format!("strlen({value})")
                    } else {
                        format!("nc_str_len({value})")
                    };
                    let result = self.fresh();
                    self.line(format!("{ct} {result} = {{ {length}, {length}, nc_alloc({length},sizeof({elem})) }};"));
                    if bytes {
                        self.line(format!("memcpy({result}.vals,{value},{result}.len);"));
                    } else {
                        let i = self.fresh();
                        let cursor = self.fresh();
                        let next = self.fresh();
                        let ch = self.fresh();
                        self.line(format!("const char *{cursor} = {value}; for (uint64_t {i}=0; {i}<{result}.len; ++{i}) {{ const char *{next} = nc_grapheme_next({cursor}); size_t length=(size_t)({next}-{cursor}); char *{ch}=nc_alloc(length+1,1); memcpy({ch},{cursor},length); {result}.vals[{i}]={ch}; {cursor}={next}; }}"));
                    }
                    return Ok(result);
                }
                if matches!(ty, Type::Named(n, _) if n == "str") {
                    return self.string_value(&value, &from);
                }
                if matches!((ty, &from), (Type::Array(_, None), Type::Array(_, Some(_)))) {
                    return Ok(value);
                }
                if let Type::Named(n, _) = ty {
                    match n.as_str() {
                        "byte" => {
                            self.panic_support();
                            self.line(format!(
                                "if ({value} < 0 || {value} > 255) nc_panic(\"cast out of range\");"
                            ));
                        }
                        "uint" => {
                            self.panic_support();
                            self.line(format!("if ({value} < 0) nc_panic(\"cast out of range\");"));
                            if matches!(&from,Type::Named(n,_) if n=="float") {
                                self.headers.insert("math.h");
                                self.line(format!("if (!isfinite({value}) || {value} >= 18446744073709551616.0) nc_panic(\"cast out of range\");"));
                            }
                        }
                        "int" if matches!(&from,Type::Named(n,_) if n=="uint") => {
                            self.panic_support();
                            self.line(format!("if ({value} > 9223372036854775807ULL) nc_panic(\"cast out of range\");"));
                        }
                        "int" if matches!(&from,Type::Named(n,_) if n=="float") => {
                            self.panic_support();
                            self.headers.insert("math.h");
                            self.line(format!("if (!isfinite({value}) || {value} < -9223372036854775808.0 || {value} >= 9223372036854775808.0) nc_panic(\"cast out of range\");"));
                        }
                        _ => {}
                    }
                }
                let ct = self.c_type(ty)?;
                format!("(({ct})({value}))")
            }
            Expr::Unary { op, value } => {
                if *op == UnaryOp::Neg
                    && matches!(&**value, Expr::Int(text) if !text.ends_with('u') && integer(text).ok() == Some(1u64 << 63))
                {
                    return self.temp(e, "(-9223372036854775807LL - 1LL)".into());
                }
                let value = self.expr(value)?;
                if *op == UnaryOp::Neg && !matches!(self.ty(e)?, Type::Named(n, _) if n == "float")
                {
                    return self.arithmetic(e, "0", BinaryOp::Sub, &value);
                }
                format!(
                    "({}{value})",
                    match op {
                        UnaryOp::Neg => "-",
                        UnaryOp::Not => "!",
                        UnaryOp::BitNot => "~",
                    }
                )
            }
            Expr::Binary { left, op, right } => {
                let l = self.expr(left)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.headers.insert("stdbool.h");
                    let result = self.fresh();
                    self.line(format!("bool {result} = {l};"));
                    self.line(format!(
                        "if ({}{result}) {{",
                        if *op == BinaryOp::Or { "!" } else { "" }
                    ));
                    let r = self.expr(right)?;
                    self.line(format!("{result} = {r};\n}}"));
                    return Ok(result);
                }
                let r = self.expr(right)?;
                match op {
                    BinaryOp::Eq | BinaryOp::Ne => {
                        let eq = self.equality(&l, &r, &self.ty(left)?)?;
                        format!("{}({eq})", if *op == BinaryOp::Ne { "!" } else { "" })
                    }
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow
                    | BinaryOp::Shl
                    | BinaryOp::Shr => return self.arithmetic(e, &l, *op, &r),
                    BinaryOp::Concat => {
                        let ty = self.ty(left)?;
                        if let Type::Map(key, value) = &ty {
                            let result = self.copy(&ty, &l)?;
                            let i = self.fresh();
                            self.line(format!("for (uint64_t {i} = 0; {i} < {r}.len; ++{i}) {{"));
                            self.map_set(
                                &result,
                                &format!("{r}.vals[{i}].f_0"),
                                &format!("{r}.vals[{i}].f_1"),
                                key,
                                value,
                            )?;
                            self.line("}");
                            return Ok(result);
                        }
                        if let Type::Array(element, _) = &ty {
                            let ct = self.c_type(&ty)?;
                            let elem = self.c_type(element)?;
                            self.allocation_support();
                            let result = self.fresh();
                            self.line(format!("if ({l}.len > UINT64_MAX - {r}.len) nc_panic(\"array length overflow\");\n{ct} {result} = {{ {l}.len + {r}.len, {l}.len + {r}.len, nc_alloc({l}.len + {r}.len, sizeof({elem})) }};"));
                            for (src, offset) in [(&l, "0".into()), (&r, format!("{l}.len"))] {
                                let i = self.fresh();
                                self.line(format!(
                                    "for (uint64_t {i} = 0; {i} < {src}.len; ++{i}) {{"
                                ));
                                let v = self.copy(element, &format!("{src}.vals[{i}]"))?;
                                self.line(format!("{result}.vals[{offset} + {i}] = {v};\n}}"));
                            }
                            return Ok(result);
                        }
                        self.allocation_support();
                        self.headers.insert("string.h");
                        let result = self.fresh();
                        let a = self.fresh();
                        let b = self.fresh();
                        self.line(format!("size_t {a} = strlen({l}), {b} = strlen({r});\nif ({a} > (size_t)-1 - {b} - 1) nc_panic(\"string length overflow\");\nchar *{result} = nc_alloc({a} + {b} + 1, 1);\nmemcpy({result}, {l}, {a}); memcpy({result} + {a}, {r}, {b} + 1);"));
                        return Ok(result);
                    }
                    BinaryOp::In => {
                        if let Type::Map(key, _) = self.ty(right)? {
                            let found = self.map_find(&r, &l, &key)?;
                            return self.temp(e, format!("{found} < {r}.len"));
                        }
                        if let Type::Array(element, _) = self.ty(right)? {
                            self.headers.insert("stdbool.h");
                            let result = self.fresh();
                            let i = self.fresh();
                            self.line(format!("bool {result} = false;\nfor (uint64_t {i} = 0; {i} < {r}.len; ++{i}) {{"));
                            let eq = self.equality(&l, &format!("{r}.vals[{i}]"), &element)?;
                            self.line(format!("if ({eq}) {{ {result} = true; break; }}\n}}"));
                            return Ok(result);
                        }
                        self.headers.insert("string.h");
                        format!("strstr({r}, {l}) != 0")
                    }
                    _ => format!("({l} {} {r})", operator(*op)),
                }
            }
            Expr::Call { callee, args, .. } => {
                if let Expr::Member { object, name } = &**callee {
                    let ty = self.ty(object)?;
                    if let Some(declaration) = self.enum_decl(&ty) {
                        let tag = declaration
                            .variants
                            .iter()
                            .position(|v| v.name == *name)
                            .unwrap();
                        let variant = &declaration.variants[tag];
                        let payload = self.c_type(&Type::Tuple(variant.values.clone()))?;
                        let ct = self.c_type(&ty)?;
                        self.allocation_support();
                        let pointer = self.fresh();
                        self.line(format!(
                            "{payload} *{pointer} = nc_alloc(1,sizeof({payload}));"
                        ));
                        for (i, (arg, ty)) in args.iter().zip(&variant.values).enumerate() {
                            let arg = self.expr_as(arg, ty)?;
                            let arg = self.copy(ty, &arg)?;
                            self.line(format!("{pointer}->f_{i} = {arg};"));
                        }
                        return self.temp(e, format!("({ct}){{{tag},{pointer}}}"));
                    }
                }
                if let Expr::Name(name) = &**callee
                    && name.starts_with('@')
                {
                    self.headers.insert("stdio.h");
                    let stream = if name.starts_with("@e") {
                        "stderr"
                    } else {
                        "stdout"
                    };
                    for arg in args {
                        let value = self.expr(arg)?;
                        let ty = self.ty(arg)?;
                        if matches!(
                            ty,
                            Type::Array(_, _)
                                | Type::Map(_, _)
                                | Type::Tuple(_)
                                | Type::Optional(_)
                        ) || self.fields(&ty).is_some()
                            || self.enum_decl(&ty).is_some()
                            || matches!(&ty,Type::Named(n,_) if matches!(self.checked.types.get(n),Some(TypeInfo::Alias(_))))
                            || matches!(&ty, Type::Named(n, _) if n == "float")
                        {
                            let string = self.string_value(&value, &ty)?;
                            self.line(format!("fprintf({stream}, \"%s\", {string});"));
                            continue;
                        }
                        let (fmt, value) = match ty {
                            Type::Named(n, _) => match n.as_str() {
                                "str" | "char" | "error" => ("%s", value),
                                "bool" => ("%s", format!("{value} ? \"true\" : \"false\"")),
                                "float" => ("%.17g", value),
                                "uint" | "byte" => ("%llu", format!("(unsigned long long){value}")),
                                _ => ("%lld", format!("(long long){value}")),
                            },
                            _ => return unsupported("printing composite types"),
                        };
                        self.line(format!("fprintf({stream}, \"{fmt}\", {value});"));
                    }
                    if name.ends_with("println") {
                        self.line(format!("fputc('\\n', {stream});"));
                    }
                    return Ok(String::new());
                }
                let Type::Function(params, _) = self.ty(callee)? else {
                    return unsupported("calling this type");
                };
                if let Expr::Name(name) = &**callee
                    && !self.scopes.iter().any(|s| s.contains_key(name))
                {
                    let values = args
                        .iter()
                        .zip(&params)
                        .map(|(arg, ty)| {
                            let value = self.expr_as(arg, ty)?;
                            self.copy(ty, &value)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    return self.temp(e, format!("{}({})", self.name(name), values.join(", ")));
                }
                let callee = self.expr(callee)?;
                let args = args
                    .iter()
                    .zip(params)
                    .map(|(a, ty)| {
                        let value = self.expr_as(a, &ty)?;
                        self.copy(&ty, &value)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let suffix = if args.is_empty() {
                    String::new()
                } else {
                    format!(", {}", args.join(", "))
                };
                format!("{callee}.call({callee}.env{suffix})")
            }
            _ => return unsupported("this expression"),
        };
        self.temp(e, value)
    }
    fn mutex_type(&mut self, ty: &Type) -> Result<String, Diagnostics> {
        if let Some((_, name)) = self.mutex_types.iter().find(|(t, _)| t == ty) {
            return Ok(name.clone());
        }
        self.allocation_support();
        self.headers.insert("pthread.h");
        self.helpers.insert("typedef struct { pthread_mutex_t *lock; } nc_lock_guard; static void nc_unlock(nc_lock_guard *guard) { pthread_mutex_unlock(guard->lock); }".into());
        let ct = self.c_type(ty)?;
        let name = self.fresh();
        self.define_type(
            &name,
            vec![ct.clone()],
            format!("typedef struct {{ pthread_mutex_t lock; {ct} value; }} {name};"),
        );
        self.mutex_types.push((ty.clone(), name.clone()));
        Ok(name)
    }
    fn init_mutex(&mut self, name: &str, ty: &Type, value: &str) -> Result<(), Diagnostics> {
        let ct = self.mutex_type(ty)?;
        self.line(format!("{name} = nc_alloc(1,sizeof({ct})); if (pthread_mutex_init(&{name}->lock,0)) nc_panic(\"cannot initialize mutex\"); {name}->value = {value};"));
        Ok(())
    }
    fn async_support(&mut self) {
        self.allocation_support();
        self.headers.insert("pthread.h");
        self.helpers.insert("/* async runtime */".into());
    }
    fn unicode_support(&mut self) {
        self.allocation_support();
        self.headers.extend(["stdint.h", "string.h"]);
        self.helpers.insert("/* unicode runtime */".into());
    }
    fn spawn(&mut self, e: &Expr, call: &Expr) -> Result<String, Diagnostics> {
        self.async_support();
        let Expr::Call { callee, args, .. } = call else {
            return unsupported("async non-call");
        };
        let callee_type = self.ty(callee)?;
        let Type::Function(params, ret) = &callee_type else {
            return unsupported("async builtin call");
        };
        let callee_value = self.expr(callee)?;
        let mut values = vec![callee_value];
        for (arg, ty) in args.iter().zip(params) {
            let value = self.expr_as(arg, ty)?;
            values.push(self.copy(ty, &value)?);
        }
        let mut fields = vec![callee_type.clone()];
        fields.extend(params.clone());
        let payload_type = self.c_type(&Type::Tuple(fields))?;
        let result_type = if **ret == Type::void() {
            "unsigned char".into()
        } else {
            self.c_type(ret)?
        };
        let job_type = self.fresh();
        self.define_type(&job_type, vec![payload_type.clone(), result_type.clone()], format!("typedef struct {{ nc_future future; {payload_type} args; {result_type} result; }} {job_type};"));
        let worker = self.fresh();
        let call_args = (0..params.len())
            .map(|i| format!(", job->args.f_{}", i + 1))
            .collect::<String>();
        self.runtime_prototypes
            .push(format!("static void *{worker}(void *raw);"));
        self.runtime_functions.push(format!("static void *{worker}(void *raw) {{ {job_type} *job = raw; {}job->args.f_0.call(job->args.f_0.env{call_args}); return 0; }}", if **ret == Type::void() { "" } else { "job->result = " }));
        let job = self.fresh();
        self.line(format!(
            "{job_type} *{job} = nc_alloc(1,sizeof({job_type}));"
        ));
        for (i, value) in values.iter().enumerate() {
            self.line(format!("{job}->args.f_{i} = {value};"));
        }
        self.line(format!(
            "{job}->future.result = &{job}->result; nc_start(&{job}->future, {worker}, {job});"
        ));
        self.temp(e, format!("&{job}->future"))
    }
    fn function_value(&mut self, e: &Expr, name: &str) -> Result<String, Diagnostics> {
        let ty = self.ty(e)?;
        let Type::Function(params, ret) = &ty else {
            unreachable!()
        };
        let ct = self.c_type(&ty)?;
        let ret_c = self.c_type(ret)?;
        let wrapper = self.fresh();
        let mut decls = vec!["void *env".into()];
        let mut args = vec![];
        for (i, t) in params.iter().enumerate() {
            decls.push(format!("{} a{i}", self.c_type(t)?));
            args.push(format!("a{i}"));
        }
        let signature = format!("static {ret_c} {wrapper}({})", decls.join(", "));
        self.runtime_prototypes.push(format!("{signature};"));
        let call = format!("{}({})", self.name(name), args.join(", "));
        self.runtime_functions.push(format!(
            "{signature} {{ (void)env; {}{call}; }}",
            if **ret == Type::void() { "" } else { "return " }
        ));
        self.temp(e, format!("({ct}){{{wrapper},0}}"))
    }
    fn lambda(&mut self, e: &Expr, f: &Function) -> Result<String, Diagnostics> {
        let captures = self.checked.captures[&(e as *const Expr as usize)].clone();
        let env_ct = if captures.is_empty() {
            None
        } else {
            let name = self.fresh();
            let mut fields = vec![];
            let mut dependencies = vec![];
            for (i, (_, ty, mutex)) in captures.iter().enumerate() {
                let ct = if *mutex {
                    format!("{} *", self.mutex_type(ty)?)
                } else {
                    self.c_type(ty)?
                };
                dependencies.push(ct.trim_end_matches(" *").into());
                fields.push(format!("{ct} f_{i};"));
            }
            self.define_type(
                &name,
                dependencies,
                format!("typedef struct {{ {} }} {name};", fields.join(" ")),
            );
            Some(name)
        };
        let ct = self.c_type(&self.ty(e)?)?;
        let function = self.fresh();
        let ret_ct = self.c_type(&f.return_type)?;
        let mut params = vec!["void *nc_env".into()];
        let mut scope = HashMap::new();
        for (i, (n, _, mutex)) in captures.iter().enumerate() {
            let code = format!("(({}*)nc_env)->f_{i}", env_ct.as_ref().unwrap());
            if *mutex {
                self.mutexes.insert(code);
            }
            scope.insert(
                n.clone(),
                format!("(({}*)nc_env)->f_{i}", env_ct.as_ref().unwrap()),
            );
        }
        for p in &f.params {
            let name = self.fresh();
            params.push(format!("{} {name}", self.c_type(&p.ty)?));
            scope.insert(p.name.clone(), name);
        }
        let signature = format!("static {ret_ct} {function}({})", params.join(", "));
        self.runtime_prototypes.push(format!("{signature};"));
        let out = std::mem::take(&mut self.out);
        let scopes = std::mem::replace(&mut self.scopes, vec![scope]);
        let ret = std::mem::replace(&mut self.return_type, f.return_type.clone());
        let loops = std::mem::take(&mut self.loops);
        let targets = std::mem::take(&mut self.value_targets);
        let indices = std::mem::take(&mut self.index_context);
        self.line(format!("{signature} {{"));
        self.block(&f.body)?;
        if f.return_type == Type::ErrorUnion(Box::new(Type::void())) {
            self.line(format!("return ({ret_ct}){{0}};"));
        }
        self.line("}");
        let body = std::mem::replace(&mut self.out, out);
        self.runtime_functions.push(body);
        self.scopes = scopes;
        self.return_type = ret;
        self.loops = loops;
        self.value_targets = targets;
        self.index_context = indices;
        let environment = if let Some(env_ct) = env_ct {
            self.allocation_support();
            let env = self.fresh();
            self.line(format!("{env_ct} *{env} = nc_alloc(1,sizeof({env_ct}));"));
            for (i, (name, ty, mutex)) in captures.iter().enumerate() {
                let value = if *mutex {
                    self.name(name)
                } else {
                    self.copy(ty, &self.name(name))?
                };
                self.line(format!("{env}->f_{i} = {value};"));
            }
            env
        } else {
            "0".into()
        };
        self.temp(e, format!("({ct}){{{function},{environment}}}"))
    }
    // A writable place must retain the original storage, not an expression copy.
    fn place(&mut self, e: &Expr) -> Result<String, Diagnostics> {
        match e {
            Expr::Name(name) => Ok(self.name(name)),
            Expr::Member { object, name } => Ok(format!("({}).f_{name}", self.place(object)?)),
            Expr::Index { object, index } => {
                let storage = self.place(object)?;
                let ty = self.ty(object)?;
                if let Type::Tuple(_) = ty {
                    let Expr::Int(i) = &**index else {
                        return unsupported("dynamic tuple indexing");
                    };
                    return Ok(format!("({storage}).f_{}", integer(i)?));
                }
                self.index_context.push(format!("({storage}).len"));
                let i = self.expr(index)?;
                self.index_context.pop();
                self.panic_support();
                if let Type::Map(key, _) = ty {
                    let found = self.map_find(&storage, &i, &key)?;
                    self.line(format!(
                        "if ({found} == ({storage}).len) nc_panic(\"map key not found\");"
                    ));
                    Ok(format!("({storage}).vals[{found}].f_1"))
                } else {
                    self.line(format!("if ((uint64_t)({i}) >= ({storage}).len) nc_panic(\"array index out of bounds\");"));
                    Ok(format!("({storage}).vals[{i}]"))
                }
            }
            _ => unsupported("assignment target"),
        }
    }
    fn index(&mut self, object: &Expr, index: &Expr) -> Result<String, Diagnostics> {
        if self.ty(object)? == Type::Named("str".into(), vec![]) {
            self.unicode_support();
            let object = self.expr(object)?;
            self.index_context.push(format!("nc_str_len({object})"));
            let index = self.expr(index)?;
            self.index_context.pop();
            return Ok(format!("nc_str_index({object},(uint64_t){index})"));
        }
        if let Type::Map(key, _) = self.ty(object)? {
            let map = self.expr(object)?;
            let key_value = self.expr(index)?;
            let found = self.map_find(&map, &key_value, &key)?;
            self.panic_support();
            self.line(format!(
                "if ({found} == {map}.len) nc_panic(\"map key not found\");"
            ));
            return Ok(format!("{map}.vals[{found}].f_1"));
        }
        if matches!(self.ty(object)?, Type::Tuple(_)) {
            let object = self.expr(object)?;
            let Expr::Int(index) = index else {
                return unsupported("dynamic tuple indexing");
            };
            return Ok(format!("({object}).f_{}", integer(index)?));
        }
        let object = self.expr(object)?;
        self.index_context.push(format!("({object}).len"));
        let index = self.expr(index)?;
        self.index_context.pop();
        self.panic_support();
        self.line(format!(
            "if ((uint64_t)({index}) >= ({object}).len) nc_panic(\"array index out of bounds\");"
        ));
        Ok(format!("({object}).vals[{index}]"))
    }
    fn fields(&self, ty: &Type) -> Option<Vec<(String, Type)>> {
        match ty {
            Type::ErrorUnion(inner) => Some(vec![
                ("failed".into(), Type::Named("bool".into(), vec![])),
                ("error".into(), Type::Named("str".into(), vec![])),
                (
                    "value".into(),
                    if **inner == Type::void() {
                        Type::Named("byte".into(), vec![])
                    } else {
                        (**inner).clone()
                    },
                ),
            ]),
            Type::Optional(inner) => Some(vec![
                ("present".into(), Type::Named("bool".into(), vec![])),
                ("value".into(), (**inner).clone()),
            ]),
            Type::Tuple(types) => Some(
                types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (format!("f_{i}"), t.clone()))
                    .collect(),
            ),
            Type::Named(n, _) => {
                if let Some(TypeInfo::Struct(declaration)) = self.checked.types.get(n) {
                    Some(
                        declaration
                            .fields
                            .iter()
                            .map(|f| (format!("f_{}", f.name), f.ty.clone()))
                            .collect(),
                    )
                } else {
                    None
                }
            }
            _ => None,
        }
    }
    fn map_find(&mut self, map: &str, key: &str, ty: &Type) -> Result<String, Diagnostics> {
        self.headers.insert("stdint.h");
        let index = self.fresh();
        self.line(format!(
            "uint64_t {index} = 0; for (; {index} < ({map}).len; ++{index}) {{"
        ));
        let eq = self.equality(&format!("({map}).vals[{index}].f_0"), key, ty)?;
        self.line(format!("if ({eq}) break; }}"));
        Ok(index)
    }
    fn map_set(
        &mut self,
        map: &str,
        key: &str,
        value: &str,
        key_ty: &Type,
        value_ty: &Type,
    ) -> Result<(), Diagnostics> {
        let found = self.map_find(map, key, key_ty)?;
        self.allocation_support();
        let entry_ty = self.c_type(&Type::Tuple(vec![key_ty.clone(), value_ty.clone()]))?;
        let new = self.fresh();
        self.line(format!("if ({found} == ({map}).len) {{\n{entry_ty} *{new} = nc_alloc(({map}).len + 1, sizeof({entry_ty}));\nfor (uint64_t nc_i = 0; nc_i < ({map}).len; ++nc_i) {new}[nc_i] = ({map}).vals[nc_i];\n({map}).vals = {new}; ++({map}).len; ({map}).cap = ({map}).len; }}"));
        let key = self.copy(key_ty, key)?;
        let value = self.copy(value_ty, value)?;
        self.line(format!(
            "({map}).vals[{found}].f_0 = {key}; ({map}).vals[{found}].f_1 = {value};"
        ));
        Ok(())
    }
    fn string_value(&mut self, value: &str, ty: &Type) -> Result<String, Diagnostics> {
        if self.fields(ty).is_some() || matches!(ty, Type::Array(_, _) | Type::Map(_, _)) {
            return self.value_helper(ValueOperation::String, ty, &[value]);
        }
        self.string_body(value, ty)
    }
    fn string_body(&mut self, value: &str, ty: &Type) -> Result<String, Diagnostics> {
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Alias(base)) = self.checked.types.get(n)
        {
            return self.string_value(value, &base.clone());
        }
        if let Some(declaration) = self.enum_decl(ty) {
            let helper = format!("nc_string_{}", declaration.name);
            let ct = self.c_type(ty)?;
            let call = format!("{helper}({value})");
            if !self.enum_strings.insert(ty.clone()) {
                return Ok(call);
            }
            self.runtime_prototypes
                .push(format!("static const char *{helper}({ct} value);"));
            let saved = std::mem::take(&mut self.out);
            self.line(format!("static const char *{helper}({ct} value) {{"));
            let value = "value";
            let result = self.fresh();
            self.line(format!(
                "const char *{result} = \"\"; switch (({value}).tag) {{"
            ));
            for (tag, variant) in declaration.variants.iter().enumerate() {
                self.line(format!(
                    "case {tag}: {{ {result} = {};",
                    c_string(&format!("{}.{}", declaration.name, variant.name))
                ));
                if !variant.values.is_empty() {
                    let ct = self.c_type(&Type::Tuple(variant.values.clone()))?;
                    self.append_string(&result, "\"(\"")?;
                    for (i, ty) in variant.values.iter().enumerate() {
                        if i > 0 {
                            self.append_string(&result, "\", \"")?;
                        }
                        let s =
                            self.string_value(&format!("(({ct}*)({value}).payload)->f_{i}"), ty)?;
                        let quoted = matches!(ty,Type::Named(n,_) if n == "str");
                        if quoted {
                            self.append_string(&result, "\"\\\"\"")?;
                        }
                        self.append_string(&result, &s)?;
                        if quoted {
                            self.append_string(&result, "\"\\\"\"")?;
                        }
                    }
                    self.append_string(&result, "\")\"")?;
                }
                self.line("break; }");
            }
            self.line("}");
            self.line(format!("return {result}; }}"));
            let function = std::mem::replace(&mut self.out, saved);
            self.runtime_functions.push(function);
            return Ok(call);
        }
        if let Type::Optional(inner) = ty {
            let result = self.fresh();
            self.line(format!(
                "const char *{result} = \"none\"; if ({value}.present) {{"
            ));
            let s = self.string_value(&format!("{value}.value"), inner)?;
            self.line(format!("{result} = {s}; }}"));
            return Ok(result);
        }
        if matches!(ty, Type::Array(_, _) | Type::Map(_, _)) {
            let result = self.fresh();
            let i = self.fresh();
            self.line(format!("const char *{result} = \"[\"; for (uint64_t {i} = 0; {i} < ({value}).len; ++{i}) {{\nif ({i}) {{"));
            self.append_string(&result, "\", \"")?;
            self.line("}");
            match ty {
                Type::Array(element, _) => {
                    let s = self.string_value(&format!("({value}).vals[{i}]"), element)?;
                    self.append_string(&result, &s)?;
                }
                Type::Map(key, val) => {
                    let k = self.string_value(&format!("({value}).vals[{i}].f_0"), key)?;
                    self.append_string(&result, &k)?;
                    self.append_string(&result, "\": \"")?;
                    let v = self.string_value(&format!("({value}).vals[{i}].f_1"), val)?;
                    self.append_string(&result, &v)?;
                }
                _ => unreachable!(),
            }
            self.line("}");
            self.append_string(&result, "\"]\"")?;
            return Ok(result);
        }
        if let Some(fields) = self.fields(ty) {
            let (open, close) = if let Type::Named(n, _) = ty {
                (format!("{n}{{"), "}")
            } else {
                ("(".into(), ")")
            };
            let result = self.fresh();
            self.line(format!("const char *{result} = {};", c_string(&open)));
            for (i, (field, ty_field)) in fields.iter().enumerate() {
                if i > 0 {
                    self.append_string(&result, "\", \"")?;
                }
                if matches!(ty, Type::Named(_, _)) {
                    self.append_string(
                        &result,
                        &c_string(&format!(".{} = ", field.trim_start_matches("f_"))),
                    )?;
                }
                let s = self.string_value(&format!("({value}).{field}"), ty_field)?;
                self.append_string(&result, &s)?;
            }
            self.append_string(&result, &c_string(close))?;
            return Ok(result);
        }
        let Type::Named(name, _) = ty else {
            return unsupported("composite-to-string conversion");
        };
        if matches!(name.as_str(), "str" | "char" | "error") {
            return Ok(value.into());
        }
        if name == "bool" {
            return Ok(format!("({value} ? \"true\" : \"false\")"));
        }
        self.allocation_support();
        self.headers.insert("stdio.h");
        let result = self.fresh();
        let (fmt, value) = match name.as_str() {
            "int" => ("%lld", format!("(long long){value}")),
            "uint" | "byte" => ("%llu", format!("(unsigned long long){value}")),
            "float" => ("%.17g", value.into()),
            _ => return unsupported("conversion of this type to str"),
        };
        self.line(format!(
            "char *{result} = nc_alloc(128, 1); snprintf({result}, 128, \"{fmt}\", {value});"
        ));
        if name == "float" {
            self.headers.insert("string.h");
            self.line(format!(
                "if (!strpbrk({result}, \".eE\")) strcat({result}, \".0\");"
            ));
        }
        Ok(result)
    }
    fn append_string(&mut self, result: &str, suffix: &str) -> Result<(), Diagnostics> {
        self.allocation_support();
        self.headers.insert("string.h");
        let a = self.fresh();
        let b = self.fresh();
        let text = self.fresh();
        self.line(format!("size_t {a} = strlen({result}), {b} = strlen({suffix}); if ({a} > (size_t)-1 - {b} - 1) nc_panic(\"string length overflow\");\nchar *{text} = nc_alloc({a} + {b} + 1, 1); memcpy({text},{result},{a}); memcpy({text}+{a},{suffix},{b}+1); {result} = {text};"));
        Ok(())
    }
    fn enum_decl(&self, ty: &Type) -> Option<EnumDecl> {
        if let Type::Named(n, _) = ty
            && let Some(TypeInfo::Enum(e)) = self.checked.types.get(n)
        {
            return Some(e.clone());
        }
        None
    }
    fn pattern(
        &mut self,
        p: &Pattern,
        value: &str,
        ty: &Type,
        next: &str,
    ) -> Result<(), Diagnostics> {
        match p {
            Pattern::Wildcard => {}
            Pattern::Name(n) => {
                if self.scopes.iter().any(|s| s.contains_key(n)) {
                    let eq = self.equality(value, &self.name(n), ty)?;
                    self.line(format!("if (!({eq})) goto {next};"));
                } else {
                    let ct = self.c_type(ty)?;
                    let name = self.bind(n);
                    self.line(format!("{ct} {name} = {value};"));
                }
            }
            Pattern::Literal(e) => {
                let e = self.expr(e)?;
                let eq = self.equality(value, &e, ty)?;
                self.line(format!("if (!({eq})) goto {next};"));
            }
            Pattern::Tuple(patterns) => {
                let Type::Tuple(types) = ty else {
                    unreachable!()
                };
                for (i, (p, t)) in patterns.iter().zip(types).enumerate() {
                    self.pattern(p, &format!("({value}).f_{i}"), t, next)?;
                }
            }
            Pattern::Array(patterns) => {
                let Type::Array(t, _) = ty else {
                    unreachable!()
                };
                self.line(format!(
                    "if (({value}).len != {}) goto {next};",
                    patterns.len()
                ));
                for (i, p) in patterns.iter().enumerate() {
                    self.pattern(p, &format!("({value}).vals[{i}]"), t, next)?;
                }
            }
            Pattern::Struct { fields, .. } => {
                let types = self.fields(ty).unwrap();
                for (field, p) in fields {
                    let (_, t) = types
                        .iter()
                        .find(|(n, _)| *n == format!("f_{field}"))
                        .unwrap();
                    self.pattern(p, &format!("({value}).f_{field}"), t, next)?;
                }
            }
            Pattern::Variant { name, values } => {
                let declaration = self.enum_decl(ty).unwrap();
                let variant = name.rsplit('.').next().unwrap();
                let tag = declaration
                    .variants
                    .iter()
                    .position(|v| v.name == variant)
                    .unwrap();
                let types = &declaration.variants[tag].values;
                self.line(format!("if (({value}).tag != {tag}) goto {next};"));
                if !values.is_empty() {
                    let ct = self.c_type(&Type::Tuple(types.clone()))?;
                    for (i, (p, t)) in values.iter().zip(types).enumerate() {
                        self.pattern(p, &format!("(({ct}*)({value}).payload)->f_{i}"), t, next)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn expr_as(&mut self, e: &Expr, expected: &Type) -> Result<String, Diagnostics> {
        let value = self.expr(e)?;
        if let Type::ErrorUnion(_) = expected
            && self.ty(e)? != *expected
        {
            let ct = self.c_type(expected)?;
            return Ok(format!("({ct}){{.value = {value}}}"));
        }
        if let Type::Optional(inner) = expected
            && self.ty(e)? != *expected
        {
            let ct = self.c_type(expected)?;
            let value = self.copy(inner, &value)?;
            return Ok(format!("({ct}){{1, {value}}}"));
        }
        Ok(value)
    }
    fn throw_value(&mut self, message: &str) -> Result<(), Diagnostics> {
        if matches!(self.return_type, Type::ErrorUnion(_)) {
            let ct = self.c_type(&self.return_type.clone())?;
            self.line(format!("return ({ct}){{.failed = 1, .error = {message}}};"));
        } else {
            self.headers.extend(["stdio.h", "stdlib.h"]);
            self.line(format!("fprintf(stderr, \"%s\\n\", {message}); exit(1);"));
        }
        Ok(())
    }
    fn value_block(&mut self, body: &Block) -> Result<(), Diagnostics> {
        self.scopes.push(HashMap::new());
        for (i, statement) in body.statements.iter().enumerate() {
            if i + 1 == body.statements.len()
                && let Stmt::Expr(value) = statement
            {
                let (result, end, ty) = self.value_targets.last().unwrap().clone();
                let value = self.expr_as(value, &ty)?;
                self.line(format!("{result} = {value}; goto {end};"));
                continue;
            }
            self.statement(statement)?;
        }
        self.scopes.pop();
        Ok(())
    }
    fn arithmetic(
        &mut self,
        expr: &Expr,
        left: &str,
        op: BinaryOp,
        right: &str,
    ) -> Result<String, Diagnostics> {
        let ty = self.ty(expr)?;
        let ct = self.c_type(&ty)?;
        self.panic_support();
        let result = self.fresh();
        self.line(format!("{ct} {result};"));
        if matches!(&ty, Type::Named(n, _) if n == "float") {
            self.headers.insert("math.h");
            let operation = match op {
                BinaryOp::Pow => format!("pow({left}, {right})"),
                BinaryOp::Mod => format!("fmod({left}, {right})"),
                _ => format!("{left} {} {right}", operator(op)),
            };
            self.line(format!("{result} = {operation};\nif (!isfinite({result})) nc_panic(\"floating-point overflow or invalid arithmetic\");"));
            return Ok(result);
        }
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                let operation = match op {
                    BinaryOp::Add => "add",
                    BinaryOp::Sub => "sub",
                    _ => "mul",
                };
                self.line(format!("if (__builtin_{operation}_overflow({left}, {right}, &{result})) nc_panic(\"integer overflow\");"));
            }
            BinaryOp::Div | BinaryOp::Mod => {
                self.line(format!("if ({right} == 0) nc_panic(\"division by zero\");"));
                if matches!(&ty, Type::Named(n, _) if n == "int") {
                    self.line(format!(
                        "if ({left} == INT64_MIN && {right} == -1) nc_panic(\"integer overflow\");"
                    ));
                }
                self.line(format!("{result} = {left} {} {right};", operator(op)));
            }
            BinaryOp::Pow => {
                self.headers.insert("stdint.h");
                if matches!(&ty, Type::Named(n, _) if n == "int") {
                    self.line(format!(
                        "if ({right} < 0) nc_panic(\"negative integer exponent\");"
                    ));
                }
                let base = self.fresh();
                let power = self.fresh();
                self.line(format!("{result} = 1; {ct} {base} = {left}; uint64_t {power} = (uint64_t){right};\nwhile ({power}) {{\nif (({power} & 1) && __builtin_mul_overflow({result}, {base}, &{result})) nc_panic(\"integer overflow\");\n{power} >>= 1;\nif ({power} && __builtin_mul_overflow({base}, {base}, &{base})) nc_panic(\"integer overflow\");\n}}"));
            }
            BinaryOp::Shl | BinaryOp::Shr => {
                let bits = if matches!(&ty, Type::Named(n, _) if n == "byte") {
                    8
                } else {
                    64
                };
                self.line(format!("if ((uint64_t){right} >= {bits}) nc_panic(\"shift count out of range\");\n{result} = {left};"));
                if op == BinaryOp::Shl {
                    let i = self.fresh();
                    self.line(format!("for (uint64_t {i} = 0; {i} < (uint64_t){right}; ++{i}) if (__builtin_mul_overflow({result}, 2, &{result})) nc_panic(\"integer overflow\");"));
                } else {
                    self.line(format!("{result} = {left} >> {right};"));
                }
            }
            _ => unreachable!(),
        }
        Ok(result)
    }
    fn declare_pattern(
        &mut self,
        pattern: &Pattern,
        ty: &Type,
        value: &str,
    ) -> Result<(), Diagnostics> {
        match (pattern, ty) {
            (Pattern::Tuple(patterns), Type::Tuple(types)) => {
                for (i, (pattern, ty)) in patterns.iter().zip(types).enumerate() {
                    self.declare_pattern(pattern, ty, &format!("({value}).f_{i}"))?;
                }
            }
            (Pattern::Name(n), _) => {
                let ct = self.c_type(ty)?;
                let name = self.bind(n);
                self.line(format!("{ct} {name} = {value};"));
            }
            _ => return unsupported("this binding pattern"),
        }
        Ok(())
    }
}
fn operator(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        _ => unreachable!(),
    }
}
fn map_array(key: &Type, value: &Type) -> Type {
    Type::Array(
        Box::new(Type::Tuple(vec![key.clone(), value.clone()])),
        None,
    )
}
fn c_string(value: &str) -> String {
    let mut s = String::from("\"");
    for b in value.bytes() {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            32..=126 => s.push(b as char),
            _ => s.push_str(&format!("\\{b:03o}")),
        }
    }
    s.push('"');
    s
}
