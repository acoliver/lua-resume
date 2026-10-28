//! Rewriting expressions: calls outside chain positions move into temporaries, and operands Lua
//! evaluates before such a call move into temporaries ahead of it.

use std::mem;

use super::func::{Func, is_temp};
use super::walk::has_call;
use crate::syntax::ast::{BinOp, Block, Call, Expr, Field, IfArm, Stat, StatKind, UnOp};

/// When Lua reads an operand that comes before a call in the same expression.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Read {
    /// Copied into a register when it is evaluated: callees, arguments, table items, list
    /// values and `..` operands.
    Copy,
    /// Read where it is when the operation runs if it is a local of this function: operands of
    /// the other operators.
    Operand,
    /// Read when the indexing runs if it is a local or an upvalue: the table being indexed.
    Table,
}

fn is_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(_) => true,
        Expr::Paren(inner) => matches!(**inner, Expr::Call(_)),
        _ => false,
    }
}

impl Func {
    /// Rewrites an expression in a chain position: a call, a parenthesized call or a table
    /// constructor stays where it is, with its own parts rewritten.
    pub(super) fn chain(&mut self, expr: &Expr) -> Expr {
        match expr {
            Expr::Call(call) => Expr::Call(Box::new(self.call(call))),
            Expr::Paren(inner) if is_call(inner) => Expr::Paren(Box::new(self.chain(inner))),
            Expr::Table(fields) => self.table(fields),
            _ => self.value(expr),
        }
    }

    /// Rewrites an expression into one with no calls, moving calls into temporaries.
    pub(super) fn value(&mut self, expr: &Expr) -> Expr {
        match expr {
            Expr::Function(body) => Expr::Function(Box::new(self.nested(body))),
            Expr::Table(_) => {
                let table = self.chain(expr);
                if has_call(&table) {
                    self.hoist(table)
                } else {
                    table
                }
            }
            _ if is_call(expr) => {
                let call = self.chain(expr);
                self.hoist(call)
            }
            Expr::Paren(inner) => Expr::Paren(Box::new(self.value(inner))),
            Expr::Binary(op, left, right) => self.binary(*op, left, right),
            Expr::Unary(op, operand) => Expr::Unary(*op, Box::new(self.value(operand))),
            Expr::Field(obj, name) => Expr::Field(Box::new(self.value(obj)), name.clone()),
            Expr::Index(obj, key) => {
                let (obj, key) = self.ordered(obj, key, Read::Table);
                Expr::Index(Box::new(obj), Box::new(key))
            }
            _ => expr.clone(),
        }
    }

    /// Rewrites a list Lua evaluates into consecutive registers, keeping calls in place.
    pub(super) fn chain_list(&mut self, exprs: &[Expr]) -> Vec<Expr> {
        self.list(exprs, Self::chain)
    }

    /// Rewrites a list Lua evaluates into consecutive registers, moving calls out.
    pub(super) fn value_refs(&mut self, exprs: &[&Expr]) -> Vec<Expr> {
        let mut out = Vec::new();
        for expr in exprs {
            let mark = self.pre.len();
            let expr = self.value(expr);
            if self.pre.len() > mark {
                self.spill_all(&mut out, mark);
            }
            out.push(expr);
        }
        out
    }

    fn list(&mut self, exprs: &[Expr], each: fn(&mut Self, &Expr) -> Expr) -> Vec<Expr> {
        let mut out = Vec::with_capacity(exprs.len());
        for expr in exprs {
            let mark = self.pre.len();
            let expr = each(self, expr);
            if self.pre.len() > mark || has_call(&expr) {
                self.spill_all(&mut out, mark);
            }
            out.push(expr);
        }
        out
    }

    /// Moves `expr` into a new temporary, after the statements already pending.
    pub(super) fn hoist(&mut self, expr: Expr) -> Expr {
        let temp = self.temp();
        let stat = self.assign(&temp, expr);
        self.pre.push(stat);
        Expr::Name(temp)
    }

    /// Keeps `expr` if Lua reads it as `read` describes and it cannot change, else hoists it.
    pub(super) fn pin(&mut self, expr: Expr, read: Read) -> Expr {
        if self.stable(&expr, read) {
            expr
        } else {
            self.hoist(expr)
        }
    }

    /// Moves `expr` into a new temporary assigned at position `at` of the pending statements.
    fn spill_at(&mut self, at: usize, expr: Expr) -> Expr {
        let temp = self.temp();
        let stat = self.assign(&temp, expr);
        self.pre.insert(at, stat);
        Expr::Name(temp)
    }

    /// Spills `expr` at `at` unless it is stable when copied, advancing `at` past the spill.
    fn spill_one(&mut self, expr: &mut Expr, at: &mut usize) {
        if !self.stable(expr, Read::Copy) {
            let value = mem::replace(expr, Expr::Nil);
            *expr = self.spill_at(*at, value);
            *at += 1;
        }
    }

    /// Spills every item that is not stable when copied, in order, starting at `at`.
    fn spill_all(&mut self, items: &mut [Expr], mut at: usize) -> usize {
        for item in items {
            self.spill_one(item, &mut at);
        }
        at
    }

    /// Whether `expr` has the same value whether it is read now or after later calls run.
    fn stable(&self, expr: &Expr, read: Read) -> bool {
        match expr {
            Expr::Paren(inner) => {
                self.stable(inner, read)
                    && !(read == Read::Table
                        && matches!(&**inner, Expr::Name(name) if self.is_upvalue(name)))
            }
            Expr::Nil
            | Expr::True
            | Expr::False
            | Expr::Vararg
            | Expr::Number(_)
            | Expr::Str(_) => true,
            Expr::Name(name) if is_temp(name) => true,
            Expr::Name(name) => match read {
                Read::Copy => false,
                Read::Operand => self.is_local(name),
                Read::Table => self.is_local(name) || self.is_upvalue(name),
            },
            _ => false,
        }
    }

    /// Rewrites two operands Lua evaluates in order, spilling the first if the second moves
    /// calls out and the first would otherwise be read after them.
    fn ordered(&mut self, first: &Expr, second: &Expr, read: Read) -> (Expr, Expr) {
        let mut first = self.value(first);
        let mark = self.pre.len();
        let second = self.value(second);
        if self.pre.len() > mark && !self.stable(&first, read) {
            first = self.spill_at(mark, first);
        }
        (first, second)
    }

    fn binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Expr {
        if matches!(op, BinOp::And | BinOp::Or) && has_call(right) {
            return self.short_circuit(op, left, right);
        }
        let read = if op == BinOp::Concat {
            Read::Copy
        } else {
            Read::Operand
        };
        let (left, right) = self.ordered(left, right, read);
        Expr::Binary(op, Box::new(left), Box::new(right))
    }

    /// `a and b` with calls in `b` becomes `t = a; if t then t = b end`; `or` tests `not t`.
    fn short_circuit(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Expr {
        let left = self.value(left);
        let temp = self.temp();
        let first = self.assign(&temp, left);
        self.pre.push(first);
        let outer = mem::take(&mut self.pre);
        let right = self.value(right);
        let mut body = mem::replace(&mut self.pre, outer);
        body.push(self.assign(&temp, right));
        let test = Expr::Name(temp.clone());
        let cond = if op == BinOp::And {
            test
        } else {
            Expr::Unary(UnOp::Not, Box::new(test))
        };
        let arm = IfArm {
            line: self.line,
            cond,
            body: Block {
                stats: body,
                ret: None,
            },
        };
        let kind = StatKind::If {
            arms: vec![arm],
            otherwise: None,
        };
        self.pre.push(Stat {
            line: self.line,
            kind,
        });
        Expr::Name(temp)
    }

    /// Looks up a method before any argument is moved out, then rewrites the argument list.
    pub(super) fn call(&mut self, call: &Call) -> Call {
        let receiver = self.value(&call.callee);
        if let Some(method) = &call.method
            && call.args.iter().any(has_call)
        {
            let receiver = self.hoist(receiver);
            let lookup = Expr::Field(Box::new(receiver.clone()), method.clone());
            let callee = self.hoist(lookup);
            let mut args = vec![receiver];
            args.extend(self.chain_list(&call.args));
            return Call {
                line: call.line,
                callee,
                method: None,
                args,
            };
        }
        let mut items = vec![receiver];
        for arg in &call.args {
            let mark = self.pre.len();
            let arg = self.chain(arg);
            if self.pre.len() > mark || has_call(&arg) {
                self.spill_all(&mut items, mark);
            }
            items.push(arg);
        }
        let callee = items.remove(0);
        Call {
            line: call.line,
            callee,
            method: call.method.clone(),
            args: items,
        }
    }

    /// Rewrites a table constructor in a chain position. Positional items Lua still holds in
    /// registers keep their calls; named and keyed fields before such an item are spilled,
    /// because Lua stores them in the table before the call runs.
    fn table(&mut self, fields: &[Field]) -> Expr {
        let mut out: Vec<Field> = Vec::with_capacity(fields.len());
        let mut checked = 0;
        for field in fields {
            let mark = self.pre.len();
            let field = self.field(field);
            let keeps_call = matches!(&field, Field::Item(value) if has_call(value));
            if self.pre.len() > mark || keeps_call {
                self.spill_fields(&mut out, mark, checked);
                checked = out.len();
            }
            out.push(field);
        }
        Expr::Table(out)
    }

    fn field(&mut self, field: &Field) -> Field {
        match field {
            Field::Item(value) => Field::Item(self.chain(value)),
            Field::Named(name, value) => Field::Named(name.clone(), self.value(value)),
            Field::Keyed(key, value) => {
                let (key, value) = self.ordered(key, value, Read::Operand);
                Field::Keyed(key, value)
            }
        }
    }

    fn spill_fields(&mut self, fields: &mut [Field], mut at: usize, checked: usize) {
        for (index, field) in fields.iter_mut().enumerate() {
            match field {
                Field::Item(value) | Field::Named(_, value) => self.spill_one(value, &mut at),
                Field::Keyed(key, value) => {
                    if !self.stable(key, Read::Operand) {
                        self.spill_one(key, &mut at);
                    }
                    self.spill_one(value, &mut at);
                    self.spill_one(key, &mut at);
                    if index >= checked {
                        let validation =
                            Expr::Table(vec![Field::Keyed(key.clone(), value.clone())]);
                        self.spill_at(at, validation);
                        at += 1;
                    }
                }
            }
        }
    }
}
