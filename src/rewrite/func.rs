//! The state of the rewrite within one function: its scopes, temporaries and pending statements.

use std::rc::Rc;

use super::analysis::{self, Reads};
use super::{RewriteError, SITE, TEMP_PREFIX, budget, storage, walk};
use crate::compiled::Compiled;
use crate::syntax::ast::{Block, Expr, FuncBody, LocalName, Name, Stat, StatKind};

pub(super) struct Func {
    /// Locals of the enclosing functions, which are upvalues here.
    outer: Vec<Name>,
    /// The locals in scope, innermost block last.
    scopes: Vec<Vec<Name>>,
    /// Statements to place before the statement being rewritten.
    pub(super) pre: Vec<Stat>,
    /// The line of the statement being rewritten, given to the statements generated for it.
    pub(super) line: u32,
    /// Temporaries used by the statement being rewritten. Each statement reuses them.
    temps: u32,
    max_temps: u32,
    loops: u32,
    /// Original compiler decisions, indexed by the borrowed AST assignment target.
    pub(super) reads: Rc<Reads>,
    /// The first construct this function or a nested one cannot rewrite.
    error: Option<RewriteError>,
}

impl Func {
    fn new(outer: Vec<Name>, reads: Rc<Reads>) -> Self {
        Self {
            outer,
            reads,
            error: None,
            scopes: Vec::new(),
            pre: Vec::new(),
            line: 1,
            temps: 0,
            max_temps: 0,
            loops: 0,
        }
    }

    /// Rewrites a main chunk.
    pub(super) fn chunk(chunk: &Block, compiled: &Compiled) -> Result<Block, RewriteError> {
        let reads = Rc::new(analysis::match_chunk(chunk, compiled)?);
        let outer = vec!["_ENV".to_owned()];
        let mut func = Self::new(outer, reads);
        let mut block = func.block(chunk);
        func.finish(&mut block, 1, 0);
        func.error.map_or(Ok(block), Err)
    }

    /// Rewrites a function body, which has its own temporaries and statement numbers.
    fn function(&mut self, body: &FuncBody, outer: Vec<Name>) -> FuncBody {
        let mut func = Self::new(outer, Rc::clone(&self.reads));
        func.line = body.line;
        func.push_scope();
        for param in &body.params {
            func.declare(param);
        }
        let mut block = func.block(&body.body);
        func.pop_scope();
        func.finish(&mut block, body.line, body.params.len());
        if self.error.is_none() {
            self.error = func.error;
        }
        FuncBody {
            line: body.line,
            end_line: body.end_line,
            params: body.params.clone(),
            vararg: body.vararg,
            body: block,
        }
    }

    /// Rewrites a function defined inside this one.
    pub(super) fn nested(&mut self, body: &FuncBody) -> FuncBody {
        let mut outer = self.outer.clone();
        outer.extend(self.scopes.iter().flatten().cloned());
        self.function(body, outer)
    }

    /// Records that the statement being rewritten cannot be rewritten faithfully.
    pub(super) fn refuse(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(RewriteError {
                line: self.line,
                message,
            });
        }
    }

    /// Numbers the statements and declares `__lrs` and the temporaries at the start.
    fn finish(&self, block: &mut Block, line: u32, params: usize) {
        let sites = walk::number_sites(block);
        let mut names = Vec::new();
        if sites > 0 {
            names.push(SITE.to_owned());
        }
        let mut values = Vec::new();
        if self.max_temps > 32
            || budget::peak(block, params) + names.len() + self.max_temps as usize > 200
        {
            storage::spill(block);
            values.resize(names.len(), Expr::Nil);
            names.push(storage::STORAGE.to_owned());
            values.push(Expr::Table(Vec::new()));
        } else {
            names.extend((1..=self.max_temps).map(|i| format!("{TEMP_PREFIX}{i}")));
        }
        if names.is_empty() {
            return;
        }
        let names = names
            .into_iter()
            .map(|name| LocalName { name, attrib: None })
            .collect();
        let kind = StatKind::Local { names, values };
        block.stats.insert(0, Stat { line, kind });
    }

    pub(super) fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    pub(super) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub(super) fn declare(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.to_owned());
        }
    }

    /// Whether `name` is a local of this function here.
    pub(super) fn is_local(&self, name: &str) -> bool {
        is_temp(name) || self.scopes.iter().flatten().any(|local| local == name)
    }

    /// Whether `name` is a local of an enclosing function.
    pub(super) fn is_upvalue(&self, name: &str) -> bool {
        !self.is_local(name) && self.outer.iter().any(|local| local == name)
    }

    /// Starts rewriting a statement on `line`.
    pub(super) fn begin(&mut self, line: u32) {
        self.line = line;
        self.temps = 0;
    }

    pub(super) fn temp(&mut self) -> Name {
        self.temps += 1;
        self.max_temps = self.max_temps.max(self.temps);
        format!("{TEMP_PREFIX}{}", self.temps)
    }

    /// Names for the state of a rewritten generic `for` loop.
    pub(super) fn loop_names(&mut self) -> [Name; 4] {
        self.loops += 1;
        ["f", "s", "c", "x"].map(|part| format!("{TEMP_PREFIX}_{part}{}", self.loops))
    }

    /// `name = value` on the current line.
    pub(super) fn assign(&self, name: &str, value: Expr) -> Stat {
        Stat {
            line: self.line,
            kind: StatKind::Assign {
                targets: vec![Expr::Name(name.to_owned())],
                values: vec![value],
            },
        }
    }
}

/// Whether `name` is one of the locals the rewrite adds.
pub(super) fn is_temp(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX)
}
