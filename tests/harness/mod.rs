//! Differential harness: runs a normalized function uninterrupted, then resumes it from a
//! continuation at every yield in a fresh Lua state and compares what the rest of the run
//! yields, returns, raises and leaves in globals.

use std::collections::HashMap;

use lua_resume::continuation::{FunctionKey, Pending, continuation_source_with};
use lua_resume::syntax::ast::{Block, Stat, StatKind};
use lua_resume::syntax::{parse, print};
use mlua::{Function, Lua, Table};

use crate::support::normalize_source;

const HELPERS: &str = include_str!("helpers.lua");

pub struct Case {
    pub source: &'static str,
    pub function: &'static str,
    pub key: FunctionKey,
    /// A Lua table literal with the arguments of the first resume.
    pub args: &'static str,
    /// Lua table literals, one per later resume.
    pub inputs: &'static [&'static str],
    pub globals: &'static [&'static str],
}

struct Snap {
    site: u32,
    locals: String,
    varargs: String,
    globals: String,
}

struct Run {
    log: Vec<String>,
    snaps: Vec<Option<Snap>>,
    globals: String,
}

fn marker_site(stat: &Stat) -> Option<u32> {
    let StatKind::Assign { targets, values } = &stat.kind else {
        return None;
    };
    let text = print(&Block {
        stats: vec![Stat {
            line: 1,
            kind: StatKind::Assign {
                targets: targets.clone(),
                values: values.clone(),
            },
        }],
        ret: None,
    });
    text.trim().strip_prefix("__lrs = ")?.parse().ok()
}

/// Adds a `__snap` call after every site marker of the function, listing the locals in scope.
fn instrument(block: &mut Block, scope: &mut Vec<String>, vararg: bool, sites: &mut Sites) {
    let depth = scope.len();
    let mut stats = Vec::new();
    for mut stat in std::mem::take(&mut block.stats) {
        if let Some(site) = marker_site(&stat) {
            let packed = format!("table.pack({})", scope.join(", "));
            let varargs = if vararg { "table.pack(...)" } else { "{}" };
            stats.push(stat.clone());
            stats.extend(
                parse(format!("__snap({site}, {packed}, {varargs})").as_bytes())
                    .unwrap()
                    .stats,
            );
            sites.insert(site, scope.clone());
            continue;
        }
        instrument_inner(&mut stat, scope, vararg, sites);
        match &stat.kind {
            StatKind::Local { names, .. } => {
                scope.extend(names.iter().map(|local| local.name.clone()));
            }
            StatKind::LocalFunction { name, .. } => scope.push(name.clone()),
            _ => {}
        }
        stats.push(stat);
    }
    block.stats = stats;
    scope.truncate(depth);
}

type Sites = HashMap<u32, Vec<String>>;

fn instrument_inner(stat: &mut Stat, scope: &mut Vec<String>, vararg: bool, sites: &mut Sites) {
    match &mut stat.kind {
        StatKind::Do(body) | StatKind::While { body, .. } | StatKind::Repeat { body, .. } => {
            instrument(body, scope, vararg, sites);
        }
        StatKind::NumericFor { var, body, .. } => {
            scope.push(var.clone());
            instrument(body, scope, vararg, sites);
            scope.pop();
        }
        StatKind::If { arms, otherwise } => {
            for arm in arms {
                instrument(&mut arm.body, scope, vararg, sites);
            }
            if let Some(block) = otherwise {
                instrument(block, scope, vararg, sites);
            }
        }
        _ => {}
    }
}

fn instrumented(tree: &Block, key: FunctionKey) -> (Block, Sites) {
    let mut tree = tree.clone();
    let mut sites = Sites::new();
    for stat in &mut tree.stats {
        if let StatKind::Function { body, .. } = &mut stat.kind
            && body.line == key.line
            && body.end_line == key.end_line
        {
            let mut scope = body.params.clone();
            instrument(&mut body.body, &mut scope, body.vararg, &mut sites);
        }
    }
    (tree, sites)
}

fn setup(lua: &Lua, case: &Case, tree: &Block) {
    lua.load(HELPERS).exec().unwrap();
    let names: Vec<String> = case
        .globals
        .iter()
        .map(|name| format!("{name:?}"))
        .collect();
    lua.load(format!("__globals = {{{}}}", names.join(", ")))
        .exec()
        .unwrap();
    lua.load(print(tree)).exec().unwrap();
}

fn drive(lua: &Lua, thread: &mlua::Thread, args: &str, inputs: &[&str]) -> Run {
    let args: Table = lua.load(format!("return {args}")).eval().unwrap();
    let inputs: Table = lua
        .load(format!("return {{{}}}", inputs.join(", ")))
        .eval()
        .unwrap();
    let drive: Function = lua.globals().get("__drive").unwrap();
    let (log, snaps): (Table, Table) = drive.call((thread.clone(), args, inputs)).unwrap();
    let globals: Function = lua.globals().get("__globals_text").unwrap();
    Run {
        log: log.sequence_values().map(Result::unwrap).collect(),
        snaps: snaps
            .sequence_values::<mlua::Value>()
            .map(|value| {
                let Ok(mlua::Value::Table(snap)) = value else {
                    return None;
                };
                Some(Snap {
                    site: snap.get(1).unwrap(),
                    locals: snap.get(2).unwrap(),
                    varargs: snap.get(3).unwrap(),
                    globals: snap.get(4).unwrap(),
                })
            })
            .collect(),
        globals: globals.call(()).unwrap(),
    }
}

fn original(case: &Case, tree: &Block) -> Run {
    let lua = Lua::new();
    setup(&lua, case, tree);
    let function: Function = lua.globals().get(case.function).unwrap();
    let thread = lua.create_thread(function).unwrap();
    drive(&lua, &thread, case.args, case.inputs)
}

fn resumed(case: &Case, tree: &Block, snap: &Snap, locals: &[String], rest: &[&str]) -> Run {
    let lua = Lua::new();
    setup(&lua, case, tree);
    let source = continuation_source_with(
        tree,
        case.key,
        (snap.site, Pending::Innermost),
        locals,
        &[],
        &[],
    )
    .unwrap();
    let continuation: Function = lua.load(&source).eval().unwrap();
    let load: Function = lua.globals().get("__load").unwrap();
    let restore: Function = lua.globals().get("__restore").unwrap();
    restore.call::<()>(snap.globals.as_str()).unwrap();
    let values: Table = load.call(snap.locals.as_str()).unwrap();
    let varargs: Table = load.call(snap.varargs.as_str()).unwrap();
    let start: Function = lua.globals().get("__start").unwrap();
    let thread: mlua::Thread = start.call((continuation, values, varargs)).unwrap();
    drive(&lua, &thread, "{}", rest)
}

/// Resumes from every yield of the run and returns how many yields were checked.
pub fn check(case: &Case) -> usize {
    let tree = normalize_source(case.source.as_bytes()).unwrap();
    let expected = original(case, &tree);
    let (tree_with_snaps, sites) = instrumented(&tree, case.key);
    let traced = original(case, &tree_with_snaps);
    assert_eq!(traced.log, expected.log, "instrumentation changed the run");
    let yields = expected.log.len() - 1;
    for index in 0..yields {
        let snap = traced.snaps[index]
            .as_ref()
            .expect("a yield has a snapshot");
        let locals = &sites[&snap.site];
        let run = resumed(case, &tree, snap, locals, &case.inputs[index..]);
        assert_eq!(
            run.log[1..],
            expected.log[index + 1..],
            "after yield {index}"
        );
        assert_eq!(run.globals, expected.globals, "globals after yield {index}");
    }
    yields
}
