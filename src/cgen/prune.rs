//! Dead-code elimination over the generated C translation unit.
//!
//! The output (runtime included) is split into top-level items: functions, prototypes,
//! structs, typedefs, variables, `#define`s and `#include`s. Starting from `main`, only items
//! whose names are transitively referenced are kept, and a standard header is only included
//! when kept code uses something it declares. Comments travel with the item that follows them.

use crate::lexer::is_ident_byte;
use std::collections::{HashMap, HashSet};

struct Item {
    /// leading comments
    comments: String,
    code: String,
    kind: Kind,
}

#[derive(PartialEq)]
enum Kind {
    Include(String),
    /// include guard plumbing: dropped from the pruned output
    Guard,
    Decl(Vec<String>),
}

/// Identifiers declared by the standard headers the runtime uses.
const HEADERS: &[(&str, &str)] = &[
    ("math.h", "isnan isinf pow fmod NAN INFINITY HUGE_VAL"),
    (
        "pthread.h",
        "pthread_t pthread_mutex_t pthread_cond_t pthread_attr_t pthread_mutex_lock \
         pthread_mutex_unlock pthread_mutex_init pthread_mutex_destroy \
         pthread_cond_init pthread_cond_destroy pthread_cond_wait pthread_cond_signal \
         pthread_cond_broadcast pthread_create pthread_detach pthread_self \
         pthread_equal pthread_attr_init pthread_attr_setstacksize pthread_attr_destroy \
         PTHREAD_MUTEX_INITIALIZER PTHREAD_COND_INITIALIZER",
    ),
    ("setjmp.h", "jmp_buf setjmp longjmp"),
    (
        "stdatomic.h",
        "atomic_init atomic_load_explicit atomic_fetch_add_explicit \
         atomic_fetch_sub_explicit memory_order_relaxed memory_order_acquire \
         memory_order_acq_rel",
    ),
    ("stdbool.h", "bool true false"),
    ("stdckdint.h", "ckd_add ckd_sub ckd_mul"),
    ("stddef.h", "size_t max_align_t unreachable NULL ptrdiff_t"),
    (
        "stdint.h",
        "int8_t int16_t int32_t int64_t uint8_t uint16_t uint32_t uint64_t INT64_C \
         UINT64_C UINT32_C INT64_MIN INT64_MAX UINT64_MAX",
    ),
    (
        "stdio.h",
        "FILE stdout stderr fwrite fputs fputc fflush printf fprintf snprintf puts",
    ),
    (
        "stdlib.h",
        "malloc calloc realloc free exit strtod atoi getenv",
    ),
    ("string.h", "memcpy memcmp memset strlen strchr"),
];

/// Names of the runtime's public (non-`static`) functions, which extern C files may call.
pub fn public_api(runtime: &str) -> Vec<String> {
    split(runtime)
        .into_iter()
        .filter(|it| {
            !it.code.starts_with("static")
                && it.code.contains('(')
                && !it.code.starts_with("typedef")
        })
        .filter_map(|it| match it.kind {
            Kind::Decl(names) => names.into_iter().next(),
            _ => None,
        })
        .collect()
}

/// Keep `main`, `extra_roots` and everything they reference.
pub fn prune(src: &str, extra_roots: &[String]) -> String {
    let items = split(src);
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        if let Kind::Decl(names) = &it.kind {
            for n in names {
                by_name.entry(n.as_str()).or_default().push(i);
            }
        }
    }
    let mut keep = vec![false; items.len()];
    let mut used: HashSet<String> = HashSet::new();
    let mut work: Vec<usize> = by_name.get("main").cloned().unwrap_or_default();
    for r in extra_roots {
        work.extend(by_name.get(r.as_str()).cloned().unwrap_or_default());
    }
    // items that define nothing we can name are kept to be safe
    for (i, it) in items.iter().enumerate() {
        if matches!(&it.kind, Kind::Decl(n) if n.is_empty()) {
            work.push(i);
        }
    }
    while let Some(i) = work.pop() {
        if keep[i] {
            continue;
        }
        keep[i] = true;
        for id in identifiers(&items[i].code) {
            if used.insert(id.clone())
                && let Some(defs) = by_name.get(id.as_str())
            {
                work.extend(defs.iter().copied());
            }
        }
    }
    let mut out = String::new();
    for (i, it) in items.iter().enumerate() {
        let wanted = match &it.kind {
            Kind::Guard => false,
            Kind::Include(h) => match HEADERS.iter().find(|(name, _)| name == h) {
                Some((_, ids)) => ids.split_whitespace().any(|id| used.contains(id)),
                None => true,
            },
            Kind::Decl(_) => keep[i],
        };
        if !wanted {
            continue;
        }
        out.push_str(&it.comments);
        out.push_str(&it.code);
        out.push('\n');
        if it.code.ends_with('}') {
            out.push('\n');
        }
    }
    out
}

/// Split C source into top-level items.
fn split(src: &str) -> Vec<Item> {
    let b = src.as_bytes();
    let mut items = Vec::new();
    let mut comments = String::new();
    let mut i = 0;
    while i < b.len() {
        // skip whitespace between items, keeping blank lines out of the output
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // comments at top level attach to the next item
        if let Some(end) = comment_end(src, i) {
            comments.push_str(&src[i..end]);
            comments.push('\n');
            i = end;
            continue;
        }
        // preprocessor directive (with line continuations)
        if b[i] == b'#' {
            let mut end = i;
            loop {
                let nl = src[end..].find('\n').map_or(b.len(), |e| end + e);
                if nl > 0 && b[nl - 1] == b'\\' && nl < b.len() {
                    end = nl + 1;
                } else {
                    end = nl;
                    break;
                }
            }
            let line = src[i..end].trim().to_string();
            let kind = directive_kind(&line);
            items.push(Item {
                comments: std::mem::take(&mut comments),
                code: line,
                kind,
            });
            i = end;
            continue;
        }
        // declaration or definition: scan to its end
        let start = i;
        let mut depth = 0i32;
        while i < b.len() {
            if let Some(end) = comment_end(src, i) {
                i = end;
                continue;
            }
            match b[i] {
                b'"' | b'\'' => i = skip_literal(b, i),
                b'{' | b'(' | b'[' => {
                    depth += 1;
                    i += 1;
                }
                b'}' | b')' | b']' => {
                    depth -= 1;
                    i += 1;
                    if depth == 0 && b[i - 1] == b'}' {
                        // a closing brace ends a function body unless the declaration
                        // continues on the same line (`} name;`, `};`)
                        let mut j = i;
                        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                            j += 1;
                        }
                        if j >= b.len() || b[j] == b'\n' || b[j] == b'\r' {
                            break;
                        }
                    }
                }
                b';' if depth == 0 => {
                    i += 1;
                    break;
                }
                _ => i += 1,
            }
        }
        let code = src[start..i].trim().to_string();
        let names = decl_names(&code);
        items.push(Item {
            comments: std::mem::take(&mut comments),
            code,
            kind: Kind::Decl(names),
        });
    }
    items
}

/// The end of the comment starting at `i`, if one does.
fn comment_end(src: &str, i: usize) -> Option<usize> {
    let b = src.as_bytes();
    match (b[i], b.get(i + 1)) {
        (b'/', Some(b'*')) => Some(src[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2)),
        (b'/', Some(b'/')) => Some(src[i..].find('\n').map_or(b.len(), |e| i + e)),
        _ => None,
    }
}

fn skip_literal(b: &[u8], mut i: usize) -> usize {
    let q = b[i];
    i += 1;
    while i < b.len() && b[i] != q {
        if b[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    i + 1
}

fn directive_kind(line: &str) -> Kind {
    let body = line.trim_start_matches('#').trim_start();
    if let Some(rest) = body.strip_prefix("include") {
        let h = rest
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"')
            .to_string();
        return Kind::Include(h);
    }
    if let Some(rest) = body.strip_prefix("define") {
        let name = ident_prefix(rest.trim_start());
        if name == "NC_RUNTIME_H" {
            return Kind::Guard;
        }
        return Kind::Decl(vec![name.to_string()]);
    }
    // #ifndef / #ifdef / #endif at top level are the runtime's include guards
    Kind::Guard
}

/// The identifier (or number) `s` starts with, if any.
fn ident_prefix(s: &str) -> &str {
    &s[..s.bytes().take_while(|&b| is_ident_byte(b)).count()]
}

/// Identifiers in code, ignoring string/char literals and comments.
fn identifiers(code: &str) -> Vec<String> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(end) = comment_end(code, i) {
            i = end;
            continue;
        }
        match b[i] {
            b'"' | b'\'' => i = skip_literal(b, i),
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let id = ident_prefix(&code[i..]);
                i += id.len();
                out.push(id.to_string());
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && (is_ident_byte(b[i]) || b[i] == b'.') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Names declared by a top-level declaration.
fn decl_names(code: &str) -> Vec<String> {
    // the part before any body or initializer, at nesting depth 0
    let b = code.as_bytes();
    let mut depth = 0;
    let mut i = 0;
    let mut paren_ident: Option<String> = None;
    let mut last_ident_before_paren: Option<String> = None;
    let mut first_paren_seen = false;
    let mut tokens: Vec<String> = Vec::new();
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                i = skip_literal(b, i);
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b'(' => {
                if depth == 0 && !first_paren_seen {
                    first_paren_seen = true;
                    last_ident_before_paren = tokens.last().cloned();
                    // `(*name)` function pointer typedef
                    let rest = code[i + 1..].trim_start();
                    if let Some(r) = rest.strip_prefix('*') {
                        let n = ident_prefix(r.trim_start());
                        if !n.is_empty() {
                            paren_ident = Some(n.to_string());
                        }
                    }
                }
                depth += 1;
            }
            b'=' if depth == 0 => break,
            c if (c.is_ascii_alphabetic() || c == b'_') && depth == 0 => {
                let id = ident_prefix(&code[i..]);
                i += id.len();
                tokens.push(id.to_string());
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let first = tokens.first().map_or("", String::as_str);
    let mut names = Vec::new();
    if first == "typedef" {
        if let Some(n) = paren_ident {
            names.push(n);
        } else if let Some(n) = tokens.last() {
            names.push(n.clone());
        }
        // `typedef struct tag {...} name;` also defines the tag
        if let Some(pos) = tokens.iter().position(|t| t == "struct" || t == "union")
            && let Some(tag) = tokens.get(pos + 1)
        {
            names.push(tag.clone());
        }
        return names;
    }
    if let Some(pos) = tokens.iter().position(|t| t == "struct" || t == "union") {
        // `struct tag {...};` or `struct tag;` (no declarator)
        if tokens.len() == pos + 2 && !first_paren_seen {
            names.push(tokens[pos + 1].clone());
            return names;
        }
    }
    if let Some(n) = paren_ident {
        // function pointer variable: `T (*name)(args) = ...;`
        names.push(n);
        return names;
    }
    if first_paren_seen {
        // function definition or prototype
        if let Some(n) = last_ident_before_paren {
            names.push(n);
        }
        return names;
    }
    // variable: the last identifier before `=` / `;`
    if let Some(n) = tokens.last() {
        names.push(n.clone());
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            decl_names("static int64_t f0_add(nc_env *env, int64_t a) {\n  return a;\n}"),
            ["f0_add"]
        );
        assert_eq!(
            decl_names("typedef struct nc_str nc_str;"),
            ["nc_str", "nc_str"]
        );
        assert_eq!(
            decl_names("struct nc_Point {\n  int64_t f_x;\n};"),
            ["nc_Point"]
        );
        assert_eq!(
            decl_names("typedef int64_t (*nc_fp_x)(nc_env *, int64_t);"),
            ["nc_fp_x"]
        );
        assert_eq!(
            decl_names("static nc_str ls0 = {-1, 5, 5, \"hello\"};"),
            ["ls0"]
        );
        assert_eq!(
            decl_names("static const nc_ti ti_5 = {sizeof(nc_str *), vrt_5};"),
            ["ti_5"]
        );
        assert_eq!(decl_names("static const nc_ti ti_5;"), ["ti_5"]);
        assert_eq!(
            decl_names("static void (*nc_fail_hook)(bool a) = NULL;"),
            ["nc_fail_hook"]
        );
        assert_eq!(
            decl_names("_Noreturn void nc_panic(const char *msg, const char *loc);"),
            ["nc_panic"]
        );
        assert_eq!(
            decl_names(
                "typedef struct nc_ti {\n  size_t size;\n  void (*retain)(void *);\n} nc_ti;"
            ),
            ["nc_ti", "nc_ti"]
        );
    }

    #[test]
    fn prunes_unused() {
        let src = "#include <stdio.h>\n#include <math.h>\n/* used */\nstatic int used(void) { return 1; }\n/* unused */\nstatic int unused(void) { return pow(2, 2); }\nint main(void) {\n  printf(\"%d\", used());\n}\n";
        let out = prune(src, &[]);
        assert!(out.contains("used(void)") && out.contains("/* used */"));
        assert!(!out.contains("unused") && !out.contains("math.h"));
        assert!(out.contains("stdio.h"));
    }
}
