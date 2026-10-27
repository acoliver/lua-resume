//! Expression parsing, following Lua 5.4's `subexpr`, `simpleexp` and `suffixedexp`.

use super::ParseError;
use super::ast::{BinOp, Call, Expr, Field, UNARY_PRIORITY, UnOp};
use super::lexer::{Kw, Sym, Tok};
use super::parser::Parser;

fn binary_op(tok: &Tok) -> Option<BinOp> {
    Some(match tok {
        Tok::Kw(Kw::Or) => BinOp::Or,
        Tok::Kw(Kw::And) => BinOp::And,
        Tok::Sym(sym) => match sym {
            Sym::Lt => BinOp::Lt,
            Sym::Gt => BinOp::Gt,
            Sym::Le => BinOp::Le,
            Sym::Ge => BinOp::Ge,
            Sym::Ne => BinOp::Ne,
            Sym::Eq => BinOp::Eq,
            Sym::Pipe => BinOp::BOr,
            Sym::Tilde => BinOp::BXor,
            Sym::Amp => BinOp::BAnd,
            Sym::Shl => BinOp::Shl,
            Sym::Shr => BinOp::Shr,
            Sym::Concat => BinOp::Concat,
            Sym::Plus => BinOp::Add,
            Sym::Minus => BinOp::Sub,
            Sym::Star => BinOp::Mul,
            Sym::Slash => BinOp::Div,
            Sym::DSlash => BinOp::IDiv,
            Sym::Percent => BinOp::Mod,
            Sym::Caret => BinOp::Pow,
            _ => return None,
        },
        _ => return None,
    })
}

fn unary_op(tok: &Tok) -> Option<UnOp> {
    Some(match tok {
        Tok::Kw(Kw::Not) => UnOp::Not,
        Tok::Sym(Sym::Minus) => UnOp::Neg,
        Tok::Sym(Sym::Hash) => UnOp::Len,
        Tok::Sym(Sym::Tilde) => UnOp::BNot,
        _ => return None,
    })
}

impl Parser {
    pub(super) fn expr(&mut self) -> Result<Expr, ParseError> {
        self.sub_expr(0)
    }

    pub(super) fn expr_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut exprs = vec![self.expr()?];
        while self.accept_sym(Sym::Comma) {
            exprs.push(self.expr()?);
        }
        Ok(exprs)
    }

    /// Parses operators whose left priority is above `limit`.
    fn sub_expr(&mut self, limit: u8) -> Result<Expr, ParseError> {
        self.enter()?;
        let mut left = if let Some(op) = unary_op(self.peek()) {
            self.advance();
            Expr::Unary(op, Box::new(self.sub_expr(UNARY_PRIORITY)?))
        } else {
            self.simple_expr()?
        };
        while let Some(op) = binary_op(self.peek()) {
            let (left_priority, right_priority) = op.priority();
            if left_priority <= limit {
                break;
            }
            self.advance();
            let right = self.sub_expr(right_priority)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        self.leave();
        Ok(left)
    }

    fn simple_expr(&mut self) -> Result<Expr, ParseError> {
        let expr = match self.peek() {
            Tok::Number(_) | Tok::Str(_) => match self.advance() {
                Tok::Number(text) => Expr::Number(text),
                Tok::Str(bytes) => Expr::Str(bytes),
                _ => unreachable!("peeked a literal"),
            },
            Tok::Kw(Kw::Nil) => self.literal(Expr::Nil),
            Tok::Kw(Kw::True) => self.literal(Expr::True),
            Tok::Kw(Kw::False) => self.literal(Expr::False),
            Tok::Sym(Sym::Dots) => self.literal(Expr::Vararg),
            Tok::Sym(Sym::LBrace) => self.table()?,
            Tok::Kw(Kw::Function) => {
                let line = self.line();
                self.advance();
                Expr::Function(Box::new(self.func_body(line, false)?))
            }
            _ => return self.suffixed_expr(),
        };
        Ok(expr)
    }

    fn literal(&mut self, expr: Expr) -> Expr {
        self.advance();
        expr
    }

    fn primary_expr(&mut self) -> Result<Expr, ParseError> {
        if self.accept_sym(Sym::LParen) {
            let inner = self.expr()?;
            self.expect_sym(Sym::RParen, ")")?;
            return Ok(Expr::Paren(Box::new(inner)));
        }
        if matches!(self.peek(), Tok::Name(_)) {
            return Ok(Expr::Name(self.name()?));
        }
        Err(self.error("unexpected symbol"))
    }

    pub(super) fn suffixed_expr(&mut self) -> Result<Expr, ParseError> {
        self.enter()?;
        // Lua gives every call in the chain the line its primary expression starts on.
        let line = self.line();
        let mut expr = self.primary_expr()?;
        loop {
            expr = match self.peek() {
                Tok::Sym(Sym::Dot) => {
                    self.advance();
                    Expr::Field(Box::new(expr), self.name()?)
                }
                Tok::Sym(Sym::LBracket) => {
                    self.advance();
                    let key = self.expr()?;
                    self.expect_sym(Sym::RBracket, "]")?;
                    Expr::Index(Box::new(expr), Box::new(key))
                }
                Tok::Sym(Sym::Colon) => {
                    self.advance();
                    let method = Some(self.name()?);
                    let args = self.args()?;
                    call(line, expr, method, args)
                }
                Tok::Sym(Sym::LParen | Sym::LBrace) | Tok::Str(_) => {
                    let args = self.args()?;
                    call(line, expr, None, args)
                }
                _ => break,
            };
        }
        self.leave();
        Ok(expr)
    }

    fn args(&mut self) -> Result<Vec<Expr>, ParseError> {
        match self.peek() {
            Tok::Str(_) => match self.advance() {
                Tok::Str(bytes) => Ok(vec![Expr::Str(bytes)]),
                _ => unreachable!("peeked a string"),
            },
            Tok::Sym(Sym::LBrace) => Ok(vec![self.table()?]),
            Tok::Sym(Sym::LParen) => {
                self.advance();
                if self.accept_sym(Sym::RParen) {
                    return Ok(Vec::new());
                }
                let args = self.expr_list()?;
                self.expect_sym(Sym::RParen, ")")?;
                Ok(args)
            }
            _ => Err(self.error("function arguments expected")),
        }
    }

    fn table(&mut self) -> Result<Expr, ParseError> {
        self.expect_sym(Sym::LBrace, "{")?;
        let mut fields = Vec::new();
        while !self.is_sym(Sym::RBrace) {
            fields.push(self.field()?);
            if !self.accept_sym(Sym::Comma) && !self.accept_sym(Sym::Semi) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace, "}")?;
        Ok(Expr::Table(fields))
    }

    fn field(&mut self) -> Result<Field, ParseError> {
        if self.accept_sym(Sym::LBracket) {
            let key = self.expr()?;
            self.expect_sym(Sym::RBracket, "]")?;
            self.expect_sym(Sym::Assign, "=")?;
            return Ok(Field::Keyed(key, self.expr()?));
        }
        if matches!(self.peek(), Tok::Name(_)) && self.next_is_assign() {
            let name = self.name()?;
            self.advance();
            return Ok(Field::Named(name, self.expr()?));
        }
        Ok(Field::Item(self.expr()?))
    }
}

fn call(line: u32, callee: Expr, method: Option<String>, args: Vec<Expr>) -> Expr {
    Expr::Call(Box::new(Call {
        line,
        callee,
        method,
        args,
    }))
}
