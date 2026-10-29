//! Prints a syntax tree back to Lua source.
//!
//! The output compiles to the same code as the tree: parentheses are written where the tree
//! has them, and added elsewhere only where operator priority requires them. Each statement,
//! `function` keyword and closing `end` of a function is placed on the line it had in the
//! source when the output has not already passed that line, so line numbers in errors and in
//! `debug.getinfo` match the original source.

use std::fmt::Write as _;

use super::ast::{
    Attrib, BinOp, Block, Call, Expr, Field, FuncBody, LocalName, Stat, StatKind, UNARY_PRIORITY,
    UnOp,
};
use super::hooks::OperatorHooks;

/// Prints a chunk.
pub fn print(chunk: &Block) -> String {
    print_with(chunk, &OperatorHooks::none())
}

/// Prints a chunk with the operators `hooks` names written as calls of host functions.
pub fn print_with(chunk: &Block, hooks: &OperatorHooks) -> String {
    let mut printer = Printer {
        out: String::new(),
        line: 1,
        hooks,
    };
    printer.block(chunk);
    printer.out.push('\n');
    printer.out
}

struct Printer<'a> {
    out: String,
    line: u32,
    hooks: &'a OperatorHooks,
}

/// Whether an expression can be printed as a prefix (callee, indexed object) without parentheses.
fn is_prefix(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Name(_) | Expr::Paren(_) | Expr::Call(_) | Expr::Field(..) | Expr::Index(..)
    )
}

/// Whether the printed expression starts with `(`, which would continue a previous statement.
fn leads_with_paren(expr: &Expr) -> bool {
    let prefix_leads = |inner: &Expr| !is_prefix(inner) || leads_with_paren(inner);
    match expr {
        Expr::Paren(_) => true,
        Expr::Field(obj, _) | Expr::Index(obj, _) => prefix_leads(obj),
        Expr::Call(call) => prefix_leads(&call.callee),
        _ => false,
    }
}

fn stat_leads_with_paren(stat: &Stat) -> bool {
    match &stat.kind {
        StatKind::Call(call) => leads_with_paren(&Expr::Call(call.clone())),
        StatKind::Assign { targets, .. } => targets.first().is_some_and(leads_with_paren),
        _ => false,
    }
}

impl Printer<'_> {
    fn text(&mut self, text: &str) {
        self.out.push_str(text);
    }

    /// Moves the output down to `line` if it is above it.
    fn at_line(&mut self, line: u32) {
        while self.line < line {
            self.out.push('\n');
            self.line += 1;
        }
        if !self.out.is_empty() && !self.out.ends_with(['\n', ' ', '(', '[', '{']) {
            self.out.push(' ');
        }
    }

    fn block(&mut self, block: &Block) {
        for stat in &block.stats {
            self.stat(stat);
        }
        if let Some(ret) = &block.ret {
            self.at_line(ret.line);
            self.text("return");
            if !ret.values.is_empty() {
                self.text(" ");
                self.exprs(&ret.values);
            }
        }
    }

    fn stat(&mut self, stat: &Stat) {
        self.at_line(stat.line);
        if stat_leads_with_paren(stat) {
            self.text(";");
        }
        match &stat.kind {
            StatKind::Assign { targets, values } => {
                self.exprs(targets);
                self.text(" = ");
                self.exprs(values);
            }
            StatKind::Call(call) => self.call(call),
            StatKind::Label(name) => self.text(&format!("::{name}::")),
            StatKind::Goto(name) => self.text(&format!("goto {name}")),
            StatKind::Break => self.text("break"),
            StatKind::Do(body) => self.wrapped("do", body, "end"),
            StatKind::While { cond, body } => {
                self.text("while ");
                self.expr(cond);
                self.wrapped(" do", body, "end");
            }
            StatKind::Repeat { body, cond } => {
                self.wrapped("repeat", body, "until ");
                self.expr(cond);
            }
            StatKind::If { arms, otherwise } => self.if_stat(arms, otherwise.as_ref()),
            _ => self.declaration(&stat.kind),
        }
    }

    fn declaration(&mut self, kind: &StatKind) {
        match kind {
            StatKind::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                self.text(&format!("for {var} = "));
                self.expr(start);
                self.text(", ");
                self.expr(limit);
                if let Some(step) = step {
                    self.text(", ");
                    self.expr(step);
                }
                self.wrapped(" do", body, "end");
            }
            StatKind::GenericFor { names, exprs, body } => {
                self.text(&format!("for {} in ", names.join(", ")));
                self.exprs(exprs);
                self.wrapped(" do", body, "end");
            }
            StatKind::Function { name, body } => {
                self.text("function ");
                self.text(&name.path.join("."));
                if let Some(method) = &name.method {
                    self.text(&format!(":{method}"));
                }
                self.func_body(body, name.method.is_some());
            }
            StatKind::LocalFunction { name, body } => {
                self.text(&format!("local function {name}"));
                self.func_body(body, false);
            }
            StatKind::Local { names, values } => self.local(names, values),
            _ => unreachable!("statements are dispatched in `stat`"),
        }
    }

    fn wrapped(&mut self, open: &str, body: &Block, close: &str) {
        self.text(open);
        self.block(body);
        self.out.push(' ');
        self.text(close);
    }

    fn if_stat(&mut self, arms: &[super::ast::IfArm], otherwise: Option<&Block>) {
        for (i, arm) in arms.iter().enumerate() {
            if i > 0 {
                self.at_line(arm.line);
                self.text("else");
            }
            self.text("if ");
            self.expr(&arm.cond);
            self.text(" then");
            self.block(&arm.body);
        }
        if let Some(otherwise) = otherwise {
            self.text(" else");
            self.block(otherwise);
        }
        self.text(" end");
    }

    fn local(&mut self, names: &[LocalName], values: &[Expr]) {
        let names: Vec<String> = names
            .iter()
            .map(|local| match local.attrib {
                None => local.name.clone(),
                Some(Attrib::Const) => format!("{} <const>", local.name),
                Some(Attrib::Close) => format!("{} <close>", local.name),
            })
            .collect();
        self.text(&format!("local {}", names.join(", ")));
        if !values.is_empty() {
            self.text(" = ");
            self.exprs(values);
        }
    }

    /// Prints `(params) body end`, leaving out the implicit `self` of a method.
    fn func_body(&mut self, body: &FuncBody, method: bool) {
        let mut params: Vec<&str> = body
            .params
            .iter()
            .skip(usize::from(method))
            .map(String::as_str)
            .collect();
        if body.vararg {
            params.push("...");
        }
        self.text(&format!("({})", params.join(", ")));
        self.block(&body.body);
        self.at_line(body.end_line);
        self.text("end");
    }

    fn exprs(&mut self, exprs: &[Expr]) {
        for (i, expr) in exprs.iter().enumerate() {
            if i > 0 {
                self.text(", ");
            }
            self.expr(expr);
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Nil => self.text("nil"),
            Expr::True => self.text("true"),
            Expr::False => self.text("false"),
            Expr::Vararg => self.text("..."),
            Expr::Number(text) => self.text(text),
            Expr::Str(bytes) => self.text(&quote(bytes)),
            Expr::Function(body) => {
                self.at_line(body.line);
                self.text("function");
                self.func_body(body, false);
            }
            Expr::Table(fields) => self.table(fields),
            Expr::Binary(op, left, right) => self.binary(*op, left, right),
            Expr::Unary(op, operand) => self.unary(*op, operand),
            Expr::Name(name) => self.text(name),
            Expr::Field(obj, name) => {
                self.prefix(obj);
                self.text(&format!(".{name}"));
            }
            Expr::Index(obj, key) => {
                self.prefix(obj);
                self.text("[");
                self.expr(key);
                self.text("]");
            }
            Expr::Call(call) => self.call(call),
            Expr::Paren(inner) => self.parenthesized(inner),
        }
    }

    fn parenthesized(&mut self, expr: &Expr) {
        self.text("(");
        self.expr(expr);
        self.text(")");
    }

    fn prefix(&mut self, expr: &Expr) {
        if is_prefix(expr) {
            self.expr(expr);
        } else {
            self.parenthesized(expr);
        }
    }

    fn call(&mut self, call: &Call) {
        self.prefix(&call.callee);
        if let Some(method) = &call.method {
            self.text(&format!(":{method}"));
        }
        self.text("(");
        self.exprs(&call.args);
        self.text(")");
    }

    fn table(&mut self, fields: &[Field]) {
        self.text("{");
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                self.text(", ");
            }
            match field {
                Field::Item(value) => self.expr(value),
                Field::Named(name, value) => {
                    self.text(&format!("{name} = "));
                    self.expr(value);
                }
                Field::Keyed(key, value) => {
                    self.text("[");
                    self.expr(key);
                    self.text("] = ");
                    self.expr(value);
                }
            }
        }
        self.text("}");
    }

    fn binary(&mut self, op: BinOp, left: &Expr, right: &Expr) {
        if let (BinOp::Div, Some(hook)) = (op, self.hooks.divide()) {
            let hook = hook.to_owned();
            return self.hooked(&hook, left, right);
        }
        let (left_priority, right_priority) = op.priority();
        let left_parens = match left {
            Expr::Binary(inner, ..) => left_priority > inner.priority().1,
            Expr::Unary(..) => left_priority > UNARY_PRIORITY,
            _ => false,
        };
        let right_parens = match right {
            Expr::Binary(inner, ..) => inner.priority().0 <= right_priority,
            _ => false,
        };
        self.operand(left, left_parens);
        self.text(&format!(" {} ", op.text()));
        self.operand(right, right_parens);
    }

    /// `(hook(left, right))`: arguments are evaluated left to right as the operands were, and
    /// the parentheses keep the call from being a tail call or yielding several values.
    fn hooked(&mut self, hook: &str, left: &Expr, right: &Expr) {
        self.text(&format!("({hook}("));
        self.expr(left);
        self.text(", ");
        self.expr(right);
        self.text("))");
    }

    fn unary(&mut self, op: UnOp, operand: &Expr) {
        self.text(op.text());
        if op == UnOp::Neg && matches!(operand, Expr::Unary(UnOp::Neg, _)) {
            // `--` would start a comment.
            self.text(" ");
        }
        let parens =
            matches!(operand, Expr::Binary(inner, ..) if inner.priority().0 <= UNARY_PRIORITY);
        self.operand(operand, parens);
    }

    fn operand(&mut self, expr: &Expr, parens: bool) {
        if parens {
            self.parenthesized(expr);
        } else {
            self.expr(expr);
        }
    }
}

/// A double-quoted Lua string literal for `bytes`, escaping everything outside printable ASCII.
fn quote(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\{b:03}");
            }
        }
    }
    out.push('"');
    out
}
