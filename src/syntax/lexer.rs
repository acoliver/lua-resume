//! The Lua 5.4 lexer, following the rules of Lua's `llex.c`.

use super::ParseError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Name(String),
    Number(String),
    Str(Vec<u8>),
    Kw(Kw),
    Sym(Sym),
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    Goto,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sym {
    Plus,
    Minus,
    Star,
    Slash,
    DSlash,
    Percent,
    Caret,
    Hash,
    Amp,
    Tilde,
    Pipe,
    Shl,
    Shr,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    DColon,
    Semi,
    Colon,
    Comma,
    Dot,
    Concat,
    Dots,
}

/// A token and the line it starts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

fn keyword(name: &str) -> Option<Kw> {
    Some(match name {
        "and" => Kw::And,
        "break" => Kw::Break,
        "do" => Kw::Do,
        "else" => Kw::Else,
        "elseif" => Kw::Elseif,
        "end" => Kw::End,
        "false" => Kw::False,
        "for" => Kw::For,
        "function" => Kw::Function,
        "goto" => Kw::Goto,
        "if" => Kw::If,
        "in" => Kw::In,
        "local" => Kw::Local,
        "nil" => Kw::Nil,
        "not" => Kw::Not,
        "or" => Kw::Or,
        "repeat" => Kw::Repeat,
        "return" => Kw::Return,
        "then" => Kw::Then,
        "true" => Kw::True,
        "until" => Kw::Until,
        "while" => Kw::While,
        _ => return None,
    })
}

/// Splits Lua source into tokens, ending with [`Tok::Eof`].
pub fn tokenize(src: &[u8]) -> Result<Vec<Token>, ParseError> {
    let mut lexer = Lexer {
        src,
        pos: 0,
        line: 1,
    };
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token()?;
        let end = token.tok == Tok::Eof;
        tokens.push(token);
        if end {
            return Ok(tokens);
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl Lexer<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            message: message.into(),
        }
    }

    fn at_newline(&self) -> bool {
        matches!(self.peek(), Some(b'\n' | b'\r'))
    }

    /// Consumes one line break: `\n`, `\r`, `\r\n` or `\n\r`.
    fn newline(&mut self) {
        let first = self.peek();
        self.pos += 1;
        if self.at_newline() && self.peek() != first {
            self.pos += 1;
        }
        self.line += 1;
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_trivia()?;
        let line = self.line;
        let Some(b) = self.peek() else {
            return Ok(Token {
                tok: Tok::Eof,
                line,
            });
        };
        let tok = if is_name_start(b) {
            self.name()
        } else if b.is_ascii_digit()
            || (b == b'.' && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()))
        {
            self.number()?
        } else if b == b'"' || b == b'\'' {
            Tok::Str(self.short_string(b)?)
        } else if let Some(level) = self.long_bracket() {
            Tok::Str(self.long_string(level)?)
        } else {
            Tok::Sym(self.symbol()?)
        };
        Ok(Token { tok, line })
    }

    fn skip_trivia(&mut self) -> Result<(), ParseError> {
        while let Some(b) = self.peek() {
            if self.at_newline() {
                self.newline();
            } else if b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c {
                self.pos += 1;
            } else if b == b'-' && self.peek_at(1) == Some(b'-') {
                self.comment()?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn comment(&mut self) -> Result<(), ParseError> {
        self.pos += 2;
        if let Some(level) = self.long_bracket() {
            self.long_string(level)?;
            return Ok(());
        }
        while self.peek().is_some() && !self.at_newline() {
            self.pos += 1;
        }
        Ok(())
    }

    /// The level of a long bracket `[==[` at the cursor, if one starts here.
    fn long_bracket(&self) -> Option<usize> {
        if self.peek() != Some(b'[') {
            return None;
        }
        let mut level = 0;
        while self.peek_at(1 + level) == Some(b'=') {
            level += 1;
        }
        (self.peek_at(1 + level) == Some(b'[')).then_some(level)
    }

    fn long_string(&mut self, level: usize) -> Result<Vec<u8>, ParseError> {
        let start_line = self.line;
        self.pos += level + 2;
        if self.at_newline() {
            self.newline();
        }
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError {
                        line: start_line,
                        message: "unfinished long string or comment".into(),
                    });
                }
                Some(b']') if self.closes(level) => {
                    self.pos += level + 2;
                    return Ok(out);
                }
                Some(b'\n' | b'\r') => {
                    self.newline();
                    out.push(b'\n');
                }
                Some(b) => {
                    out.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    fn closes(&self, level: usize) -> bool {
        (1..=level).all(|i| self.peek_at(i) == Some(b'=')) && self.peek_at(level + 1) == Some(b']')
    }

    fn name(&mut self) -> Tok {
        let start = self.pos;
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        keyword(&text).map_or(Tok::Name(text), Tok::Kw)
    }

    /// Reads a numeral as Lua's `read_numeral` does, then checks that it is well formed.
    fn number(&mut self) -> Result<Tok, ParseError> {
        let start = self.pos;
        let mut exponent = *b"eE";
        if self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'x' | b'X')) {
            exponent = *b"pP";
            self.pos += 2;
        }
        while let Some(b) = self.peek() {
            if exponent.contains(&b) {
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
            } else if b.is_ascii_hexdigit() || b == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        if super::numeral::is_valid(&text) {
            Ok(Tok::Number(text))
        } else {
            Err(self.error(format!("malformed number near '{text}'")))
        }
    }

    fn short_string(&mut self, quote: u8) -> Result<Vec<u8>, ParseError> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'\n' | b'\r') => return Err(self.error("unfinished string")),
                Some(b) if b == quote => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => self.escape(&mut out)?,
                Some(b) => {
                    out.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), ParseError> {
        self.pos += 1;
        let Some(b) = self.peek() else {
            return Err(self.error("unfinished string"));
        };
        let simple = match b {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'\\' | b'"' | b'\'' => Some(b),
            _ => None,
        };
        if let Some(value) = simple {
            out.push(value);
            self.pos += 1;
            return Ok(());
        }
        match b {
            b'\n' | b'\r' => {
                self.newline();
                out.push(b'\n');
            }
            b'x' => out.push(self.hex_escape()?),
            b'z' => self.skip_z(),
            b'u' => self.utf8_escape(out)?,
            b'0'..=b'9' => out.push(self.decimal_escape()?),
            _ => return Err(self.error("invalid escape sequence")),
        }
        Ok(())
    }

    fn hex_digit(&mut self) -> Result<u32, ParseError> {
        let digit = self
            .peek()
            .and_then(|b| char::from(b).to_digit(16))
            .ok_or_else(|| self.error("hexadecimal digit expected"))?;
        self.pos += 1;
        Ok(digit)
    }

    fn hex_escape(&mut self) -> Result<u8, ParseError> {
        self.pos += 1;
        let high = self.hex_digit()?;
        let low = self.hex_digit()?;
        Ok(u8::try_from(high * 16 + low).expect("two hex digits fit a byte"))
    }

    fn skip_z(&mut self) {
        self.pos += 1;
        while let Some(b) = self.peek() {
            if self.at_newline() {
                self.newline();
            } else if b.is_ascii_whitespace() || b == 0x0b {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn decimal_escape(&mut self) -> Result<u8, ParseError> {
        let mut value = 0u32;
        for _ in 0..3 {
            match self.peek() {
                Some(b @ b'0'..=b'9') => {
                    value = value * 10 + u32::from(b - b'0');
                    self.pos += 1;
                }
                _ => break,
            }
        }
        u8::try_from(value).map_err(|_| self.error("decimal escape too large"))
    }

    fn utf8_escape(&mut self, out: &mut Vec<u8>) -> Result<(), ParseError> {
        self.pos += 1;
        if self.peek() != Some(b'{') {
            return Err(self.error("missing '{' in \\u{xxxx}"));
        }
        self.pos += 1;
        let mut value = self.hex_digit()?;
        while self.peek() != Some(b'}') {
            if value > 0x7FF_FFFF {
                return Err(self.error("UTF-8 value too large"));
            }
            value = value * 16 + self.hex_digit()?;
        }
        self.pos += 1;
        encode_utf8(value, out);
        Ok(())
    }

    fn symbol(&mut self) -> Result<Sym, ParseError> {
        let b = self.peek().expect("symbol at a byte");
        let next = self.peek_at(1);
        let third = self.peek_at(2);
        let (sym, len) = match (b, next) {
            (b'.', Some(b'.')) if third == Some(b'.') => (Sym::Dots, 3),
            (b'.', Some(b'.')) => (Sym::Concat, 2),
            (b'/', Some(b'/')) => (Sym::DSlash, 2),
            (b':', Some(b':')) => (Sym::DColon, 2),
            (b'<', Some(b'<')) => (Sym::Shl, 2),
            (b'>', Some(b'>')) => (Sym::Shr, 2),
            (b'=', Some(b'=')) => (Sym::Eq, 2),
            (b'~', Some(b'=')) => (Sym::Ne, 2),
            (b'<', Some(b'=')) => (Sym::Le, 2),
            (b'>', Some(b'=')) => (Sym::Ge, 2),
            _ => (
                single(b).ok_or_else(|| {
                    self.error(format!("unexpected symbol near '{}'", char::from(b)))
                })?,
                1,
            ),
        };
        self.pos += len;
        Ok(sym)
    }
}

fn single(b: u8) -> Option<Sym> {
    Some(match b {
        b'+' => Sym::Plus,
        b'-' => Sym::Minus,
        b'*' => Sym::Star,
        b'/' => Sym::Slash,
        b'%' => Sym::Percent,
        b'^' => Sym::Caret,
        b'#' => Sym::Hash,
        b'&' => Sym::Amp,
        b'~' => Sym::Tilde,
        b'|' => Sym::Pipe,
        b'<' => Sym::Lt,
        b'>' => Sym::Gt,
        b'=' => Sym::Assign,
        b'(' => Sym::LParen,
        b')' => Sym::RParen,
        b'{' => Sym::LBrace,
        b'}' => Sym::RBrace,
        b'[' => Sym::LBracket,
        b']' => Sym::RBracket,
        b';' => Sym::Semi,
        b':' => Sym::Colon,
        b',' => Sym::Comma,
        b'.' => Sym::Dot,
        _ => return None,
    })
}

/// Encodes a code point up to `0x7FFFFFFF` as Lua's `luaO_utf8esc` does.
fn encode_utf8(mut value: u32, out: &mut Vec<u8>) {
    if value < 0x80 {
        out.push(u8::try_from(value).expect("below 0x80"));
        return;
    }
    let mut tail = Vec::new();
    let mut first_max = 0x3f;
    loop {
        tail.push(0x80 | u8::try_from(value & 0x3f).expect("six bits"));
        value >>= 6;
        first_max >>= 1;
        if value <= first_max {
            break;
        }
    }
    let lead = (!first_max << 1) | value;
    out.push(u8::try_from(lead & 0xff).expect("one byte"));
    out.extend(tail.iter().rev());
}
