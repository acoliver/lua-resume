//! Generates the continuation of a normalized function at one of its call sites.
//!
//! A continuation is a Lua function that declares the frame's saved locals, assigns them from
//! a table, re-enters the loops and branches that enclose the call statement and continues
//! right after it. The results of the call come from `__lr_next()`: the next inner frame, or
//! the `coroutine.yield` the innermost frame was waiting in.
//!
//! The generated chunk is `local <upvalues> return function(__lr_v, __lr_next, __lr_unpack, ...)`.
//! `__lr_v` is a table of the saved local values in declaration order, `__lr_unpack` is
//! `table.unpack` and `...` are the frame's saved varargs. The host assigns the upvalues.
//!
//! Supported enclosing constructs are `do`, `if`, `while` (which includes generic `for` after
//! normalization), `repeat` and numeric `for`. The site statement is a call, a `local` or an
//! assignment whose call is replaced by `__lr_next()` wherever it sits in the statement, or a
//! `return`. Labels before the site in an enclosing block are kept and entered with a `goto`.
//! Anything else is refused with an error that names the construct.
//!
//! A frame suspended in `pcall(f, ...)` has the `pcall` call as its site. The host builds the
//! `__lr_next` of that frame with [`PROTECTED_NEXT`], so the continuation of `f` runs inside a
//! new `pcall` and an error raised after the restore is caught like the original.

mod find;
mod loops;
mod path;
mod resume;
mod site;

use crate::syntax::ast::{Block, Expr, Stat, StatKind};
use crate::syntax::{parse, print};
use path::{find_function, find_site, scope_at};

/// Why a continuation cannot be generated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationError {
    #[error("no function defined at lines {0} to {1}")]
    NoFunction(u32, u32),
    #[error(
        "{count} functions are defined at lines {line} to {end_line}; put them on different lines to tell them apart"
    )]
    Ambiguous {
        line: u32,
        end_line: u32,
        count: usize,
    },
    #[error("the function has no call site {0}")]
    NoSite(u32),
    #[error("cannot resume inside {0}")]
    Unsupported(&'static str),
}

/// The function a continuation is for, by the lines its definition spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionKey {
    pub line: u32,
    pub end_line: u32,
}

/// Generates the chunk source of the continuation of `function` at call site `site`.
/// `locals` are the names of the saved locals and `upvalues` those of the function.
/// For each name in `shared`, the continuation stores a function that has that local as its
/// only upvalue in `__lr_v.cells`, just before the pending call. The host joins the closures
/// that share the local's cell to it with `debug.upvaluejoin`.
pub fn continuation_source(
    chunk: &Block,
    function: FunctionKey,
    site: u32,
    locals: &[String],
    upvalues: &[String],
    shared: &[String],
) -> Result<String, ContinuationError> {
    let body = find_function(chunk, function)?;
    let steps = find_site(&body.body, site).ok_or(ContinuationError::NoSite(site))?;
    let mut stats = Vec::new();
    if !locals.is_empty() {
        let names = locals.join(", ");
        stats.extend(snippet(&format!(
            "local {names} = __lr_unpack(__lr_v, 1, {})",
            locals.len()
        )));
    }
    let binds: Vec<Stat> = shared
        .iter()
        .enumerate()
        .flat_map(|(index, name)| {
            snippet(&format!(
                "__lr_v.cells[{}] = function() return {name} end",
                index + 1
            ))
        })
        .collect();
    let rest = resume::resume_block(&body.body, &steps, (0, &binds))?;
    stats.extend(rest.stats);
    let block = Block {
        stats,
        ret: rest.ret,
    };
    let declared = if upvalues.is_empty() {
        String::new()
    } else {
        format!("local {} ", upvalues.join(", "))
    };
    Ok(format!(
        "{declared}return function(__lr_v, __lr_next, __lr_unpack, ...) {} end",
        print(&block)
    ))
}

/// Chunk source that evaluates to `function(inner, handler)`. It returns the `__lr_next` of a
/// frame suspended in `pcall(f, ...)`, or in `xpcall(f, handler, ...)` when `handler` is given.
/// `inner` is the continuation of the next frame, already applied to its own arguments.
pub const PROTECTED_NEXT: &str = "local pcall, xpcall = pcall, xpcall \
    return function(inner, handler) \
      return function() \
        if handler then return xpcall(inner, handler) end \
        return pcall(inner) \
      end \
    end";

/// The statements of a snippet. The snippet is trusted generated text.
pub(super) fn snippet(text: &str) -> Vec<Stat> {
    parse(text.as_bytes())
        .expect("generated snippet parses")
        .stats
}

/// Chunk source that evaluates to the function defined at `function`, with `upvalues` declared
/// as locals in the order given and every other free name left to `_ENV`. The function keeps
/// its definition lines, so the host can check it found the function the snapshot names.
pub fn function_source(
    chunk: &Block,
    function: FunctionKey,
    upvalues: &[String],
) -> Result<String, ContinuationError> {
    let body = find_function(chunk, function)?;
    let mut stats = Vec::new();
    if !upvalues.is_empty() {
        stats.extend(snippet(&format!("local {}", upvalues.join(", "))));
    }
    let mut definition = snippet("local __lr_f = nil");
    let StatKind::Local { values, .. } = &mut definition[0].kind else {
        unreachable!("the snippet is a local")
    };
    values[0] = Expr::Function(Box::new(body.clone()));
    definition[0].line = body.line;
    stats.extend(definition);
    let ret = parse(b"return __lr_f")
        .expect("generated snippet parses")
        .ret;
    Ok(print(&Block { stats, ret }))
}

/// The locals in scope at a call site of a normalized function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteScope {
    /// Parameters, then each local declared before the site, in declaration order.
    pub locals: Vec<String>,
    /// Whether the site statement is a `return`.
    pub in_return: bool,
}

/// The locals a continuation of `function` at `site` declares, in order.
pub fn site_scope(
    chunk: &Block,
    function: FunctionKey,
    site: u32,
) -> Result<SiteScope, ContinuationError> {
    let body = find_function(chunk, function)?;
    let steps = find_site(&body.body, site).ok_or(ContinuationError::NoSite(site))?;
    let scope = scope_at(body, &steps);
    Ok(SiteScope {
        locals: scope.names,
        in_return: scope.in_return,
    })
}
