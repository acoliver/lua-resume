//! Several threads and a heap import into one snapshot that a restore rebuilds once: shared
//! tables, roots, closures that share an upvalue with a frame, permanent metatables and the
//! cost of importing thousands of threads.

use crate::support::{Host, World, host, int, line_of, list, sources, string, yield_fn};
use lua_resume::capture::{Snapshot, TableData, Value};
use lua_resume::import::{ImportError, PermanentMap, import_threads};
use lua_resume::restore::restore_all;
use mlua::Function;

/// Two `listloop` threads stopped in the second iteration with the same `out` table, and a
/// `share` thread whose `count` is an open upvalue of the closure that the table `hooks`
/// also holds. `out` is the root `out` and `hooks` the root `hooks`.
fn shared_world() -> (World, Vec<lua_resume::import::SavedThread>) {
    let mut world = World::new();
    world.heap.tables.push(list(&[10, 20, 30]));
    world.heap.tables.push(list(&[111]));
    let listloop = world.closure(line_of("listloop"), &[]);
    let aux = Value::Permanent("__ipairs_aux__".to_owned());
    let registers = [
        Value::Table(0),
        Value::Table(1),
        aux,
        Value::Table(0),
        int(2),
        int(2),
        int(20),
    ];
    let mut threads = Vec::new();
    for _ in 0..2 {
        world.frame(listloop, 1, &registers, yield_fn());
        world.yielded();
        threads.push(world.next_thread());
    }
    let count_cell = world.cell(Value::Nil);
    let raise = world.closure(line_of("raise"), &[("count", count_cell)]);
    let outer = world.closure(line_of("share"), &[]);
    let registers = [int(10), int(10), Value::Closure(raise)];
    let base = world.frame(outer, 0, &registers, yield_fn());
    world.yielded();
    let mut share = world.next_thread();
    share.open_upvalues.push((base + 1, count_cell));
    threads.push(share);
    world.heap.tables.push(TableData {
        entries: vec![(string("raise"), Value::Closure(raise))],
        metatable: None,
    });
    world.heap.roots = vec![
        ("out".to_owned(), Value::Table(1)),
        ("hooks".to_owned(), Value::Table(2)),
    ];
    (world, threads)
}

fn import_shared(
    fresh: &Host,
    world: &World,
    threads: &[lua_resume::import::SavedThread],
) -> Snapshot {
    import_threads(
        threads,
        &world.heap,
        &sources(fresh),
        &fresh.permanents,
        &PermanentMap::default(),
    )
    .unwrap()
}

#[test]
fn several_threads_import_into_one_snapshot_in_order() {
    let (world, threads) = shared_world();
    let snapshot = import_shared(&host(), &world, &threads);
    assert_eq!(snapshot.captured, [0, 1, 2]);
    assert_eq!(snapshot.threads.len(), 3);
    assert_eq!(snapshot.tables.len(), 3);
    assert_eq!(snapshot.cells.len(), 2);
    let names: Vec<&str> = snapshot
        .roots
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["out", "hooks"]);
    let outs: Vec<_> = (0..2)
        .map(|thread| match &snapshot.threads[thread] {
            lua_resume::capture::ThreadData::Suspended { frames } => frames[0]
                .locals
                .iter()
                .find(|local| local.name == "out")
                .unwrap()
                .value
                .clone(),
            other => panic!("thread {thread} is {other:?}"),
        })
        .collect();
    assert_eq!(outs, [Value::Table(1), Value::Table(1)]);
}

#[test]
fn restored_threads_roots_and_closures_share_state() {
    let (world, threads) = shared_world();
    let fresh = host();
    let snapshot = import_shared(&fresh, &world, &threads);
    let restored = restore_all(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &sources(&fresh).restore_sources(),
        &fresh.permanents,
    )
    .unwrap();
    let [first, second, share] = restored.threads.as_slice() else {
        panic!("expected three threads");
    };
    let root = |name: &str| match &restored.roots.iter().find(|(n, _)| n == name).unwrap().1 {
        mlua::Value::Table(table) => table.clone(),
        other => panic!("root {name} is {other:?}"),
    };
    let out = root("out");
    // The first thread's append reaches the root.
    assert_eq!(fresh.step(first, "222"), "yield:\"item\",3,30");
    assert_eq!(out.raw_len(), 2);
    // A write through the root and the first thread's append are both seen by the second.
    out.raw_set(3, 555).unwrap();
    assert_eq!(fresh.step(second, "999"), "yield:\"item\",3,30");
    assert_eq!(fresh.step(second, "333"), "return:5,111,222,555");
    assert_eq!(out.raw_len(), 5);
    // The closure in `hooks` and the frame of the third thread share `count`.
    let raise: Function = root("hooks").get("raise").unwrap();
    raise.call::<()>(()).unwrap();
    assert_eq!(fresh.step(share, "5"), "return:12,23,5");
}

#[test]
fn a_thread_that_cannot_be_imported_is_reported_with_its_index() {
    let (world, mut threads) = shared_world();
    threads[1].frames[0].call_status = 0;
    let error = import_threads(
        &threads,
        &world.heap,
        &sources(&host()),
        &host().permanents,
        &PermanentMap::default(),
    )
    .unwrap_err();
    let ImportError::Thread { thread: 1, source } = &error else {
        panic!("{error:?}");
    };
    assert!(
        source.to_string().starts_with("frame 0: a native frame"),
        "{error}"
    );
    assert!(
        error.to_string().starts_with("thread 1: frame 0"),
        "{error}"
    );
}

#[test]
fn two_roots_with_one_name_are_refused() {
    let (mut world, threads) = shared_world();
    world.heap.roots.push(("out".to_owned(), Value::Nil));
    let error = import_threads(
        &threads,
        &world.heap,
        &sources(&host()),
        &host().permanents,
        &PermanentMap::default(),
    )
    .unwrap_err();
    assert!(matches!(error, ImportError::Malformed { .. }), "{error:?}");
}

#[test]
fn a_permanent_metatable_in_the_heap_is_renamed_and_checked() {
    let (mut world, threads) = shared_world();
    world.heap.tables[1].metatable = Some(lua_resume::capture::Metatable::Permanent(
        "class:Old".to_owned(),
    ));
    let mut names = PermanentMap::default();
    names.rename("class:Old", "string");
    let fresh = host();
    let snapshot = import_threads(
        &threads,
        &world.heap,
        &sources(&fresh),
        &fresh.permanents,
        &names,
    )
    .unwrap();
    assert_eq!(
        snapshot.tables[1].metatable,
        Some(lua_resume::capture::Metatable::Permanent(
            "string".to_owned()
        ))
    );
    let error = import_threads(
        &threads,
        &world.heap,
        &sources(&fresh),
        &fresh.permanents,
        &PermanentMap::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, ImportError::MissingPermanent { ref name, .. } if name == "class:Old"),
        "{error:?}"
    );
}

/// A heap of 20,000 tables, each linked to the next, with 2,000 `tailer` threads that each
/// hold one of them.
#[test]
fn importing_two_thousand_threads_over_twenty_thousand_tables_takes_one_pass() {
    let mut world = World::new();
    let tailer = world.closure(line_of("tailer"), &[]);
    world.heap.tables = (0..20_000)
        .map(|index| TableData {
            entries: vec![
                (string("id"), int(index)),
                (
                    string("next"),
                    Value::Table(usize::try_from((index + 1) % 20_000).unwrap()),
                ),
            ],
            metatable: None,
        })
        .collect();
    let base = world.frame(tailer, 0, &[Value::Nil], yield_fn());
    world.yielded();
    let template = world.next_thread();
    let threads: Vec<_> = (0..2_000)
        .map(|index| {
            let mut thread = template.clone();
            thread.stack[base] = Value::Table(index * 10);
            thread
        })
        .collect();
    let fresh = host();
    let started = std::time::Instant::now();
    let snapshot = import_threads(
        &threads,
        &world.heap,
        &sources(&fresh),
        &fresh.permanents,
        &PermanentMap::default(),
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(snapshot.threads.len(), 2_000);
    assert_eq!(snapshot.tables.len(), 20_000);
    assert_eq!(snapshot.captured.len(), 2_000);
    // The heap is cloned and checked once, whatever the number of threads. The bound is far
    // above the time of a debug build so that a slow runner does not fail the test.
    assert!(elapsed.as_secs() < 10, "the import took {elapsed:?}");
}
