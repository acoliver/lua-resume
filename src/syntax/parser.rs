//! Statement parsing, following Lua 5.4's `lparser.c`.

use super::ParseError;
use super::ast::{
    Attrib, Block, Expr, FuncBody, FuncName, IfArm, LocalName, Name, Return, Stat, StatKind,
};
use super::lexer::{Kw, Sym, Tok, Token};

/// Nesting deeper than this is refused, as Lua refuses more than 200 C levels.
const MAX_DEPTH: u32 = 200;

pub(super) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: u32,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
        }
    }

    pub(super) fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    pub(super) fn line(&self) -> u32 {
        self.tokens[self.pos].line
    }

    pub(super) fn advance(&mut self) -> Tok {
        let tok = self.tokens[self.pos].tok.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    pub(super) fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line(),
            message: message.into(),
        }
    }

    /// Whether the token after the current one is `=`.
    pub(super) fn next_is_assign(&self) -> bool {
        self.tokens
            .get(self.pos + 1)
            .is_some_and(|token| token.tok == Tok::Sym(Sym::Assign))
    }

    pub(super) fn is_sym(&self, sym: Sym) -> bool {
        *self.peek() == Tok::Sym(sym)
    }

    pub(super) fn is_kw(&self, kw: Kw) -> bool {
        *self.peek() == Tok::Kw(kw)
    }

    pub(super) fn accept_sym(&mut self, sym: Sym) -> bool {
        let found = self.is_sym(sym);
        if found {
            self.advance();
        }
        found
    }

    fn accept_kw(&mut self, kw: Kw) -> bool {
        let found = self.is_kw(kw);
        if found {
            self.advance();
        }
        found
    }

    pub(super) fn expect_sym(&mut self, sym: Sym, what: &str) -> Result<(), ParseError> {
        if self.accept_sym(sym) {
            Ok(())
        } else {
            Err(self.error(format!("'{what}' expected")))
        }
    }

    fn expect_kw(&mut self, kw: Kw, what: &str) -> Result<(), ParseError> {
        if self.accept_kw(kw) {
            Ok(())
        } else {
            Err(self.error(format!("'{what}' expected")))
        }
    }

    pub(super) fn name(&mut self) -> Result<Name, ParseError> {
        match self.peek() {
            Tok::Name(_) => match self.advance() {
                Tok::Name(name) => Ok(name),
                _ => unreachable!("peeked a name"),
            },
            _ => Err(self.error("<name> expected")),
        }
    }

    pub(super) fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("chunk has too many syntax levels"));
        }
        Ok(())
    }

    pub(super) fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Parses a whole chunk.
    pub(super) fn chunk(&mut self) -> Result<Block, ParseError> {
        let block = self.block()?;
        if *self.peek() != Tok::Eof {
            return Err(self.error("'<eof>' expected"));
        }
        Ok(block)
    }

    fn block_follows(&self, with_until: bool) -> bool {
        match self.peek() {
            Tok::Eof | Tok::Kw(Kw::Else | Kw::Elseif | Kw::End) => true,
            Tok::Kw(Kw::Until) => with_until,
            _ => false,
        }
    }

    pub(super) fn block(&mut self) -> Result<Block, ParseError> {
        self.enter()?;
        let mut block = Block::default();
        while !self.block_follows(true) {
            if self.is_kw(Kw::Return) {
                block.ret = Some(self.return_stat()?);
                break;
            }
            if self.accept_sym(Sym::Semi) {
                continue;
            }
            block.stats.push(self.statement()?);
        }
        self.leave();
        Ok(block)
    }

    fn return_stat(&mut self) -> Result<Return, ParseError> {
        let line = self.line();
        self.advance();
        let values = if self.block_follows(true) || self.is_sym(Sym::Semi) {
            Vec::new()
        } else {
            self.expr_list()?
        };
        self.accept_sym(Sym::Semi);
        if !self.block_follows(true) {
            return Err(self.error("'<eof>' expected"));
        }
        Ok(Return { line, values })
    }

    fn statement(&mut self) -> Result<Stat, ParseError> {
        let line = self.line();
        let kind = match self.peek() {
            Tok::Kw(Kw::If) => self.if_stat()?,
            Tok::Kw(Kw::While) => self.while_stat()?,
            Tok::Kw(Kw::Do) => {
                self.advance();
                let body = self.block()?;
                self.expect_kw(Kw::End, "end")?;
                StatKind::Do(body)
            }
            Tok::Kw(Kw::For) => self.for_stat()?,
            Tok::Kw(Kw::Repeat) => self.repeat_stat()?,
            Tok::Kw(Kw::Function) => self.function_stat()?,
            Tok::Kw(Kw::Local) => self.local_stat()?,
            Tok::Sym(Sym::DColon) => self.label_stat()?,
            Tok::Kw(Kw::Break) => {
                self.advance();
                StatKind::Break
            }
            Tok::Kw(Kw::Goto) => {
                self.advance();
                StatKind::Goto(self.name()?)
            }
            _ => self.expr_stat()?,
        };
        Ok(Stat { line, kind })
    }

    fn if_stat(&mut self) -> Result<StatKind, ParseError> {
        let mut arms = Vec::new();
        loop {
            let line = self.line();
            self.advance();
            let cond = self.expr()?;
            self.expect_kw(Kw::Then, "then")?;
            let body = self.block()?;
            arms.push(IfArm { line, cond, body });
            if !self.is_kw(Kw::Elseif) {
                break;
            }
        }
        let otherwise = if self.accept_kw(Kw::Else) {
            Some(self.block()?)
        } else {
            None
        };
        self.expect_kw(Kw::End, "end")?;
        Ok(StatKind::If { arms, otherwise })
    }

    fn while_stat(&mut self) -> Result<StatKind, ParseError> {
        self.advance();
        let cond = self.expr()?;
        self.expect_kw(Kw::Do, "do")?;
        let body = self.block()?;
        self.expect_kw(Kw::End, "end")?;
        Ok(StatKind::While { cond, body })
    }

    fn repeat_stat(&mut self) -> Result<StatKind, ParseError> {
        self.advance();
        let body = self.block()?;
        self.expect_kw(Kw::Until, "until")?;
        let cond = self.expr()?;
        Ok(StatKind::Repeat { body, cond })
    }

    fn for_stat(&mut self) -> Result<StatKind, ParseError> {
        self.advance();
        let first = self.name()?;
        if self.accept_sym(Sym::Assign) {
            let start = self.expr()?;
            self.expect_sym(Sym::Comma, ",")?;
            let limit = self.expr()?;
            let step = if self.accept_sym(Sym::Comma) {
                Some(self.expr()?)
            } else {
                None
            };
            let body = self.loop_body()?;
            return Ok(StatKind::NumericFor {
                var: first,
                start,
                limit,
                step,
                body,
            });
        }
        let mut names = vec![first];
        while self.accept_sym(Sym::Comma) {
            names.push(self.name()?);
        }
        self.expect_kw(Kw::In, "in")?;
        let exprs = self.expr_list()?;
        let body = self.loop_body()?;
        Ok(StatKind::GenericFor { names, exprs, body })
    }

    fn loop_body(&mut self) -> Result<Block, ParseError> {
        self.expect_kw(Kw::Do, "do")?;
        let body = self.block()?;
        self.expect_kw(Kw::End, "end")?;
        Ok(body)
    }

    fn function_stat(&mut self) -> Result<StatKind, ParseError> {
        let line = self.line();
        self.advance();
        let mut path = vec![self.name()?];
        while self.accept_sym(Sym::Dot) {
            path.push(self.name()?);
        }
        let method = if self.accept_sym(Sym::Colon) {
            Some(self.name()?)
        } else {
            None
        };
        let body = self.func_body(line, method.is_some())?;
        Ok(StatKind::Function {
            name: FuncName { path, method },
            body: Box::new(body),
        })
    }

    fn local_stat(&mut self) -> Result<StatKind, ParseError> {
        let line = self.line();
        self.advance();
        if self.accept_kw(Kw::Function) {
            let name = self.name()?;
            let body = self.func_body(line, false)?;
            return Ok(StatKind::LocalFunction {
                name,
                body: Box::new(body),
            });
        }
        let mut names = vec![self.local_name()?];
        while self.accept_sym(Sym::Comma) {
            names.push(self.local_name()?);
        }
        let values = if self.accept_sym(Sym::Assign) {
            self.expr_list()?
        } else {
            Vec::new()
        };
        Ok(StatKind::Local { names, values })
    }

    fn local_name(&mut self) -> Result<LocalName, ParseError> {
        let name = self.name()?;
        if !self.accept_sym(Sym::Lt) {
            return Ok(LocalName { name, attrib: None });
        }
        let attrib = match self.name()?.as_str() {
            "const" => Attrib::Const,
            "close" => Attrib::Close,
            other => return Err(self.error(format!("unknown attribute '{other}'"))),
        };
        self.expect_sym(Sym::Gt, ">")?;
        Ok(LocalName {
            name,
            attrib: Some(attrib),
        })
    }

    fn label_stat(&mut self) -> Result<StatKind, ParseError> {
        self.advance();
        let name = self.name()?;
        self.expect_sym(Sym::DColon, "::")?;
        Ok(StatKind::Label(name))
    }

    fn expr_stat(&mut self) -> Result<StatKind, ParseError> {
        let first = self.suffixed_expr()?;
        if self.is_sym(Sym::Assign) || self.is_sym(Sym::Comma) {
            let mut targets = vec![first];
            while self.accept_sym(Sym::Comma) {
                targets.push(self.suffixed_expr()?);
            }
            self.expect_sym(Sym::Assign, "=")?;
            if !targets.iter().all(is_assignable) {
                return Err(self.error("syntax error near '='"));
            }
            let values = self.expr_list()?;
            return Ok(StatKind::Assign { targets, values });
        }
        match first {
            Expr::Call(call) => Ok(StatKind::Call(call)),
            _ => Err(self.error("syntax error")),
        }
    }

    /// Parses `(params) body end`; `line` is the line of the `function` keyword.
    pub(super) fn func_body(&mut self, line: u32, method: bool) -> Result<FuncBody, ParseError> {
        self.expect_sym(Sym::LParen, "(")?;
        let mut params = Vec::new();
        if method {
            params.push("self".to_owned());
        }
        let mut vararg = false;
        if !self.is_sym(Sym::RParen) {
            loop {
                if self.accept_sym(Sym::Dots) {
                    vararg = true;
                    break;
                }
                params.push(self.name()?);
                if !self.accept_sym(Sym::Comma) {
                    break;
                }
            }
        }
        self.expect_sym(Sym::RParen, ")")?;
        let body = self.block()?;
        let end_line = self.line();
        self.expect_kw(Kw::End, "end")?;
        Ok(FuncBody {
            line,
            end_line,
            params,
            vararg,
            body,
        })
    }
}

fn is_assignable(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(_) | Expr::Field(..) | Expr::Index(..))
}
