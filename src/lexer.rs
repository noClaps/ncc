use std::ops::Range;

use crate::diagnostic::Diagnostics;

/// Shared integer decoding for values, fixed-array lengths, and tooling.
///
/// # Errors
/// Returns diagnostics if the literal is invalid for its base or overflows `u64`.
pub fn integer(text: &str) -> Result<u64, Diagnostics> {
    let text = text.strip_suffix('u').unwrap_or(text);
    let (digits, base) = if let Some(x) = text.strip_prefix("0x") {
        (x, 16)
    } else if let Some(x) = text.strip_prefix("0b") {
        (x, 2)
    } else if let Some(x) = text.strip_prefix("0o") {
        (x, 8)
    } else {
        (text, 10)
    };
    u64::from_str_radix(digits, base)
        .map_err(|_| Diagnostics::one("invalid or overflowing integer literal", 0..0))
}

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
    Semicolon,
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

/// Tokenize NC source, preserving source spans.
///
/// # Errors
/// Returns diagnostics for unexpected characters, malformed or unterminated quoted
/// literals, invalid escapes, or char literals that are not one grapheme cluster.
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
        let (kind, end) = scan_token(source, start)?;
        i = end;
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

fn scan_token(source: &str, start: usize) -> Result<(TokenKind, usize), Diagnostics> {
    let bytes = source.as_bytes();
    let c = bytes[start] as char;
    if c == '"' || c == '\'' {
        let (value, end) = quoted(source, start, c)?;
        let kind = if c == '"' {
            TokenKind::String(value)
        } else {
            if crate::unicode::boundaries(&value).len() != 1 {
                return Err(Diagnostics::one(
                    "a char literal must contain one Unicode grapheme cluster",
                    start..end,
                ));
            }
            TokenKind::Char(value)
        };
        return Ok((kind, end));
    }
    if c.is_ascii_digit() {
        return Ok(scan_number(source, start));
    }
    if c.is_ascii_alphabetic() || c == '_' {
        let mut i = start + 1;
        while i < bytes.len() && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        let s = &source[start..i];
        let kind = if matches!(s, "NaN" | "inf") {
            TokenKind::Float(s.into())
        } else {
            keyword(s).map_or_else(|| TokenKind::Ident(s.into()), TokenKind::Keyword)
        };
        return Ok((kind, i));
    }
    let (kind, width) = punctuation(source, start)?;
    Ok((kind, start + width))
}

fn scan_number(source: &str, start: usize) -> (TokenKind, usize) {
    let bytes = source.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
        i += 1;
    }
    let kind = if i < bytes.len()
        && bytes[i] == b'.'
        && i + 1 < bytes.len()
        && (bytes[i + 1] as char).is_ascii_digit()
    {
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
            i += 1;
        }
        TokenKind::Float(source[start..i].into())
    } else {
        TokenKind::Int(source[start..i].into())
    };
    (kind, i)
}

fn punctuation(source: &str, start: usize) -> Result<(TokenKind, usize), Diagnostics> {
    let c = source.as_bytes()[start] as char;
    Ok(match &source[start..] {
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
            ';' => (TokenKind::Semicolon, 1),
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
    })
}

fn quoted(source: &str, start: usize, quote: char) -> Result<(String, usize), Diagnostics> {
    let multiline = quote == '"' && source[start..].starts_with("\"\"\"");
    let mut braces = 0usize;
    let mut nested_quote = None;
    let mut nested_escape = false;
    let mut i = start + if multiline { 3 } else { quote.len_utf8() };
    let mut value = String::new();
    let bytes = source.as_bytes();
    while i < bytes.len() {
        if multiline && braces == 0 && source[i..].starts_with("\"\"\"") {
            let closing_line = source[..i].rsplit('\n').next().unwrap();
            let indent = if closing_line
                .bytes()
                .all(|b| b == b' ' || b == b'\t' || b == b'\r')
            {
                closing_line.len()
            } else {
                0
            };
            return Ok((dedent(&value, indent), i + 3));
        }
        let c = source[i..].chars().next().unwrap();
        i += c.len_utf8();
        if braces > 0 {
            value.push(c);
            scan_interpolation(c, &mut braces, &mut nested_quote, &mut nested_escape);
            continue;
        }
        if c == '{' && quote == '"' {
            braces = 1;
            value.push(c);
            continue;
        }
        if c == quote && !multiline {
            return Ok((value, i));
        }
        if c == '\\' {
            let Some(next) = source[i..].chars().next() else {
                break;
            };
            i += next.len_utf8();
            let escaped = decode_escape(source, &mut i, next, quote)?;
            if escaped == '{' && quote == '"' {
                value.push('\\');
            }
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

fn scan_interpolation(
    c: char,
    braces: &mut usize,
    nested_quote: &mut Option<char>,
    nested_escape: &mut bool,
) {
    if *nested_escape {
        *nested_escape = false;
        return;
    }
    if let Some(q) = *nested_quote {
        if c == '\\' {
            *nested_escape = true;
        } else if c == q {
            *nested_quote = None;
        }
    } else {
        match c {
            '\'' | '"' => *nested_quote = Some(c),
            '{' => *braces += 1,
            '}' => *braces -= 1,
            _ => {}
        }
    }
}

fn unicode_escape(source: &str, i: &mut usize) -> Result<char, Diagnostics> {
    let bytes = source.as_bytes();
    let escape_start = *i - 2;
    if !source[*i..].starts_with('{') {
        return Err(Diagnostics::one(
            "Unicode escapes require `\\u{hex}`",
            escape_start..*i,
        ));
    }
    *i += 1;
    let digits = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_hexdigit() {
        *i += 1;
    }
    if *i == digits || *i - digits > 6 || bytes.get(*i) != Some(&b'}') {
        return Err(Diagnostics::one(
            "Unicode escape requires 1 to 6 hex digits and a closing `}`",
            escape_start..*i,
        ));
    }
    let scalar = u32::from_str_radix(&source[digits..*i], 16).unwrap();
    *i += 1;
    let Some(character) = char::from_u32(scalar) else {
        return Err(Diagnostics::one(
            "Unicode escape is not a valid Unicode scalar value",
            escape_start..*i,
        ));
    };
    Ok(character)
}

fn decode_escape(
    source: &str,
    i: &mut usize,
    next: char,
    quote: char,
) -> Result<char, Diagnostics> {
    Ok(match next {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'e' => '\u{1b}',
        'u' => unicode_escape(source, i)?,
        '\\' => '\\',
        '\'' if quote == '\'' => '\'',
        '"' if quote == '"' => '"',
        '{' if quote == '"' => '{',
        other => {
            return Err(Diagnostics::one(
                format!("unknown escape `\\{other}`"),
                *i - 2..*i,
            ));
        }
    })
}

fn dedent(raw: &str, indent: usize) -> String {
    let raw = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);
    let mut result = String::new();
    for line in raw.split_inclusive('\n') {
        let remove = line
            .bytes()
            .take(indent)
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        result.push_str(&line[remove..]);
    }
    // Drop the closing delimiter's indentation, but preserve content newlines.
    if result.ends_with('\r') {
        result.pop();
    }
    result
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
    fn quoted_tokens_keep_raw_byte_spans_and_newlines() {
        let literal = r#""🍪\u{7b}literal} {call("}", '\'')}""#;
        let source = format!("// comment\n{literal}\n'o\\u{{308}}'");
        let tokens = lex(&source).unwrap();
        assert_eq!(&source[tokens[0].span.clone()], literal);
        assert_eq!(
            tokens[0].kind,
            TokenKind::String("🍪\\{literal} {call(\"}\", '\\'')}".into())
        );
        assert!(tokens[0].newline_before);
        assert!(tokens[1].newline_before);
        assert_eq!(tokens[1].kind, TokenKind::Char("o\u{308}".into()));
        assert_eq!(&source[tokens[1].span.clone()], "'o\\u{308}'");
        assert_eq!(tokens[2].span, source.len()..source.len());
    }

    #[test]
    fn escape_errors_keep_original_byte_ranges() {
        for (literal, escape) in [
            (r#""🍪\u{}""#, r"\u{"),
            (r#""🍪\u{D800}""#, r"\u{D800}"),
            (r#""🍪\q""#, r"\q"),
        ] {
            let error = lex(literal).unwrap_err();
            assert_eq!(&literal[error.0[0].span.clone()], escape);
        }
    }

    #[test]
    fn strings() {
        assert_eq!(
            lex("\"a\\nb\"").unwrap()[0].kind,
            TokenKind::String("a\nb".into())
        );
    }
}
