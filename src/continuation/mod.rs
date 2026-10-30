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
mod needs;
mod path;
mod resume;
mod site;

use crate::syntax::ast::{Block, Expr, Stat, StatKind};
use crate::syntax::{OperatorHooks, parse, print_with};
pub use needs::needed_upvalues;
use path::{find_function, find_site, scope_at};
pub use site::Pending;

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

/// What tells apart functions that start and end on the same lines: the parameter count
/// (`self` of a method included), the vararg flag and the upvalue count (`_ENV` included),
/// as `debug.getinfo` reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FunctionShape {
    pub params: u32,
    pub vararg: bool,
    pub upvalues: u32,
}

/// A function by its lines and, when several functions share them, by its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionRef {
    pub key: FunctionKey,
    pub shape: Option<FunctionShape>,
}

impl From<FunctionKey> for FunctionRef {
    fn from(key: FunctionKey) -> Self {
        Self { key, shape: None }
    }
}

/// Generates the chunk source of the continuation of `function` at call site `site`, for a
/// frame suspended in the outermost call of the site statement, which is what capture and
/// import produce. `locals` are the names of the saved locals and `upvalues` those of the
/// function.
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
    continuation_source_with(
        chunk,
        function,
        (site, Pending::Outermost),
        locals,
        upvalues,
        shared,
    )
}

/// [`continuation_source`] for a frame suspended in the call `pending` selects.
pub fn continuation_source_with(
    chunk: &Block,
    function: FunctionKey,
    (site, pending): (u32, Pending),
    locals: &[String],
    upvalues: &[String],
    shared: &[String],
) -> Result<String, ContinuationError> {
    continuation_source_hooked(
        chunk,
        function,
        (site, pending),
        (locals, upvalues, shared),
        &OperatorHooks::none(),
    )
}

/// [`continuation_source_with`] for a chunk loaded with `hooks`, which prints the operators
/// they name as the host functions the original frame called. `names` are the saved locals,
/// the upvalues and the locals closures share.
pub fn continuation_source_hooked(
    chunk: &Block,
    function: FunctionKey,
    (site, pending): (u32, Pending),
    names: (&[String], &[String], &[String]),
    hooks: &OperatorHooks,
) -> Result<String, ContinuationError> {
    continuation_hooked(chunk, function, (site, pending), names, hooks).map(|made| made.source)
}

/// A function the continuation defines on other lines than the game's source does. A loop
/// that is re-entered in the middle of an iteration appears twice, the rest of the iteration
/// and the whole body, and printed lines only move forward, so the second copy cannot keep
/// its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    /// The lines of the definition in the game's source.
    pub original: FunctionKey,
    /// The lines of the definition in the generated source.
    pub generated: FunctionKey,
}

/// Whether `body` is one of the functions the continuation adds to hand a shared local to the
/// host: `function() return name end` for a name in `shared`.
fn is_bind(body: &crate::syntax::ast::FuncBody, shared: &[String]) -> bool {
    body.params.is_empty()
        && body.body.stats.is_empty()
        && body.body.ret.as_ref().is_some_and(
            |ret| matches!(&ret.values[..], [Expr::Name(name)] if shared.contains(name)),
        )
}

/// A generated continuation and where its functions moved to.
#[derive(Clone, Debug)]
pub struct Continuation {
    pub source: String,
    /// The functions the continuation defines on other lines than the game's source, outer
    /// before nested.
    pub moved: Vec<Moved>,
}

/// [`continuation_source_hooked`] that also reports the functions that moved.
pub fn continuation_hooked(
    chunk: &Block,
    function: impl Into<FunctionRef>,
    (site, pending): (u32, Pending),
    (locals, upvalues, shared): (&[String], &[String], &[String]),
    hooks: &OperatorHooks,
) -> Result<Continuation, ContinuationError> {
    let body = find_function(chunk, function.into())?;
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
    let rest = resume::resume_block(&body.body, &steps, (0, &binds), pending)?;
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
    let source = format!(
        "{declared}return function(__lr_v, __lr_next, __lr_unpack, ...) {} end",
        print_with(&block, hooks)
    );
    let moved = moved_functions(&block, &source, shared);
    Ok(Continuation { source, moved })
}

/// Pairs the functions of `block` with those of the source printed from it. The printed tree
/// has the shape of the block, so the two lists line up; if they do not, nothing is reported.
fn moved_functions(block: &Block, source: &str, shared: &[String]) -> Vec<Moved> {
    let printed = parse(source.as_bytes()).expect("generated source parses");
    let generated = find::all_functions(&printed);
    let original = find::all_functions(block);
    // The first generated function is the continuation itself.
    let Some(nested) = generated
        .get(1..)
        .filter(|nested| nested.len() == original.len())
    else {
        return Vec::new();
    };
    original
        .iter()
        .zip(nested)
        .filter(|(from, _)| !is_bind(from, shared))
        .map(|(from, to)| Moved {
            original: FunctionKey {
                line: from.line,
                end_line: from.end_line,
            },
            generated: FunctionKey {
                line: to.line,
                end_line: to.end_line,
            },
        })
        .filter(|moved| moved.original != moved.generated)
        .collect()
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
    function_source_hooked(chunk, function, upvalues, &OperatorHooks::none())
}

/// [`function_source`] for a chunk loaded with `hooks`.
pub fn function_source_hooked(
    chunk: &Block,
    function: impl Into<FunctionRef>,
    upvalues: &[String],
    hooks: &OperatorHooks,
) -> Result<String, ContinuationError> {
    let body = find_function(chunk, function.into())?;
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
    Ok(print_with(&Block { stats, ret }, hooks))
}

/// Whether `chunk` defines at least one function that spans the lines of `function`.
pub fn defines_function(chunk: &Block, function: FunctionKey) -> bool {
    !matches!(find::locate(chunk, function.into()), find::Found::None)
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
    let body = find_function(chunk, function.into())?;
    let steps = find_site(&body.body, site).ok_or(ContinuationError::NoSite(site))?;
    let scope = scope_at(body, &steps);
    Ok(SiteScope {
        locals: scope.names,
        in_return: scope.in_return,
    })
}
