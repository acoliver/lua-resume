//! Match compiler store occurrences to AST targets, including stores in constructors.
//!
//! Traversal follows source compilation order. Assignment stores follow their RHS in reverse
//! target order. Each nested prototype is consumed at its definition, independently of stores
//! in its parent. Pointer keys identify borrowed AST nodes only during this normalization.

use std::collections::HashMap;

use super::{RewriteError, walk};
use crate::compiled::Compiled;
use crate::syntax::ast::{Block, Expr, Field, FuncBody, Stat, StatKind};

pub(super) type Reads = HashMap<*const Expr, bool>;

pub(super) fn match_chunk(chunk: &Block, compiled: &Compiled) -> Result<Reads, RewriteError> {
    let mut scanner = Scanner::new(compiled, vec!["_ENV".to_owned()]);
    scanner.block(chunk)?;
    scanner.finish(0)?;
    Ok(scanner.reads)
}

struct Scanner<'a> {
    compiled: &'a Compiled,
    names: Vec<String>,
    occurrences: Vec<Option<*const Expr>>,
    functions: usize,
    reads: Reads,
}

fn mismatch(line: u32, message: &str) -> RewriteError {
    RewriteError {
        line,
        message: format!("the compiled chunk does not match the source: {message}"),
    }
}

impl<'a> Scanner<'a> {
    fn new(compiled: &'a Compiled, names: Vec<String>) -> Self {
        Self {
            compiled,
            names,
            occurrences: Vec::new(),
            functions: 0,
            reads: Reads::new(),
        }
    }

    fn finish(&mut self, line: u32) -> Result<(), RewriteError> {
        let source = self.occurrences.len();
        let analysed = self.compiled.stores.len();
        if source != analysed {
            return Err(mismatch(
                line,
                &format!(
                    "function has {source} store occurrences in the AST and {analysed} in the analysis"
                ),
            ));
        }
        if self.functions != self.compiled.functions.len() {
            return Err(mismatch(
                line,
                &format!(
                    "function has {} nested functions in the AST and {} in the analysis",
                    self.functions,
                    self.compiled.functions.len()
                ),
            ));
        }
        for (target, late) in self.occurrences.iter().zip(&self.compiled.stores) {
            if let Some(target) = target {
                self.reads.insert(*target, *late);
            }
        }
        Ok(())
    }

    fn nested(&mut self, body: &FuncBody) -> Result<(), RewriteError> {
        let compiled = self
            .compiled
            .functions
            .get(self.functions)
            .ok_or_else(|| mismatch(body.line, "missing nested function prototype"))?;
        self.functions += 1;
        if compiled.line != body.line as usize {
            return Err(mismatch(
                body.line,
                &format!(
                    "nested prototype linedefined is {}, AST function starts at {}",
                    compiled.line, body.line
                ),
            ));
        }
        let mut names = self.names.clone();
        names.extend(body.params.iter().cloned());
        let mut scanner = Self::new(compiled, names);
        scanner.block(&body.body)?;
        scanner.finish(body.line)?;
        self.reads.extend(scanner.reads);
        Ok(())
    }

    fn block(&mut self, block: &Block) -> Result<(), RewriteError> {
        let scope = self.names.len();
        for stat in &block.stats {
            self.stat(stat)?;
        }
        for expr in block.ret.iter().flat_map(|ret| &ret.values) {
            self.expr(expr)?;
        }
        self.names.truncate(scope);
        Ok(())
    }

    fn indexed(&self, target: &Expr) -> bool {
        match target {
            Expr::Name(name) => !self.names.contains(name),
            Expr::Field(..) | Expr::Index(..) => true,
            _ => false,
        }
    }

    fn stat(&mut self, stat: &Stat) -> Result<(), RewriteError> {
        match &stat.kind {
            StatKind::Assign { targets, values } => {
                for expr in targets.iter().chain(values) {
                    self.expr(expr)?;
                }
                for target in targets
                    .iter()
                    .rev()
                    .filter(|t| self.indexed(t))
                    .collect::<Vec<_>>()
                {
                    self.occurrences.push(Some(std::ptr::from_ref(target)));
                }
            }
            StatKind::Local { names, values } => {
                for value in values {
                    self.expr(value)?;
                }
                self.names
                    .extend(names.iter().map(|name| name.name.clone()));
            }
            StatKind::LocalFunction { name, body } => {
                self.names.push(name.clone());
                self.nested(body)?;
            }
            StatKind::Function { name, body } => {
                self.nested(body)?;
                if name.path.len() > 1
                    || name.method.is_some()
                    || !self.names.contains(&name.path[0])
                {
                    self.occurrences.push(None);
                }
            }
            _ => self.control(stat)?,
        }
        Ok(())
    }

    fn control(&mut self, stat: &Stat) -> Result<(), RewriteError> {
        match &stat.kind {
            StatKind::Repeat { body, cond } => {
                let scope = self.names.len();
                for stat in &body.stats {
                    self.stat(stat)?;
                }
                for expr in body.ret.iter().flat_map(|ret| &ret.values) {
                    self.expr(expr)?;
                }
                self.expr(cond)?;
                self.names.truncate(scope);
            }
            StatKind::If { arms, otherwise } => {
                for arm in arms {
                    self.expr(&arm.cond)?;
                    self.block(&arm.body)?;
                }
                if let Some(body) = otherwise {
                    self.block(body)?;
                }
            }
            _ => self.loop_or_plain(stat)?,
        }
        Ok(())
    }

    fn loop_or_plain(&mut self, stat: &Stat) -> Result<(), RewriteError> {
        for expr in walk::expressions(stat) {
            self.expr(expr)?;
        }
        let scope = self.names.len();
        match &stat.kind {
            StatKind::NumericFor { var, .. } => self.names.push(var.clone()),
            StatKind::GenericFor { names, .. } => self.names.extend(names.iter().cloned()),
            _ => {}
        }
        for body in walk::blocks(stat) {
            self.block(body)?;
        }
        self.names.truncate(scope);
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<(), RewriteError> {
        match expr {
            Expr::Function(body) => self.nested(body)?,
            Expr::Table(fields) => {
                for field in fields {
                    match field {
                        Field::Item(value) => self.expr(value)?,
                        Field::Named(_, value) => {
                            self.expr(value)?;
                            self.occurrences.push(None);
                        }
                        Field::Keyed(key, value) => {
                            self.expr(key)?;
                            self.expr(value)?;
                            self.occurrences.push(None);
                        }
                    }
                }
            }
            _ => {
                for child in walk::children(expr) {
                    self.expr(child)?;
                }
            }
        }
        Ok(())
    }
}
