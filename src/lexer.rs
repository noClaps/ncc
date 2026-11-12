//! Tokenizer for NC source text.

use crate::diag::{Diag, Result, Span, err};

#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    Lit(String),
    /// Interpolated expression: its source text and the byte offset of that text in the file.
    Expr(String, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    /// Integer literal value and whether it had the `u` suffix.
    Int(u128, bool),
    Float(f64),
    Char(u32),
    Str(Vec<StrPart>),
    /// `@name` builtin
    Builtin(String),
    Newline,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Colon,
    Question,
    Bang,
    Dollar,
    Assign,
    Arrow,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Percent,
    Shl,
    Shr,
    Amp,
    Pipe,
    Caret,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Self::Ident(s) => format!("`{s}`"),
            Self::Int(..) => "integer literal".into(),
            Self::Float(_) => "float literal".into(),
            Self::Char(_) => "char literal".into(),
            Self::Str(_) => "string literal".into(),
            Self::Builtin(s) => format!("`@{s}`"),
            Self::Newline => "newline".into(),
            Self::Eof => "end of file".into(),
            t => {
                let (text, _) = PUNCT.iter().find(|(_, p)| p == t).unwrap();
                format!("`{text}`")
            }
        }
    }
}

/// Punctuation tokens and their text, two-character ones first.
const PUNCT: &[(&str, Tok)] = &[
    ("**", Tok::StarStar),
    ("<<", Tok::Shl),
    (">>", Tok::Shr),
    ("<>", Tok::Concat),
    ("<=", Tok::Le),
    (">=", Tok::Ge),
    ("==", Tok::Eq),
    ("!=", Tok::Ne),
    ("->", Tok::Arrow),
    ("(", Tok::LParen),
    (")", Tok::RParen),
    ("[", Tok::LBracket),
    ("]", Tok::RBracket),
    ("{", Tok::LBrace),
    ("}", Tok::RBrace),
    (",", Tok::Comma),
    (".", Tok::Dot),
    (":", Tok::Colon),
    ("?", Tok::Question),
    ("!", Tok::Bang),
    ("$", Tok::Dollar),
    ("=", Tok::Assign),
    ("+", Tok::Plus),
    ("-", Tok::Minus),
    ("*", Tok::Star),
    ("/", Tok::Slash),
    ("%", Tok::Percent),
    ("&", Tok::Amp),
    ("|", Tok::Pipe),
    ("^", Tok::Caret),
    ("<", Tok::Lt),
    (">", Tok::Gt),
];

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// Offset added to all spans (for interpolated sub-expressions).
    base: usize,
    file: u32,
    /// byte ranges of `//` comments seen so far
    comments: Vec<(usize, usize)>,
}

pub fn lex(src: &str, file: u32, base: usize) -> Result<Vec<Token>> {
    let (toks, mut errors) = lex_all(src, file, base);
    if errors.is_empty() {
        Ok(toks)
    } else {
        Err(errors.remove(0))
    }
}

/// Byte ranges of all `//` comments in the source (for the formatter).
pub fn comments(src: &str) -> Vec<(usize, usize)> {
    run(src, 0, 0).2
}

/// Tokenize everything, recovering from errors by skipping the offending character.
pub fn lex_all(src: &str, file: u32, base: usize) -> (Vec<Token>, Vec<Diag>) {
    let (toks, errors, _) = run(src, file, base);
    (toks, errors)
}

/// Tokens, errors and comment ranges of the whole source.
fn run(src: &str, file: u32, base: usize) -> (Vec<Token>, Vec<Diag>, Vec<(usize, usize)>) {
    let mut lx = Lexer::new(src, file, base, 0);
    let mut out = Vec::new();
    let mut errors = Vec::new();
    loop {
        let start = lx.pos;
        match lx.next() {
            Ok(t) => {
                let eof = t.tok == Tok::Eof;
                out.push(t);
                if eof {
                    break;
                }
            }
            Err(d) => {
                errors.push(d);
                if lx.pos <= start {
                    // skip one character to make progress
                    lx.pos = start + src[start..].chars().next().map_or(1, char::len_utf8);
                }
            }
        }
    }
    (out, errors, lx.comments)
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, file: u32, base: usize, pos: usize) -> Self {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos,
            base,
            file,
            comments: vec![],
        }
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, self.base + start, self.base + self.pos)
    }

    fn peek(&self) -> u8 {
        self.peek_at(0)
    }
    fn peek_at(&self, n: usize) -> u8 {
        *self.bytes.get(self.pos + n).unwrap_or(&0)
    }

    /// Advance past the character at `self.pos`.
    fn bump_char(&mut self) -> char {
        let ch = self.src[self.pos..].chars().next().unwrap();
        self.pos += ch.len_utf8();
        ch
    }

    /// Advance past the bytes matching `f`; returns where they start.
    fn skip_while(&mut self, f: impl Fn(u8) -> bool) -> usize {
        let start = self.pos;
        while self.pos < self.bytes.len() && f(self.bytes[self.pos]) {
            self.pos += 1;
        }
        start
    }

    fn next(&mut self) -> Result<Token> {
        // skip spaces and comments
        loop {
            let c = self.peek();
            if c == b' ' || c == b'\t' || c == b'\r' {
                self.pos += 1;
            } else if c == b'/' && self.peek_at(1) == b'/' {
                let start = self.skip_while(|c| c != b'\n');
                self.comments
                    .push((self.base + start, self.base + self.pos));
            } else {
                break;
            }
        }
        let start = self.pos;
        if self.pos >= self.bytes.len() {
            return Ok(Token {
                tok: Tok::Eof,
                span: self.span(start),
            });
        }
        let c = self.peek();
        let tok = match c {
            b'\n' => {
                self.pos += 1;
                Tok::Newline
            }
            b'0'..=b'9' => self.number(start)?,
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                self.skip_while(is_ident_byte);
                Tok::Ident(self.src[start..self.pos].to_string())
            }
            b'@' => {
                self.pos += 1;
                let s = self.skip_while(is_ident_byte);
                if s == self.pos {
                    return err(self.span(start), "expected builtin name after `@`");
                }
                Tok::Builtin(self.src[s..self.pos].to_string())
            }
            b'\'' => self.char_lit(start)?,
            b'"' => self.string(start)?,
            _ => {
                let rest = &self.src[self.pos..];
                let Some((text, t)) = PUNCT.iter().find(|(text, _)| rest.starts_with(text)) else {
                    let ch = self.bump_char();
                    return err(self.span(start), format!("unexpected character `{ch}`"));
                };
                self.pos += text.len();
                t.clone()
            }
        };
        Ok(Token {
            tok,
            span: self.span(start),
        })
    }

    fn number(&mut self, start: usize) -> Result<Tok> {
        let radix = if self.peek() == b'0' {
            match self.peek_at(1) {
                b'x' | b'X' => 16,
                b'o' => 8,
                b'b' => 2,
                _ => 10,
            }
        } else {
            10
        };
        if radix != 10 {
            self.pos += 2;
            let s = self.skip_while(is_ident_byte);
            let mut digits: String = self.src[s..self.pos].replace('_', "");
            // a trailing `u` suffix (`u` is never a digit in any of these radixes)
            let unsigned = digits.ends_with('u');
            if unsigned {
                digits.pop();
            }
            if digits.is_empty() {
                return err(self.span(start), "expected digits after number prefix");
            }
            let v = u128::from_str_radix(&digits, radix)
                .map_err(|_| Diag::new(self.span(start), "invalid number literal"))?;
            return Ok(Tok::Int(v, unsigned));
        }
        self.skip_while(digit_char);
        let is_float = self.peek() == b'.' && self.peek_at(1).is_ascii_digit();
        if is_float {
            self.pos += 1;
            self.skip_while(digit_char);
        }
        let mant_end = self.pos;
        let mut exp: Option<i64> = None;
        if matches!(self.peek(), b'e' | b'E') {
            let k = if matches!(self.peek_at(1), b'+' | b'-') {
                2
            } else {
                1
            };
            if self.peek_at(k).is_ascii_digit() {
                let es = self.pos + 1;
                self.pos += k;
                self.skip_while(digit_char);
                let etext = self.src[es..self.pos].replace('_', "");
                exp = Some(
                    etext
                        .parse()
                        .map_err(|_| Diag::new(self.span(start), "exponent too large"))?,
                );
            }
        }
        if is_float || exp.is_some_and(|e| e < 0) {
            let text = self.src[start..self.pos].replace('_', "");
            let v: f64 = text.parse().unwrap();
            if self.peek().is_ascii_alphanumeric() {
                return err(self.span(start), "invalid number literal");
            }
            return Ok(Tok::Float(v));
        }
        let unsigned = self.peek() == b'u' && !self.peek_at(1).is_ascii_alphanumeric();
        self.pos += usize::from(unsigned);
        if self.peek().is_ascii_alphanumeric() {
            return err(self.span(start), "invalid number literal");
        }
        // an integer, exact with a non-negative exponent
        let too_large = || Diag::new(self.span(start), "integer literal too large");
        let mut v: u128 = self.src[start..mant_end]
            .replace('_', "")
            .parse()
            .map_err(|_| too_large())?;
        for _ in 0..exp.unwrap_or(0) {
            v = v.checked_mul(10).ok_or_else(too_large)?;
        }
        Ok(Tok::Int(v, unsigned))
    }

    /// Parse one escape sequence after a backslash at `self.pos` (pointing to the char after `\`).
    fn escape(&mut self, start: usize, in_char: bool) -> Result<char> {
        let c = self.peek();
        self.pos += 1;
        Ok(match c {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'e' => '\u{1b}',
            b'\\' => '\\',
            b'\'' if in_char => '\'',
            b'"' if !in_char => '"',
            b'{' if !in_char => '{',
            b'u' => {
                if self.peek() != b'{' {
                    return err(self.span(start), "expected `{` after `\\u`");
                }
                self.pos += 1;
                let s = self.skip_while(|c| c.is_ascii_hexdigit());
                let digits = &self.src[s..self.pos];
                if self.peek() != b'}' || digits.is_empty() || digits.len() > 6 {
                    return err(
                        self.span(start),
                        "invalid unicode escape, expected `\\u{XXXXXX}`",
                    );
                }
                self.pos += 1;
                let v = u32::from_str_radix(digits, 16).unwrap();
                char::from_u32(v).ok_or_else(|| {
                    Diag::new(
                        self.span(start),
                        format!("`{digits}` is not a valid unicode character"),
                    )
                })?
            }
            _ => {
                return err(self.span(start), "invalid escape sequence");
            }
        })
    }

    fn char_lit(&mut self, start: usize) -> Result<Tok> {
        self.pos += 1;
        let c = match self.peek() {
            b'\\' => {
                self.pos += 1;
                self.escape(start, true)?
            }
            b'\'' | b'\n' | 0 => {
                return err(self.span(start), "empty or unterminated char literal");
            }
            _ => self.bump_char(),
        };
        if self.peek() != b'\'' {
            return err(
                self.span(start),
                "char literal must contain exactly one character",
            );
        }
        self.pos += 1;
        Ok(Tok::Char(c as u32))
    }

    fn string(&mut self, start: usize) -> Result<Tok> {
        if self.src[self.pos..].starts_with("\"\"\"") {
            return self.multiline(start);
        }
        self.pos += 1;
        let mut parts = Vec::new();
        let mut cur = String::new();
        loop {
            match self.peek() {
                0 | b'\n' => return err(self.span(start), "unterminated string literal"),
                b'"' => {
                    self.pos += 1;
                    break;
                }
                _ => self.str_piece(start, &mut cur, &mut parts)?,
            }
        }
        Ok(str_tok(cur, parts))
    }

    /// One piece of a string's contents at `self.pos`: an escape, an interpolation or a
    /// character.
    fn str_piece(
        &mut self,
        start: usize,
        cur: &mut String,
        parts: &mut Vec<StrPart>,
    ) -> Result<()> {
        match self.peek() {
            b'\\' => {
                self.pos += 1;
                cur.push(self.escape(start, false)?);
            }
            b'{' => {
                if !cur.is_empty() {
                    parts.push(StrPart::Lit(std::mem::take(cur)));
                }
                let (text, off) = self.interp(start)?;
                parts.push(StrPart::Expr(text, self.base + off));
            }
            _ => {
                cur.push(self.bump_char());
            }
        }
        Ok(())
    }

    /// Scan an interpolation `{ ... }` starting at `{`; returns inner text and its offset.
    fn interp(&mut self, start: usize) -> Result<(String, usize)> {
        self.pos += 1;
        let s = self.pos;
        let mut depth = 0usize;
        loop {
            match self.peek() {
                0 => return err(self.span(start), "unterminated interpolation in string"),
                b'{' => depth += 1,
                b'}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                // a string or char literal inside the interpolation
                q @ (b'"' | b'\'') => {
                    self.pos += 1;
                    while self.peek() != q {
                        if self.peek() == 0 || self.peek() == b'\n' {
                            let what = if q == b'"' { "string" } else { "char" };
                            return err(self.span(start), format!("unterminated {what} literal"));
                        }
                        if self.peek() == b'\\' {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        let text = self.src[s..self.pos].to_string();
        self.pos += 1;
        if text.trim().is_empty() {
            return err(
                self.span(start),
                "empty interpolation `{}` in string; use `\\{` for a literal brace",
            );
        }
        Ok((text, s))
    }

    fn multiline(&mut self, start: usize) -> Result<Tok> {
        self.pos += 3;
        let body_start = self.pos;
        let Some(rel_end) = self.src[self.pos..].find("\"\"\"") else {
            return err(self.span(start), "unterminated multiline string");
        };
        let raw = &self.src[body_start..body_start + rel_end];
        self.pos = body_start + rel_end + 3;
        // closing indentation: whitespace between the last newline and the closing quotes
        let mut lines: Vec<(usize, &str)> = Vec::new(); // (offset in raw, line)
        let mut off = 0;
        for l in raw.split('\n') {
            lines.push((off, l));
            off += l.len() + 1;
        }
        // drop first line if it's empty (newline right after opening quotes)
        if lines.len() > 1 && lines[0].1.trim().is_empty() {
            lines.remove(0);
        }
        let mut indent = 0;
        if lines.len() > 1 {
            let last = lines.last().unwrap().1;
            if last.chars().all(|c| c == ' ' || c == '\t') {
                indent = last.len();
                lines.pop();
            }
        }
        // rebuild the dedented text, keeping track of offsets for interpolations
        let mut parts = Vec::new();
        let mut cur = String::new();
        for (i, (loff, line)) in lines.iter().enumerate() {
            if i > 0 {
                cur.push('\n');
            }
            let strip = line
                .bytes()
                .take(indent)
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count();
            let content_off = body_start + loff + strip;
            let content = &line[strip..];
            // scan content for escapes and interpolations
            let mut sub = Lexer::new(self.src, self.file, self.base, content_off);
            let end = content_off + content.len();
            while sub.pos < end {
                sub.str_piece(start, &mut cur, &mut parts)?;
                if sub.pos > end {
                    return err(self.span(start), "interpolation cannot span lines");
                }
            }
        }
        Ok(str_tok(cur, parts))
    }
}

/// A string token from its parts and the literal text after the last one.
fn str_tok(cur: String, mut parts: Vec<StrPart>) -> Tok {
    if !cur.is_empty() || parts.is_empty() {
        parts.push(StrPart::Lit(cur));
    }
    Tok::Str(parts)
}

/// A byte of an identifier: an ASCII letter, digit or `_`.
pub fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// A decimal digit or `_` separator.
fn digit_char(c: u8) -> bool {
    c.is_ascii_digit() || c == b'_'
}
