//! A coroutine that was created and never resumed is saved as the function it starts with and
//! restored as a coroutine that starts with that function. The arguments of its first resume
//! belong to the caller, so the restored coroutine runs like the original when given the same.

#[path = "support/mod.rs"]
mod normalizer;

use lua_resume::capture::{Debug, Permanents, Snapshot, ThreadData, Value as Saved, capture_all};
use lua_resume::restore::{Restored, Sources, restore_all};
use lua_resume::syntax::ast::Block;
use lua_resume::syntax::print;
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread, Value};

const CHUNK: &str = "=unstarted.lua";

struct Host {
    lua: Lua,
    debug: Debug,
    permanents: Permanents,
    tree: Block,
}

impl Host {
    fn new(source: &str) -> Self {
        let (lua, table) =
            new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let debug = Debug::new(&table).unwrap();
        let permanents = Permanents::standard(&lua).unwrap();
        let tree = normalizer::normalize_source(source.as_bytes()).unwrap();
        lua.load(print(&tree)).set_name(CHUNK).exec().unwrap();
        Self {
            lua,
            debug,
            permanents,
            tree,
        }
    }

    fn global<T: mlua::FromLua>(&self, name: &str) -> T {
        self.lua.globals().get(name).unwrap()
    }

    fn fresh(&self, entry: &str) -> Thread {
        self.lua.create_thread(self.global(entry)).unwrap()
    }

    fn capture(&self, roots: &[(&str, Value)], threads: &[Thread]) -> Snapshot {
        let snapshot =
            capture_all(&self.lua, &self.debug, roots, threads, &self.permanents).unwrap();
        through_json(&snapshot)
    }

    fn restore(&self, snapshot: &Snapshot) -> Restored {
        let mut sources = Sources::new();
        sources.insert(CHUNK, self.tree.clone());
        restore_all(&self.lua, &self.debug, snapshot, &sources, &self.permanents).unwrap()
    }
}

fn through_json(snapshot: &Snapshot) -> Snapshot {
    serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap()
}

type Step = (String, i64);

fn resume(thread: &Thread, args: (i64, i64)) -> Step {
    thread.resume::<Step>(args).unwrap()
}

const WORKER: &str = "function worker(a, b)
  local n = a + b
  local got = coroutine.yield('first', n)
  return 'done', n + got
end
";

#[test]
fn a_coroutine_with_arguments_pending_runs_like_the_original() {
    let host = Host::new(WORKER);
    let original = host.fresh("worker");
    let snapshot = host.capture(&[], std::slice::from_ref(&original));
    assert!(matches!(
        snapshot.threads[0],
        ThreadData::Unstarted {
            function: Saved::Closure(_)
        }
    ));

    let second = Host::new(WORKER);
    let restored = second.restore(&snapshot).threads.remove(0);
    assert_eq!(restored.status(), mlua::ThreadStatus::Resumable);

    assert_eq!(resume(&original, (3, 4)), ("first".to_owned(), 7));
    assert_eq!(resume(&restored, (3, 4)), ("first".to_owned(), 7));
    assert_eq!(resume(&original, (10, 0)), ("done".to_owned(), 17));
    assert_eq!(resume(&restored, (10, 0)), ("done".to_owned(), 17));
}

#[test]
fn capturing_a_restored_unstarted_coroutine_gives_the_same_snapshot() {
    let host = Host::new(WORKER);
    let first = host.capture(&[], &[host.fresh("worker")]);
    let second = Host::new(WORKER);
    let restored = second.restore(&first).threads;
    let again = second.capture(&[], &restored);
    assert_eq!(again, first);
}

const SHARED: &str = "function make()
  local count = 0
  local function step() count = count + 1 return count end
  local function runner() coroutine.yield(step()) coroutine.yield(step()) return step() end
  local function late(extra) coroutine.yield(step() + extra) return step() end
  return coroutine.create(runner), coroutine.create(late)
end
";

/// Resumes the coroutines in turn; the first resume of `late` supplies its argument.
fn drive(running: &Thread, late: &Thread) -> Vec<i64> {
    vec![
        late.resume::<i64>(10).unwrap(),
        running.resume::<i64>(()).unwrap(),
        late.resume::<i64>(()).unwrap(),
        running.resume::<i64>(()).unwrap(),
    ]
}

#[test]
fn an_unstarted_coroutine_shares_an_upvalue_cell_with_a_running_one() {
    let host = Host::new(SHARED);
    let (running, late): (Thread, Thread) = host.global::<Function>("make").call(()).unwrap();
    assert_eq!(running.resume::<i64>(()).unwrap(), 1);
    let snapshot = host.capture(&[], &[running.clone(), late.clone()]);
    assert_eq!(snapshot.captured, vec![0, 1]);
    assert!(matches!(snapshot.threads[0], ThreadData::Suspended { .. }));
    let ThreadData::Unstarted {
        function: Saved::Closure(late_closure),
    } = &snapshot.threads[1]
    else {
        panic!("the second coroutine is not unstarted");
    };
    let count_cell = snapshot.closures[*late_closure]
        .upvalues
        .iter()
        .find(|upvalue| upvalue.name == "step")
        .unwrap()
        .cell;
    let ThreadData::Suspended { frames } = &snapshot.threads[0] else {
        unreachable!();
    };
    let runner = &snapshot.closures[frames[0].closure];
    assert!(runner.upvalues.iter().any(|up| up.cell == count_cell));

    let second = Host::new(SHARED);
    let restored = second.restore(&snapshot).threads;
    assert_eq!(second.capture(&[], &restored), snapshot);
    let after_restore = drive(&restored[0], &restored[1]);
    let uninterrupted = drive(&running, &late);
    assert_eq!(after_restore, uninterrupted);
    assert_eq!(after_restore, [12, 3, 4, 5]);
}

const REACHABLE: &str = "function make()
  return {pending = coroutine.create(worker)}
end
function worker(a, b)
  coroutine.yield('first', a * b)
  return 'done', 0
end
";

#[test]
fn an_unstarted_coroutine_reachable_only_from_data_is_saved_with_it() {
    let host = Host::new(REACHABLE);
    let table: Table = host.global::<Function>("make").call(()).unwrap();
    let snapshot = host.capture(&[("data", Value::Table(table.clone()))], &[]);
    assert_eq!(snapshot.captured, Vec::<usize>::new());
    assert!(matches!(
        snapshot.threads.as_slice(),
        [ThreadData::Unstarted { .. }]
    ));

    let second = Host::new(REACHABLE);
    let restored = second.restore(&snapshot);
    let Value::Table(data) = &restored.roots[0].1 else {
        panic!("the root is not a table");
    };
    let pending: Thread = data.get("pending").unwrap();
    let original: Thread = table.get("pending").unwrap();
    assert_eq!(resume(&pending, (6, 7)), resume(&original, (6, 7)));
    assert_eq!(resume(&pending, (0, 0)), ("done".to_owned(), 0));
}

#[test]
fn a_listed_unstarted_coroutine_is_one_thread_whatever_refers_to_it() {
    let host = Host::new(REACHABLE);
    let table: Table = host.global::<Function>("make").call(()).unwrap();
    let listed: Thread = table.get("pending").unwrap();
    let other = host.fresh("worker");
    let threads = [other.clone(), listed.clone()];
    let snapshot = host.capture(&[("data", Value::Table(table))], &threads);
    assert_eq!(snapshot.captured, vec![0, 1]);
    assert_eq!(snapshot.threads.len(), 2);
    let field = &snapshot.tables[0].entries[0];
    assert_eq!(field.1, Saved::Thread(1));

    let second = Host::new(REACHABLE);
    let restored = second.restore(&snapshot);
    let Value::Table(data) = &restored.roots[0].1 else {
        panic!("the root is not a table");
    };
    let pending: Thread = data.get("pending").unwrap();
    assert_eq!(pending, restored.threads[1]);
}
