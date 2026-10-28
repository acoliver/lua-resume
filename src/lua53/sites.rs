//! The calls of each source function in the order Lua evaluates them, which is the order of
//! the call-class instructions in the compiled function.
//!
//! Operands are evaluated before the call that uses them, so an expression yields its callee
//! and arguments before itself. A generic `for` yields the calls in its expressions, then the
//! calls in its body, then its implicit iterator call, because the compiler places that call
//! after the body. Calls inside a nested function belong to that function.

use super::Proto;
use super::op::{CallKind, call_sequence};
use crate::rewrite::SITE;
use crate::syntax::ast::{Block, Call, Expr, Field, FuncBody, Stat, StatKind};

/// What kind of call a site is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteKind {
    Call,
    /// The implicit iterator call of a generic `for`.
    Iterator,
}

impl SiteKind {
    fn matches(self, kind: CallKind) -> bool {
        match self {
            Self::Call => matches!(kind, CallKind::Call | CallKind::TailCall),
            Self::Iterator => kind == CallKind::IterCall,
        }
    }
}

/// One call in a function, in evaluation order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub kind: SiteKind,
    pub line: u32,
    /// The `__lrs` statement number in force at the call, when the site comes from a
    /// normalized chunk.
    pub marker: Option<u32>,
}

/// The calls of one function, identified by the lines of `function` and `end`. The main
/// chunk has both lines 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSites {
    pub line_defined: u32,
    pub last_line_defined: u32,
    pub calls: Vec<CallSite>,
}

/// A saved frame that does not correspond to the source.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SiteError {
    #[error("no function in the source spans lines {line_defined} to {last_line_defined}")]
    UnknownFunction {
        line_defined: u32,
        last_line_defined: u32,
    },
    #[error("{count} functions in the source span lines {line_defined} to {last_line_defined}")]
    AmbiguousFunction {
        line_defined: u32,
        last_line_defined: u32,
        count: usize,
    },
    #[error("call {ordinal} requested, but the function has {count} calls")]
    NoSuchCall { ordinal: usize, count: usize },
    #[error("the compiled function has {compiled} calls and the source function has {in_source}")]
    CallCountMismatch { compiled: usize, in_source: usize },
    #[error("call {ordinal} is a different kind of call in the compiled function and the source")]
    CallKindMismatch { ordinal: usize },
    #[error("the normalized chunk has different functions or calls than the original")]
    NormalizedMismatch,
}

/// Every function of a chunk with its calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionIndex {
    functions: Vec<FunctionSites>,
}

impl FunctionIndex {
    /// Indexes the chunk after `assert` and `dbg` statements were removed from it.
    pub fn new(stripped: &Block) -> Self {
        Self {
            functions: collect(stripped).into_iter().map(Raw::finish).collect(),
        }
    }

    /// Indexes the stripped chunk and records, for each call, the statement number the
    /// normalized chunk assigns to it. The two chunks must have the same functions and calls.
    /// The normalized chunk calls a generic `for` iterator at the top of the loop body as an
    /// ordinary call; the index still lists it where the compiled function has it.
    pub fn with_normalized(stripped: &Block, normalized: &Block) -> Result<Self, SiteError> {
        let mut functions = collect(stripped);
        let normalized = collect(normalized);
        let same = functions.len() == normalized.len()
            && functions.iter().zip(&normalized).all(|(a, b)| {
                a.line_defined == b.line_defined
                    && a.last_line_defined == b.last_line_defined
                    && a.calls.len() == b.calls.len()
            });
        if !same {
            return Err(SiteError::NormalizedMismatch);
        }
        for (function, other) in functions.iter_mut().zip(&normalized) {
            for (call, found) in function.calls.iter_mut().zip(&other.calls) {
                call.marker = found.marker;
            }
        }
        Ok(Self {
            functions: functions.into_iter().map(Raw::finish).collect(),
        })
    }

    /// The function defined on exactly these lines.
    pub fn function(
        &self,
        line_defined: u32,
        last_line_defined: u32,
    ) -> Result<&FunctionSites, SiteError> {
        let mut found = self
            .functions
            .iter()
            .filter(|f| f.line_defined == line_defined && f.last_line_defined == last_line_defined);
        match (found.next(), found.count()) {
            (Some(function), 0) => Ok(function),
            (Some(_), more) => Err(SiteError::AmbiguousFunction {
                line_defined,
                last_line_defined,
                count: more + 1,
            }),
            (None, _) => Err(SiteError::UnknownFunction {
                line_defined,
                last_line_defined,
            }),
        }
    }

    /// Finds the source function for a prototype and checks that its call-class instructions
    /// match the source calls in number and kind.
    pub fn function_for<K, C>(&self, proto: &Proto<K, C>) -> Result<&FunctionSites, SiteError> {
        let function = self.function(proto.line_defined, proto.last_line_defined)?;
        let compiled = call_sequence(&proto.code);
        if compiled.len() != function.calls.len() {
            return Err(SiteError::CallCountMismatch {
                compiled: compiled.len(),
                in_source: function.calls.len(),
            });
        }
        for (ordinal, (kind, site)) in compiled.iter().zip(&function.calls).enumerate() {
            if !site.kind.matches(*kind) {
                return Err(SiteError::CallKindMismatch { ordinal });
            }
        }
        Ok(function)
    }
}

impl FunctionSites {
    /// The call with this ordinal.
    pub fn call(&self, ordinal: usize) -> Result<&CallSite, SiteError> {
        self.calls.get(ordinal).ok_or(SiteError::NoSuchCall {
            ordinal,
            count: self.calls.len(),
        })
    }
}

/// A function's calls in the order the normalized chunk makes them, with the order the
/// compiled function makes them. The two differ only for generic `for` iterator calls.
struct Raw {
    line_defined: u32,
    last_line_defined: u32,
    calls: Vec<CallSite>,
    evaluation: Vec<usize>,
    marker: Option<u32>,
}

impl Raw {
    fn finish(self) -> FunctionSites {
        FunctionSites {
            line_defined: self.line_defined,
            last_line_defined: self.last_line_defined,
            calls: self
                .evaluation
                .iter()
                .map(|&at| self.calls[at].clone())
                .collect(),
        }
    }
}

fn collect(chunk: &Block) -> Vec<Raw> {
    let mut collector = Collector {
        functions: Vec::new(),
    };
    collector.function(0, 0, chunk);
    collector.functions
}

struct Collector {
    functions: Vec<Raw>,
}

impl Collector {
    fn function(&mut self, line_defined: u32, last_line_defined: u32, body: &Block) {
        let id = self.functions.len();
        self.functions.push(Raw {
            line_defined,
            last_line_defined,
            calls: Vec::new(),
            evaluation: Vec::new(),
            marker: None,
        });
        self.block(id, body);
    }

    fn nested(&mut self, body: &FuncBody) {
        self.function(body.line, body.end_line, &body.body);
    }

    /// Adds a call to the normalized order and returns its position in it.
    fn reserve(&mut self, id: usize, kind: SiteKind, line: u32) -> usize {
        let function = &mut self.functions[id];
        let marker = function.marker;
        function.calls.push(CallSite { kind, line, marker });
        function.calls.len() - 1
    }

    fn push(&mut self, id: usize, kind: SiteKind, line: u32) {
        let at = self.reserve(id, kind, line);
        self.functions[id].evaluation.push(at);
    }

    fn block(&mut self, id: usize, block: &Block) {
        for stat in &block.stats {
            self.stat(id, stat);
        }
        if let Some(ret) = &block.ret {
            self.exprs(id, &ret.values);
        }
    }

    fn stat(&mut self, id: usize, stat: &Stat) {
        match &stat.kind {
            StatKind::Assign { targets, values } => {
                if let Some(number) = site_marker(targets, values) {
                    self.functions[id].marker = Some(number);
                }
                self.exprs(id, targets);
                self.exprs(id, values);
            }
            StatKind::Call(call) => self.call(id, call),
            StatKind::Do(body) => self.block(id, body),
            StatKind::While { cond, body } => {
                self.expr(id, cond);
                self.block(id, body);
            }
            StatKind::Repeat { body, cond } => {
                self.block(id, body);
                self.expr(id, cond);
            }
            StatKind::If { arms, otherwise } => {
                for arm in arms {
                    self.expr(id, &arm.cond);
                    self.block(id, &arm.body);
                }
                if let Some(body) = otherwise {
                    self.block(id, body);
                }
            }
            _ => self.binding(id, stat),
        }
    }

    fn binding(&mut self, id: usize, stat: &Stat) {
        match &stat.kind {
            StatKind::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.expr(id, start);
                self.expr(id, limit);
                if let Some(step) = step {
                    self.expr(id, step);
                }
                self.block(id, body);
            }
            StatKind::GenericFor { exprs, body, .. } => {
                self.exprs(id, exprs);
                let at = self.reserve(id, SiteKind::Iterator, stat.line);
                self.block(id, body);
                self.functions[id].evaluation.push(at);
            }
            StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => {
                self.nested(body);
            }
            StatKind::Local { values, .. } => self.exprs(id, values),
            _ => {}
        }
    }

    fn call(&mut self, id: usize, call: &Call) {
        self.expr(id, &call.callee);
        self.exprs(id, &call.args);
        self.push(id, SiteKind::Call, call.line);
    }

    fn exprs(&mut self, id: usize, exprs: &[Expr]) {
        for expr in exprs {
            self.expr(id, expr);
        }
    }

    fn expr(&mut self, id: usize, expr: &Expr) {
        match expr {
            Expr::Function(body) => self.nested(body),
            Expr::Table(fields) => {
                for field in fields {
                    match field {
                        Field::Item(value) | Field::Named(_, value) => self.expr(id, value),
                        Field::Keyed(key, value) => {
                            self.expr(id, key);
                            self.expr(id, value);
                        }
                    }
                }
            }
            Expr::Binary(_, left, right) | Expr::Index(left, right) => {
                self.expr(id, left);
                self.expr(id, right);
            }
            Expr::Unary(_, inner) | Expr::Paren(inner) | Expr::Field(inner, _) => {
                self.expr(id, inner);
            }
            Expr::Call(call) => self.call(id, call),
            _ => {}
        }
    }
}

/// The number in a statement `__lrs = N`.
fn site_marker(targets: &[Expr], values: &[Expr]) -> Option<u32> {
    match (targets, values) {
        ([Expr::Name(name)], [Expr::Number(number)]) if name == SITE => number.parse().ok(),
        _ => None,
    }
}
