//! Normalizes explicit calls into source-level suspension sites.
//!
//! Original compiler store decisions come from [`Compiled`], matched per function and per
//! occurrence before rewriting. Implicit metamethod calls and garbage-collector/debug
//! observations are outside this syntactic pass.
//!
//! Explicit calls are placed in a *chain position*, where values already computed for the
//! rest of the statement are held in registers in source order:
//!
//! - a call statement;
//! - a value in the list of a `local`, a `return`, or an assignment to local variables;
//! - an argument of a call, or a positional item of a table constructor, that is itself in a
//!   chain position.
//!
//! A call anywhere else, such as an operand of an operator, a condition, a table key, the
//! object of a method call or the header of a loop, is moved into a new local `__lrN` in a
//! statement of its own before the statement that used it. Operands that Lua evaluates before
//! that call are first copied into new locals too, so values are computed and functions called
//! in the original order. A statement may keep calls in at most one argument or item of each
//! list; calls in earlier ones are moved out.
//!
//! Every statement that contains a call is preceded by `__lrs = N`, which numbers the
//! statements of each function, so a saved frame records which statement it is suspended in.
//!
//! Temporaries are declared at function entry and reused between statements, preserving goto
//! scope. Large expressions spill into a private `__lrt` table to stay within Lua's local
//! limit. Source variables beginning with `__lr` are reserved. Error messages that name an
//! operand may name a generated temporary. Insufficient local capacity is reported explicitly.

mod analysis;
mod assign;
mod budget;
mod check;
mod expr;
mod func;
mod stat;
mod storage;
mod walk;

pub use check::check_normal_form;
pub(crate) use walk::has_call;

use crate::compiled::Compiled;
use crate::syntax::ast::Block;

/// The prefix of the locals the rewrite adds. Source that uses it for a variable is refused.
pub const TEMP_PREFIX: &str = "__lr";

/// The local that holds the number of the statement a function is executing.
pub const SITE: &str = "__lrs";

/// Source the rewrite refuses.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct RewriteError {
    pub line: u32,
    pub message: String,
}

/// Rewrites a chunk into the form described in the module documentation. `compiled` is the
/// same chunk as compiled by Lua, which decides when Lua reads some assignment targets.
pub fn normalize(chunk: &Block, compiled: &Compiled) -> Result<Block, RewriteError> {
    walk::reject_reserved(chunk)?;
    let rewritten = func::Func::chunk(chunk, compiled)?;
    budget::check(&rewritten, 0, 1)?;
    Ok(rewritten)
}
