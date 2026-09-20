use std::ops::Range;

use crate::diagnostic::Diagnostics;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub newline_before: bool,
    pub kind: TokenKind,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Int(String),
    Float(String),
    String(String),
    Char(String),
    At,
    Dollar,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    Question,
    Bang,
    Assign,
    Arrow,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Keyword(Keyword),
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Keyword {
    As,
    Assert,
    Async,
    Await,
    Break,
    Catch,
    Continue,
    Else,
    Enum,
    Extern,
    False,
    Fn,
    For,
    Fut,
    If,
    Import,
    In,
    Lock,
    Mutex,
    Mut,
    None,
    Not,
    Or,
    And,
    Pub,
    Return,
    Struct,
    Test,
    Throw,
    True,
    Try,
    Type,
    While,
}

fn keyword(s: &str) -> Option<Keyword> {
    Some(match s {
        "as" => Keyword::As,
        "assert" => Keyword::Assert,
        "async" => Keyword::Async,
        "await" => Keyword::Await,
        "break" => Keyword::Break,
        "catch" => Keyword::Catch,
        "continue" => Keyword::Continue,
        "else" => Keyword::Else,
        "enum" => Keyword::Enum,
        "extern" => Keyword::Extern,
        "false" => Keyword::False,
        "fn" => Keyword::Fn,
        "for" => Keyword::For,
        "fut" => Keyword::Fut,
        "if" => Keyword::If,
        "import" => Keyword::Import,
        "in" => Keyword::In,
        "lock" => Keyword::Lock,
        "mutex" => Keyword::Mutex,
        "mut" => Keyword::Mut,
        "none" => Keyword::None,
        "not" => Keyword::Not,
        "or" => Keyword::Or,
        "and" => Keyword::And,
        "pub" => Keyword::Pub,
        "return" => Keyword::Return,
        "struct" => Keyword::Struct,
        "test" => Keyword::Test,
        "throw" => Keyword::Throw,
        "true" => Keyword::True,
        "try" => Keyword::Try,
        "type" => Keyword::Type,
        "while" => Keyword::While,
        _ => return None,
    })
}

pub fn lex(source: &str) -> Result<Vec<Token>, Diagnostics> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = source.as_bytes();
    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if source[i..].starts_with("//") {
            i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if source[i..].starts_with("\"\"\"") {
            i += 3;
            let body_start = i;
            let Some(end) = source[i..].find("\"\"\"") else {
                return Err(Diagnostics::one(
                    "unterminated multiline string",
                    start..bytes.len(),
                ));
            };
            i += end;
            let raw = &source[body_start..i];
            i += 3;
            out.push(Token {
                newline_before: false,
                kind: TokenKind::String(dedent(raw)),
                span: start..i,
            });
            continue;
        }
        if c == '"' {
            let (value, end) = quoted(source, i, '"')?;
            i = end;
            out.push(Token {
                newline_before: false,
                kind: TokenKind::String(value),
                span: start..i,
            });
            continue;
        }
        if c == '\'' {
            let (value, end) = quoted(source, i, '\'')?;
            if crate::unicode::boundaries(&value).len() != 1 {
                return Err(Diagnostics::one(
                    "a char literal must contain one Unicode grapheme cluster",
                    start..end,
                ));
            }
            i = end;
            out.push(Token {
                newline_before: false,
                kind: TokenKind::Char(value),
                span: start..i,
            });
            continue;
        }
        if c.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
                i += 1;
            }
            if i < bytes.len()
                && bytes[i] == b'.'
                && i + 1 < bytes.len()
                && (bytes[i + 1] as char).is_ascii_digit()
            {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                    i += 1;
                }
                out.push(Token {
                    newline_before: false,
                    kind: TokenKind::Float(source[start..i].into()),
                    span: start..i,
                });
            } else {
                out.push(Token {
                    newline_before: false,
                    kind: TokenKind::Int(source[start..i].into()),
                    span: start..i,
                });
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            i += 1;
            while i < bytes.len()
                && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let s = &source[start..i];
            out.push(Token {
                newline_before: false,
                kind: keyword(s)
                    .map(TokenKind::Keyword)
                    .unwrap_or_else(|| TokenKind::Ident(s.into())),
                span: start..i,
            });
            continue;
        }
        let (kind, width) = match &source[i..] {
            s if s.starts_with("->") => (TokenKind::Arrow, 2),
            s if s.starts_with("**") => (TokenKind::Power, 2),
            s if s.starts_with("<>") => (TokenKind::Concat, 2),
            s if s.starts_with("==") => (TokenKind::Eq, 2),
            s if s.starts_with("!=") => (TokenKind::Ne, 2),
            s if s.starts_with("<=") => (TokenKind::Le, 2),
            s if s.starts_with(">=") => (TokenKind::Ge, 2),
            s if s.starts_with("<<") => (TokenKind::Shl, 2),
            s if s.starts_with(">>") => (TokenKind::Shr, 2),
            _ => match c {
                '@' => (TokenKind::At, 1),
                '$' => (TokenKind::Dollar, 1),
                '(' => (TokenKind::LParen, 1),
                ')' => (TokenKind::RParen, 1),
                '{' => (TokenKind::LBrace, 1),
                '}' => (TokenKind::RBrace, 1),
                '[' => (TokenKind::LBracket, 1),
                ']' => (TokenKind::RBracket, 1),
                ',' => (TokenKind::Comma, 1),
                ':' => (TokenKind::Colon, 1),
                '.' => (TokenKind::Dot, 1),
                '?' => (TokenKind::Question, 1),
                '!' => (TokenKind::Bang, 1),
                '=' => (TokenKind::Assign, 1),
                '+' => (TokenKind::Plus, 1),
                '-' => (TokenKind::Minus, 1),
                '*' => (TokenKind::Star, 1),
                '/' => (TokenKind::Slash, 1),
                '%' => (TokenKind::Percent, 1),
                '<' => (TokenKind::Lt, 1),
                '>' => (TokenKind::Gt, 1),
                '&' => (TokenKind::BitAnd, 1),
                '|' => (TokenKind::BitOr, 1),
                '^' => (TokenKind::BitXor, 1),
                _ => {
                    return Err(Diagnostics::one(
                        format!("unexpected character `{c}`"),
                        start..start + c.len_utf8(),
                    ));
                }
            },
        };
        i += width;
        out.push(Token {
            newline_before: false,
            kind,
            span: start..i,
        });
    }
    out.push(Token {
        newline_before: false,
        kind: TokenKind::Eof,
        span: bytes.len()..bytes.len(),
    });
    let mut previous = 0;
    for token in &mut out {
        token.newline_before = source[previous..token.span.start].contains('\n');
        previous = token.span.end;
    }
    Ok(out)
}

fn quoted(source: &str, start: usize, quote: char) -> Result<(String, usize), Diagnostics> {
    let mut braces = 0usize;
    let mut nested_quote = None;
    let mut nested_escape = false;
    let mut i = start + quote.len_utf8();
    let mut value = String::new();
    let bytes = source.as_bytes();
    while i < bytes.len() {
        let c = source[i..].chars().next().unwrap();
        i += c.len_utf8();
        if braces > 0 {
            value.push(c);
            if nested_escape {
                nested_escape = false;
                continue;
            }
            if let Some(q) = nested_quote {
                if c == '\\' {
                    nested_escape = true;
                } else if c == q {
                    nested_quote = None;
                }
            } else {
                match c {
                    '\'' | '"' => nested_quote = Some(c),
                    '{' => braces += 1,
                    '}' => braces -= 1,
                    _ => {}
                }
            }
            continue;
        }
        if c == '{' && quote == '"' {
            braces = 1;
            value.push(c);
            continue;
        }
        if c == quote {
            return Ok((value, i));
        }
        if c == '\\' {
            let Some(next) = source[i..].chars().next() else {
                break;
            };
            i += next.len_utf8();
            let escaped = match next {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                '{' => {
                    value.push('\\');
                    '{'
                }
                other => {
                    return Err(Diagnostics::one(
                        format!("unknown escape `\\{other}`"),
                        i - 2..i,
                    ));
                }
            };
            value.push(escaped);
        } else {
            value.push(c);
        }
    }
    Err(Diagnostics::one(
        "unterminated string literal",
        start..bytes.len(),
    ))
}

fn dedent(raw: &str) -> String {
    let raw = raw.strip_prefix('\n').unwrap_or(raw);
    let indent = raw
        .lines()
        .last()
        .map(|s| s.len() - s.trim_start().len())
        .unwrap_or(0);
    raw.lines()
        .map(|line| line.get(indent.min(line.len())..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end_matches('\n')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comments_and_operators() {
        let x = lex("int a = 0xF // hi\na <> b").unwrap();
        assert!(matches!(x[0].kind, TokenKind::Ident(_)));
        assert_eq!(x[5].kind, TokenKind::Concat);
    }
    #[test]
    fn strings() {
        assert_eq!(
            lex("\"a\\nb\"").unwrap()[0].kind,
            TokenKind::String("a\nb".into())
        );
    }
}
