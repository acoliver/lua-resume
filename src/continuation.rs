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
//! Supported enclosing constructs are `do`, `if` and `while` (which includes generic `for`
//! after normalization). The call statement must be a call, `local names = call`, or
//! `names = call`. Anything else is refused with an error that names the construct.

use crate::syntax::ast::{Block, Expr, FuncBody, IfArm, Name, Stat, StatKind, UnOp};
use crate::syntax::{parse, print};

/// Why a continuation cannot be generated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationError {
    #[error("no function defined at lines {0} to {1}")]
    NoFunction(u32, u32),
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
pub fn continuation_source(
    chunk: &Block,
    function: FunctionKey,
    site: u32,
    locals: &[String],
    upvalues: &[String],
) -> Result<String, ContinuationError> {
    let body = find_function(chunk, function).ok_or(ContinuationError::NoFunction(
        function.line,
        function.end_line,
    ))?;
    let steps = find_site(&body.body, site).ok_or(ContinuationError::NoSite(site))?;
    let mut stats = Vec::new();
    if !locals.is_empty() {
        let names = locals.join(", ");
        stats.extend(snippet(&format!(
            "local {names} = __lr_unpack(__lr_v, 1, {})",
            locals.len()
        )));
    }
    stats.extend(resume_block(&body.body, &steps, 0)?);
    let block = Block {
        stats,
        ret: body.body.ret.clone(),
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

/// The statements of a snippet. The snippet is trusted generated text.
fn snippet(text: &str) -> Vec<Stat> {
    parse(text.as_bytes())
        .expect("generated snippet parses")
        .stats
}

/// A step from a block into the statement at `index`; `branch` selects an `if` arm.
#[derive(Clone, Copy)]
struct Step {
    index: usize,
    branch: usize,
}

fn find_function(block: &Block, key: FunctionKey) -> Option<&FuncBody> {
    for stat in &block.stats {
        let own = match &stat.kind {
            StatKind::Function { body, .. } | StatKind::LocalFunction { body, .. } => Some(body),
            StatKind::Local { values, .. } | StatKind::Assign { values, .. } => {
                values.iter().find_map(|value| match value {
                    Expr::Function(body) => Some(body),
                    _ => None,
                })
            }
            _ => None,
        };
        if let Some(body) = own {
            if body.line == key.line && body.end_line == key.end_line {
                return Some(body);
            }
            if let Some(found) = find_function(&body.body, key) {
                return Some(found);
            }
        }
        for inner in inner_blocks(stat) {
            if let Some(found) = find_function(inner, key) {
                return Some(found);
            }
        }
    }
    None
}

fn inner_blocks(stat: &Stat) -> Vec<&Block> {
    match &stat.kind {
        StatKind::Do(body)
        | StatKind::While { body, .. }
        | StatKind::Repeat { body, .. }
        | StatKind::NumericFor { body, .. }
        | StatKind::GenericFor { body, .. } => vec![body],
        StatKind::If { arms, otherwise } => arms
            .iter()
            .map(|arm| &arm.body)
            .chain(otherwise.iter())
            .collect(),
        _ => Vec::new(),
    }
}

fn is_marker(stat: &Stat, site: u32) -> bool {
    matches!(&stat.kind, StatKind::Assign { targets, values }
        if targets == &[Expr::Name(crate::rewrite::SITE.to_owned())]
            && values == &[Expr::Number(site.to_string())])
}

/// The path to the statement after the marker of `site`. Functions nested in the body have
/// their own markers and are not entered.
fn find_site(block: &Block, site: u32) -> Option<Vec<Step>> {
    for (index, stat) in block.stats.iter().enumerate() {
        if is_marker(stat, site) && index + 1 < block.stats.len() {
            return Some(vec![Step {
                index: index + 1,
                branch: 0,
            }]);
        }
        for (branch, inner) in inner_blocks(stat).into_iter().enumerate() {
            if let Some(mut path) = find_site(inner, site) {
                path.insert(0, Step { index, branch });
                return Some(path);
            }
        }
    }
    None
}

/// The statements that finish `block`, entered at the step `steps[depth]`.
fn resume_block(
    block: &Block,
    steps: &[Step],
    depth: usize,
) -> Result<Vec<Stat>, ContinuationError> {
    let step = steps[depth];
    if block.stats[..step.index]
        .iter()
        .any(|stat| matches!(stat.kind, StatKind::Label(_)))
    {
        return Err(ContinuationError::Unsupported(
            "a block with a label before the call",
        ));
    }
    let stat = &block.stats[step.index];
    let mut out = if depth + 1 == steps.len() {
        site_result(stat)?
    } else {
        vec![resume_container(stat, step.branch, steps, depth + 1)?]
    };
    out.extend(block.stats[step.index + 1..].iter().cloned());
    Ok(out)
}

fn sub_block(block: &Block, steps: &[Step], depth: usize) -> Result<Block, ContinuationError> {
    Ok(Block {
        stats: resume_block(block, steps, depth)?,
        ret: block.ret.clone(),
    })
}

fn resume_container(
    stat: &Stat,
    branch: usize,
    steps: &[Step],
    depth: usize,
) -> Result<Stat, ContinuationError> {
    let kind = match &stat.kind {
        StatKind::Do(body) => StatKind::Do(sub_block(body, steps, depth)?),
        StatKind::If { arms, otherwise } => {
            let body = match arms.get(branch) {
                Some(IfArm { body, .. }) => body,
                None => otherwise.as_ref().expect("the step names an arm"),
            };
            StatKind::Do(sub_block(body, steps, depth)?)
        }
        StatKind::While { cond, body } => resume_while(cond, body, steps, depth)?,
        StatKind::Repeat { .. } => return Err(ContinuationError::Unsupported("a repeat loop")),
        StatKind::NumericFor { .. } => {
            return Err(ContinuationError::Unsupported("a numeric for loop"));
        }
        _ => return Err(ContinuationError::Unsupported("a generic for loop")),
    };
    Ok(Stat {
        line: stat.line,
        kind,
    })
}

/// `do local first = true while true do if first then first = false <rest>
/// else if not cond then break end <body> end end end`.
fn resume_while(
    cond: &Expr,
    body: &Block,
    steps: &[Step],
    depth: usize,
) -> Result<StatKind, ContinuationError> {
    let rest = sub_block(body, steps, depth)?;
    let mut template = snippet(&format!(
        "do local __lr_first{depth} = true while true do if __lr_first{depth} then \
         __lr_first{depth} = false else if not __lr_cond then break end end end end"
    ));
    let StatKind::Do(outer) = &mut template[0].kind else {
        unreachable!("the template is a do block")
    };
    let StatKind::While { body: looped, .. } = &mut outer.stats[1].kind else {
        unreachable!("the template holds a while loop")
    };
    let StatKind::If { arms, otherwise } = &mut looped.stats[0].kind else {
        unreachable!("the template holds an if")
    };
    arms[0].body.stats.extend(rest.stats);
    arms[0].body.ret = rest.ret;
    let guard = otherwise.as_mut().expect("the template has an else");
    let StatKind::If { arms: check, .. } = &mut guard.stats[0].kind else {
        unreachable!("the else holds the loop test")
    };
    check[0].cond = Expr::Unary(UnOp::Not, Box::new(Expr::Paren(Box::new(cond.clone()))));
    guard.stats.extend(body.stats.iter().cloned());
    guard.ret.clone_from(&body.ret);
    Ok(template.remove(0).kind)
}

fn is_call(values: &[Expr]) -> bool {
    matches!(values, [Expr::Call(_)])
}

/// The statements that replace the call statement, taking its results from `__lr_next`.
fn site_result(stat: &Stat) -> Result<Vec<Stat>, ContinuationError> {
    let next = |names: &str, declare: bool| {
        let local = if declare { "local " } else { "" };
        snippet(&format!("{local}{names} = __lr_next()"))
    };
    match &stat.kind {
        StatKind::Call(_) => Ok(snippet("__lr_next()")),
        StatKind::Local { names, values } if is_call(values) => {
            let list: Vec<&Name> = names.iter().map(|name| &name.name).collect();
            let text = list
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(next(&text, true))
        }
        StatKind::Assign { targets, values } if is_call(values) => {
            let all_names = targets.iter().all(|target| matches!(target, Expr::Name(_)));
            if !all_names {
                return Err(ContinuationError::Unsupported("an assignment to a field"));
            }
            let text = targets
                .iter()
                .filter_map(|target| match target {
                    Expr::Name(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ");
            Ok(next(&text, false))
        }
        _ => Err(ContinuationError::Unsupported(
            "a statement that is not a plain call",
        )),
    }
}
