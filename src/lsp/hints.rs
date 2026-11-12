//! Inlay hints and semantic tokens.

use std::collections::{HashMap, HashSet};

use lsp_types::*;

use super::{KEYWORDS, Positions, Server, offset_to_pos, pos_to_offset};
use crate::ast;
use crate::check::index::{DeclKind, HintKind};
use crate::lexer::{StrPart, Tok};

/// Blocks at least this many lines long get a hint after their closing brace.
const CLOSING_HINT_LINES: u32 = 25;

const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::TYPE,
    SemanticTokenType::STRUCT,
    SemanticTokenType::ENUM,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::KEYWORD,
    SemanticTokenType::COMMENT,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    // not in the LSP's predefined set; rust-analyzer uses the same name
    SemanticTokenType::new("boolean"),
];

const NAMESPACE: u32 = 0;
const TYPE: u32 = 1;
const STRUCT: u32 = 2;
const ENUM: u32 = 3;
const ENUM_MEMBER: u32 = 4;
const FUNCTION: u32 = 5;
const PARAMETER: u32 = 6;
const VARIABLE: u32 = 7;
const PROPERTY: u32 = 8;
const KEYWORD: u32 = 9;
const COMMENT: u32 = 10;
const STRING: u32 = 11;
const NUMBER: u32 = 12;
const BOOLEAN: u32 = 13;

const MODIFIERS: &[&str] = &[
    "declaration",
    "readonly",
    "defaultLibrary",
    "documentation",
    "mutable",
];
const M_DECLARATION: u32 = 1;
const M_READONLY: u32 = 2;
const M_DEFAULT_LIBRARY: u32 = 4;
const M_DOCUMENTATION: u32 = 8;
const M_MUTABLE: u32 = 16;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: MODIFIERS
            .iter()
            .map(|m| SemanticTokenModifier::new(m))
            .collect(),
    }
}

impl Server {
    pub(super) fn inlay_hints(&self, p: InlayHintParams) -> Option<Vec<InlayHint>> {
        let (a, fid, text) = self.fresh(&p.text_document.uri)?;
        let path = super::uri_to_path(&p.text_document.uri)?;
        let (rs, re) = (
            pos_to_offset(text, p.range.start) as u32,
            pos_to_offset(text, p.range.end) as u32,
        );
        // generic function bodies are checked once per instantiation: a hint whose label
        // differs between instantiations would be misleading, so it is left out
        let mut labels: HashMap<(u32, HintKind), HashSet<&str>> = HashMap::new();
        for h in &a.index.hints {
            if h.at.file == fid && rs <= h.at.start && h.at.start <= re {
                labels
                    .entry((h.at.start, h.kind))
                    .or_default()
                    .insert(&h.label);
            }
        }
        let mut out: Vec<InlayHint> = labels
            .into_iter()
            .filter(|(_, ls)| ls.len() == 1)
            .map(|((at, kind), ls)| {
                let label = ls.into_iter().next().unwrap().trim_end().to_string();
                let kind = match kind {
                    HintKind::Type => InlayHintKind::TYPE,
                    HintKind::Param => InlayHintKind::PARAMETER,
                };
                hint(offset_to_pos(text, at as usize), label, Some(kind))
            })
            .collect();
        // `} // fn name` after long blocks
        let module = self.module_of(&path, text);
        let mut closing = |label: String, body: crate::diag::Span| {
            let (s, e) = (body.start as usize, body.end as usize);
            if e == 0 || e > text.len() || !text[..e].ends_with('}') {
                return;
            }
            let lines = offset_to_pos(text, e).line - offset_to_pos(text, s).line;
            if lines >= CLOSING_HINT_LINES && rs as usize <= e && e <= re as usize {
                out.push(hint(offset_to_pos(text, e), label, None));
            }
        };
        for it in &module.items {
            match it {
                ast::Item::Fn(f) => closing(format!("fn {}", f.sig.name), f.body.span),
                ast::Item::Struct(s) => closing(format!("struct {}", s.name), s.span),
                ast::Item::Enum(e) => closing(format!("enum {}", e.name), e.span),
                ast::Item::Test(t) => closing(format!("test \"{}\"", t.name), t.body.span),
                _ => {}
            }
        }
        out.sort_by_key(|h| h.position);
        Some(out)
    }

    pub(super) fn semantic_tokens(
        &self,
        uri: &Uri,
        range: Option<Range>,
    ) -> Option<SemanticTokensResult> {
        let (path, text) = self.source(uri)?;
        // identifiers resolved by the last analysis (only usable when it saw this text)
        let mut resolved: HashMap<u32, (u32, u32)> = HashMap::new();
        if let Some((a, fid, _)) = self.fresh(uri) {
            for r in &a.index.refs {
                if r.span.file != fid {
                    continue;
                }
                let Some(def) = r.def else { continue };
                let Some(d) = a.decl(def) else { continue };
                let ty = match d.kind {
                    DeclKind::Variable if d.param => PARAMETER,
                    DeclKind::Variable => VARIABLE,
                    DeclKind::Function => FUNCTION,
                    DeclKind::Struct => STRUCT,
                    DeclKind::Enum => ENUM,
                    DeclKind::Type => TYPE,
                    DeclKind::Field => PROPERTY,
                    DeclKind::Variant => ENUM_MEMBER,
                };
                let mut mods = 0;
                if d.kind == DeclKind::Variable {
                    mods |= if d.mutable { M_MUTABLE } else { M_READONLY };
                }
                if r.span == def {
                    mods |= M_DECLARATION;
                }
                let ns = r.name_span;
                if text.get(ns.start as usize..ns.end as usize) != Some(d.name.as_str()) {
                    continue;
                }
                // the smallest reference wins (a field inside a larger expression)
                resolved.entry(ns.start).or_insert((ty, mods));
            }
        }
        let module = self.module_of(&path, &text);
        let aliases: HashSet<&str> = (module.items.iter())
            .filter_map(|it| match it {
                ast::Item::Import(im) => Some(im.alias.as_str()),
                ast::Item::Extern(e) => Some(e.alias.as_str()),
                _ => None,
            })
            .collect();
        let mut toks: Vec<(usize, usize, u32, u32)> = Vec::new();
        let (lexed, _) = crate::lexer::lex_all(&text, 0, 0);
        classify(&text, &lexed, &resolved, &aliases, &mut toks);
        for (s, e) in crate::lexer::comments(&text) {
            if text[s..e].starts_with("///") {
                toks.push((s, e, COMMENT, M_DOCUMENTATION));
            }
        }
        toks.sort_by_key(|t| t.0);
        let (rs, re) = match range {
            Some(r) => (pos_to_offset(&text, r.start), pos_to_offset(&text, r.end)),
            None => (0, text.len()),
        };
        let mut data = Vec::new();
        let mut positions = Positions::new(&text);
        let (mut pl, mut pc) = (0u32, 0u32);
        for (s, e, ty, mods) in toks {
            if e < rs || s > re {
                continue;
            }
            // tokens may not span lines
            let mut start = s;
            for piece in text[s..e].split_inclusive('\n') {
                let len = piece.trim_end_matches('\n').encode_utf16().count() as u32;
                let pos = positions.at(start);
                start += piece.len();
                if len == 0 {
                    continue;
                }
                let dl = pos.line - pl;
                let dc = if dl == 0 {
                    pos.character - pc
                } else {
                    pos.character
                };
                data.push(SemanticToken {
                    delta_line: dl,
                    delta_start: dc,
                    length: len,
                    token_type: ty,
                    token_modifiers_bitset: mods,
                });
                pl = pos.line;
                pc = pos.character;
            }
        }
        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }
}

/// An inlay hint: of a kind before what it is about, of none after a closing brace.
fn hint(position: Position, label: String, kind: Option<InlayHintKind>) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(label),
        padding_left: kind.is_none().then_some(true),
        padding_right: kind.is_some().then_some(true),
        kind,
        text_edits: None,
        tooltip: None,
        data: None,
    }
}

/// Classify lexed tokens as (start, end, type, modifiers).
fn classify(
    text: &str,
    lexed: &[crate::lexer::Token],
    resolved: &HashMap<u32, (u32, u32)>,
    aliases: &HashSet<&str>,
    out: &mut Vec<(usize, usize, u32, u32)>,
) {
    for (i, t) in lexed.iter().enumerate() {
        let (s, e) = (t.span.start as usize, t.span.end as usize);
        match &t.tok {
            Tok::Ident(name) => {
                if let Some(&(ty, mods)) = resolved.get(&t.span.start) {
                    out.push((s, e, ty, mods));
                } else if name == "true" || name == "false" {
                    out.push((s, e, BOOLEAN, 0));
                } else if name == "none" {
                    // like Rust's `None`: a built-in variant
                    out.push((s, e, ENUM_MEMBER, M_DEFAULT_LIBRARY));
                } else if KEYWORDS.contains(&name.as_str()) {
                    out.push((s, e, KEYWORD, 0));
                } else if crate::types::builtin_type(name).is_some() {
                    out.push((s, e, TYPE, M_DEFAULT_LIBRARY));
                } else if aliases.contains(name.as_str()) {
                    let decl = i > 0 && matches!(&lexed[i - 1].tok, Tok::Ident(k) if k == "as");
                    let used = matches!(lexed.get(i + 1).map(|t| &t.tok), Some(Tok::Dot));
                    if decl || used {
                        out.push((s, e, NAMESPACE, if decl { M_DECLARATION } else { 0 }));
                    }
                }
            }
            Tok::Builtin(_) => out.push((s, e, FUNCTION, M_DEFAULT_LIBRARY)),
            Tok::Int(..) | Tok::Float(_) => out.push((s, e, NUMBER, 0)),
            Tok::Char(_) => out.push((s, e, STRING, 0)),
            Tok::Str(parts) => {
                // string pieces around interpolations; the interpolated code is classified too
                let mut at = s;
                for p in parts {
                    if let StrPart::Expr(src, off) = p {
                        let open = text[..*off].rfind('{').unwrap_or(*off);
                        if open > at {
                            out.push((at, open + 1, STRING, 0));
                        }
                        let (inner, _) = crate::lexer::lex_all(src, 0, *off);
                        classify(text, &inner, resolved, aliases, out);
                        at = off + src.len();
                    }
                }
                if e > at {
                    out.push((at, e, STRING, 0));
                }
            }
            _ => {}
        }
    }
}
