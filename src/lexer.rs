//! Tokenizer for NC source text.

use crate::diag::{Result, Span, err};

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
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Int(..) => "integer literal".into(),
            Tok::Float(_) => "float literal".into(),
            Tok::Char(_) => "char literal".into(),
            Tok::Str(_) => "string literal".into(),
            Tok::Builtin(s) => format!("`@{s}`"),
            Tok::Newline => "newline".into(),
            Tok::Eof => "end of file".into(),
            t => format!("`{}`", punct(t)),
        }
    }
}

fn punct(t: &Tok) -> &'static str {
    match t {
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::Comma => ",",
        Tok::Dot => ".",
        Tok::Colon => ":",
        Tok::Question => "?",
        Tok::Bang => "!",
        Tok::Dollar => "$",
        Tok::Assign => "=",
        Tok::Arrow => "->",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::StarStar => "**",
        Tok::Slash => "/",
        Tok::Percent => "%",
        Tok::Shl => "<<",
        Tok::Shr => ">>",
        Tok::Amp => "&",
        Tok::Pipe => "|",
        Tok::Caret => "^",
        Tok::Concat => "<>",
        Tok::Eq => "==",
        Tok::Ne => "!=",
        Tok::Lt => "<",
        Tok::Gt => ">",
        Tok::Le => "<=",
        Tok::Ge => ">=",
        _ => "?",
    }
}

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
    let mut lx = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        base: 0,
        file: 0,
        comments: vec![],
    };
    loop {
        let start = lx.pos;
        match lx.next() {
            Ok(t) if t.tok == Tok::Eof => break,
            Ok(_) => {}
            Err(_) => {
                if lx.pos <= start {
                    lx.pos = start
                        + src[start..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(1);
                }
            }
        }
    }
    lx.comments
}

/// Tokenize everything, recovering from errors by skipping the offending character.
pub fn lex_all(src: &str, file: u32, base: usize) -> (Vec<Token>, Vec<crate::diag::Diag>) {
    let mut lx = Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        base,
        file,
        comments: vec![],
    };
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
                    let ch = src[start..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    lx.pos = start + ch;
                }
            }
        }
    }
    (out, errors)
}

impl<'a> Lexer<'a> {
    fn span(&self, start: usize) -> Span {
        Span::new(self.file, self.base + start, self.base + self.pos)
    }

    fn peek(&self) -> u8 {
        *self.bytes.get(self.pos).unwrap_or(&0)
    }
    fn peek_at(&self, n: usize) -> u8 {
        *self.bytes.get(self.pos + n).unwrap_or(&0)
    }

    fn next(&mut self) -> Result<Token> {
        // skip spaces and comments
        loop {
            let c = self.peek();
            if c == b' ' || c == b'\t' || c == b'\r' {
                self.pos += 1;
            } else if c == b'/' && self.peek_at(1) == b'/' {
                let start = self.pos;
                while self.pos < self.bytes.len() && self.peek() != b'\n' {
                    self.pos += 1;
                }
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
                while matches!(self.peek(), b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9') {
                    self.pos += 1;
                }
                Tok::Ident(self.src[start..self.pos].to_string())
            }
            b'@' => {
                self.pos += 1;
                let s = self.pos;
                while matches!(self.peek(), b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9') {
                    self.pos += 1;
                }
                if s == self.pos {
                    return err(self.span(start), "expected builtin name after `@`");
                }
                Tok::Builtin(self.src[s..self.pos].to_string())
            }
            b'\'' => self.char_lit(start)?,
            b'"' => self.string(start)?,
            _ => {
                let two = (c, self.peek_at(1));
                let (t, n) = match two {
                    (b'*', b'*') => (Tok::StarStar, 2),
                    (b'<', b'<') => (Tok::Shl, 2),
                    (b'>', b'>') => (Tok::Shr, 2),
                    (b'<', b'>') => (Tok::Concat, 2),
                    (b'<', b'=') => (Tok::Le, 2),
                    (b'>', b'=') => (Tok::Ge, 2),
                    (b'=', b'=') => (Tok::Eq, 2),
                    (b'!', b'=') => (Tok::Ne, 2),
                    (b'-', b'>') => (Tok::Arrow, 2),
                    _ => {
                        let t = match c {
                            b'(' => Tok::LParen,
                            b')' => Tok::RParen,
                            b'[' => Tok::LBracket,
                            b']' => Tok::RBracket,
                            b'{' => Tok::LBrace,
                            b'}' => Tok::RBrace,
                            b',' => Tok::Comma,
                            b'.' => Tok::Dot,
                            b':' => Tok::Colon,
                            b'?' => Tok::Question,
                            b'!' => Tok::Bang,
                            b'$' => Tok::Dollar,
                            b'=' => Tok::Assign,
                            b'+' => Tok::Plus,
                            b'-' => Tok::Minus,
                            b'*' => Tok::Star,
                            b'/' => Tok::Slash,
                            b'%' => Tok::Percent,
                            b'&' => Tok::Amp,
                            b'|' => Tok::Pipe,
                            b'^' => Tok::Caret,
                            b'<' => Tok::Lt,
                            b'>' => Tok::Gt,
                            _ => {
                                let ch = self.src[self.pos..].chars().next().unwrap();
                                self.pos += ch.len_utf8();
                                return err(
                                    self.span(start),
                                    format!("unexpected character `{ch}`"),
                                );
                            }
                        };
                        (t, 1)
                    }
                };
                self.pos += n;
                t
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
            let s = self.pos;
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.pos += 1;
            }
            let mut digits: String = self.src[s..self.pos].replace('_', "");
            let mut unsigned = false;
            // a trailing `u` suffix (`u` is never a digit in any of these radixes)
            if digits.ends_with('u') {
                digits.pop();
                unsigned = true;
            }
            if digits.is_empty() {
                return err(self.span(start), "expected digits after number prefix");
            }
            let v = u128::from_str_radix(&digits, radix)
                .map_err(|_| crate::diag::Diag::new(self.span(start), "invalid number literal"))?;
            return Ok(Tok::Int(v, unsigned));
        }
        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            is_float = true;
            self.pos += 1;
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                self.pos += 1;
            }
        }
        let mant_end = self.pos;
        let mut exp: Option<i64> = None;
        if matches!(self.peek(), b'e' | b'E') {
            let mut k = 1;
            if matches!(self.peek_at(1), b'+' | b'-') {
                k = 2;
            }
            if self.peek_at(k).is_ascii_digit() {
                let es = self.pos + 1;
                self.pos += k;
                while self.peek().is_ascii_digit() || self.peek() == b'_' {
                    self.pos += 1;
                }
                let etext = self.src[es..self.pos].replace('_', "");
                exp =
                    Some(etext.parse().map_err(|_| {
                        crate::diag::Diag::new(self.span(start), "exponent too large")
                    })?);
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
        if let Some(e) = exp {
            // integer literal with a non-negative exponent: exact integer value
            let mant: u128 = self.src[start..mant_end]
                .replace('_', "")
                .parse()
                .map_err(|_| {
                    crate::diag::Diag::new(self.span(start), "integer literal too large")
                })?;
            let mut v = mant;
            for _ in 0..e {
                v = v.checked_mul(10).ok_or_else(|| {
                    crate::diag::Diag::new(self.span(start), "integer literal too large")
                })?;
            }
            let mut unsigned = false;
            if self.peek() == b'u' && !self.peek_at(1).is_ascii_alphanumeric() {
                self.pos += 1;
                unsigned = true;
            }
            if self.peek().is_ascii_alphanumeric() {
                return err(self.span(start), "invalid number literal");
            }
            return Ok(Tok::Int(v, unsigned));
        }
        let text = self.src[start..self.pos].replace('_', "");
        let mut unsigned = false;
        if self.peek() == b'u' && !self.peek_at(1).is_ascii_alphanumeric() {
            self.pos += 1;
            unsigned = true;
        }
        if self.peek().is_ascii_alphabetic() {
            return err(self.span(start), "invalid number literal");
        }
        let v: u128 = text
            .parse()
            .map_err(|_| crate::diag::Diag::new(self.span(start), "integer literal too large"))?;
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
                let s = self.pos;
                while self.peek().is_ascii_hexdigit() {
                    self.pos += 1;
                }
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
                    crate::diag::Diag::new(
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
            _ => {
                let ch = self.src[self.pos..].chars().next().unwrap();
                self.pos += ch.len_utf8();
                ch
            }
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
                b'\\' => {
                    self.pos += 1;
                    cur.push(self.escape(start, false)?);
                }
                b'{' => {
                    if !cur.is_empty() {
                        parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                    }
                    let (text, off) = self.interp(start)?;
                    parts.push(StrPart::Expr(text, self.base + off));
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                    cur.push(ch);
                }
            }
        }
        if !cur.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        Ok(Tok::Str(parts))
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
                b'"' => {
                    // nested string inside interpolation
                    self.pos += 1;
                    while self.peek() != b'"' {
                        if self.peek() == 0 || self.peek() == b'\n' {
                            return err(self.span(start), "unterminated string literal");
                        }
                        if self.peek() == b'\\' {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                }
                b'\'' => {
                    self.pos += 1;
                    while self.peek() != b'\'' {
                        if self.peek() == 0 || self.peek() == b'\n' {
                            return err(self.span(start), "unterminated char literal");
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
        let rel_end = match self.src[self.pos..].find("\"\"\"") {
            Some(i) => i,
            None => return err(self.span(start), "unterminated multiline string"),
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
            let mut sub = Lexer {
                src: self.src,
                bytes: self.bytes,
                pos: content_off,
                base: self.base,
                file: self.file,
                comments: vec![],
            };
            let end = content_off + content.len();
            while sub.pos < end {
                match sub.peek() {
                    b'\\' => {
                        sub.pos += 1;
                        let c = sub.escape(start, false)?;
                        cur.push(c);
                    }
                    b'{' => {
                        if !cur.is_empty() {
                            parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                        }
                        let (text, o) = sub.interp(start)?;
                        if sub.pos > end {
                            return err(self.span(start), "interpolation cannot span lines");
                        }
                        parts.push(StrPart::Expr(text, self.base + o));
                    }
                    _ => {
                        let ch = self.src[sub.pos..].chars().next().unwrap();
                        sub.pos += ch.len_utf8();
                        cur.push(ch);
                    }
                }
            }
        }
        if !cur.is_empty() || parts.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        Ok(Tok::Str(parts))
    }
}
