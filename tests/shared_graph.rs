//! Many coroutines and data roots captured as one graph: what they share before the save they
//! share after the restore, so a change through one is seen through the others.

mod shared_support;

use lua_resume::capture::{CaptureError, Snapshot, ThreadData, Value as Saved};
use lua_resume::restore::Restored;
use mlua::{Table, Value};
use shared_support::{Host, STEPS, World, through_json};

fn field(snapshot: &Snapshot, table: usize, key: &str) -> Saved {
    let found = snapshot.tables[table].entries.iter().find(
        |(name, _)| matches!(name, Saved::String(bytes) if bytes.as_slice() == key.as_bytes()),
    );
    found.unwrap_or_else(|| panic!("no field {key}")).1.clone()
}

fn local(snapshot: &Snapshot, thread: usize, name: &str) -> Saved {
    let ThreadData::Suspended { frames } = &snapshot.threads[thread] else {
        panic!("thread {thread} is not suspended");
    };
    frames
        .iter()
        .flat_map(|frame| &frame.locals)
        .find(|found| found.name == name)
        .unwrap_or_else(|| panic!("no local {name}"))
        .value
        .clone()
}

fn world_root(world: &World) -> [(&'static str, Value); 1] {
    [("world", Value::Table(world.table.clone()))]
}

/// A host whose world has run the first `k` scheduler steps, with the events they gave.
fn started(k: usize) -> (Host, World, Vec<String>) {
    let host = Host::new();
    let world = host.new_world();
    let events = Host::drive(&world.threads, 0..k);
    (host, world, events)
}

fn root_table(restored: &Restored) -> Table {
    match &restored.roots[0].1 {
        Value::Table(table) => table.clone(),
        other => panic!("root is {other:?}"),
    }
}

#[test]
fn threads_share_a_table_and_an_upvalue_cell() {
    let (host, world, _) = started(2);
    let roots = world_root(&world);
    let snapshot = host.capture(&roots, &world.threads);
    let Saved::Table(world_id) = snapshot.roots[0].1 else {
        panic!("root is not a table");
    };
    let shared = field(&snapshot, world_id, "shared");
    assert_eq!(local(&snapshot, 0, "shared"), shared);
    assert_eq!(local(&snapshot, 1, "shared"), shared);
    let (Saved::Closure(inc), Saved::Closure(get)) =
        (local(&snapshot, 0, "step"), local(&snapshot, 1, "get"))
    else {
        panic!("inc and get are closures");
    };
    assert_ne!(inc, get);
    assert_eq!(
        snapshot.closures[inc].upvalues[0].cell,
        snapshot.closures[get].upvalues[0].cell
    );
    assert_eq!(snapshot.captured, vec![0, 1]);
    assert_eq!(field(&snapshot, world_id, "a"), Saved::Thread(0));
}

/// The report after running every step, with a change made through the root at step `k`.
fn uninterrupted(k: usize) -> (Vec<String>, String) {
    let (host, world, mut events) = started(k);
    host.call("poke", &world.table);
    events.extend(Host::drive(&world.threads, k..STEPS));
    (events, host.call("report", &world.table))
}

#[test]
fn every_save_point_continues_like_the_uninterrupted_run() {
    for k in 2..=6 {
        let (expected, expected_report) = uninterrupted(k);
        let (host, world, mut events) = started(k);
        let snapshot = through_json(&host.capture(&world_root(&world), &world.threads));
        let fresh = Host::new();
        let restored = fresh.restore(&snapshot);
        let table = root_table(&restored);
        let threads = [restored.threads[0].clone(), restored.threads[1].clone()];
        fresh.call("poke", &table);
        events.extend(Host::drive(&threads, k..STEPS));
        assert_eq!(events, expected, "events after saving at step {k}");
        assert_eq!(fresh.call("report", &table), expected_report, "step {k}");
    }
}

#[test]
fn a_restore_recaptures_to_an_equal_snapshot() {
    let (host, world, _) = started(5);
    let first = through_json(&host.capture(&world_root(&world), &world.threads));
    let fresh = Host::new();
    let restored = fresh.restore(&first);
    let roots = [("world", restored.roots[0].1.clone())];
    assert_eq!(fresh.capture(&roots, &restored.threads), first);
}

#[test]
fn a_coroutine_in_a_root_table_is_the_listed_coroutine() {
    let (host, world, _) = started(3);
    let snapshot = through_json(&host.capture(&world_root(&world), &world.threads));
    let fresh = Host::new();
    let restored = fresh.restore(&snapshot);
    let table = root_table(&restored);
    let held: mlua::Thread = table.get("a").unwrap();
    assert_eq!(held.to_pointer(), restored.threads[0].to_pointer());
    let other: mlua::Thread = table.get("b").unwrap();
    assert_eq!(other.to_pointer(), restored.threads[1].to_pointer());
}

#[test]
fn a_coroutine_reached_only_through_a_root_is_captured_and_resumable() {
    let (host, world, _) = started(2);
    let snapshot = through_json(&host.capture(&world_root(&world), &[]));
    assert_eq!(snapshot.captured, Vec::<usize>::new());
    assert_eq!(snapshot.threads.len(), 2);
    let fresh = Host::new();
    let restored = fresh.restore(&snapshot);
    let held: mlua::Thread = root_table(&restored).get("a").unwrap();
    assert_eq!(held.resume::<Option<i64>>(()).unwrap(), Some(2));
    let expected = world.threads[0].resume::<Option<i64>>(()).unwrap();
    assert_eq!(expected, Some(2));
}

#[test]
fn equal_state_gives_equal_snapshots_and_roots_keep_their_order() {
    let (host, world, _) = started(4);
    let roots = [
        ("zeta", Value::Table(world.table.clone())),
        ("alpha", Value::Integer(7)),
    ];
    let first = host.capture(&roots, &world.threads);
    assert_eq!(first, host.capture(&roots, &world.threads));
    let names: Vec<&str> = first.roots.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["zeta", "alpha"]);
}

#[test]
fn a_finished_coroutine_and_a_repeated_root_name_are_refused() {
    let (host, world, _) = started(7);
    let finished = host.try_capture(&[], &world.threads);
    assert!(matches!(finished, Err(CaptureError::NotSuspended { .. })));
    let (host, world, _) = started(2);
    let twice = [("a", Value::Nil), ("a", Value::Nil)];
    let repeated = host.try_capture(&twice, &world.threads);
    assert!(matches!(repeated, Err(CaptureError::DuplicateRoot { .. })));
}
