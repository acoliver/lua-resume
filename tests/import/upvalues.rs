//! Upvalue names decide which saved cell each upvalue of a rebuilt function takes, and a
//! restore or `check_restore` refuses a function the save no longer describes.

use crate::support::{Host, SOURCE, World, imported, int, line_of, sources, yield_fn};
use lua_resume::capture::{Snapshot, TableData, Value};
use lua_resume::restore::{RestoreError, check_restore, restore, restore_all};
use mlua::Function;

// A Lua 5.3 closure numbers its upvalues in the order its source mentions them. The 5.4
// function rebuilt from the normalized source numbers them in the order the normalized source
// mentions them, and normalization moves a global call ahead of the table it stores into, so
// restores bind upvalues by name.

/// A `stash` frame stopped in its yield, whose closure shares a table with the save.
fn stash_world() -> World {
    let mut world = World::new();
    world.heap.tables.push(TableData::default());
    let seed = world.cell(Value::Table(0));
    let count = world.cell(int(1));
    let stash = world.closure(line_of("stash"), &[("seed", seed), ("count", count)]);
    world.frame(stash, 0, &[int(5)], yield_fn());
    world.yielded();
    world
}

fn names(snapshot: &Snapshot) -> Vec<&str> {
    snapshot.closures[0]
        .upvalues
        .iter()
        .map(|upvalue| upvalue.name.as_str())
        .collect()
}

fn restore_in(host: &Host, snapshot: &Snapshot) -> Result<mlua::Thread, RestoreError> {
    let sources = sources(host).restore_sources();
    restore(&host.lua, &host.debug, snapshot, &sources, &host.permanents)
}

fn reason(error: RestoreError) -> String {
    match error {
        RestoreError::SourceChanged { reason, .. } => reason,
        other => panic!("expected SourceChanged, got {other}"),
    }
}

#[test]
fn the_two_compiles_number_the_upvalues_differently() {
    let snapshot = imported(&stash_world());
    assert_eq!(names(&snapshot), ["seed", "_ENV", "count"]);
    let host = Host::new(SOURCE);
    let stash: Function = host.lua.globals().get("stash").unwrap();
    let rebuilt: Vec<String> = (1..=3)
        .map(|index| host.debug.upvalue(&stash, index).unwrap().0)
        .collect();
    assert_eq!(rebuilt, ["_ENV", "seed", "count"]);
}

#[test]
fn a_closure_with_reordered_upvalues_resumes_like_the_uninterrupted_run() {
    let snapshot = imported(&stash_world());
    let expected = {
        let host = Host::new(SOURCE);
        let function: Function = host.lua.globals().get("stash").unwrap();
        let thread = host.lua.create_thread(function).unwrap();
        vec![host.step(&thread, "5"), host.step(&thread, "5")]
    };
    let fresh = Host::new(SOURCE);
    let thread = restore_in(&fresh, &snapshot).unwrap();
    assert_eq!(fresh.step(&thread, "5"), expected[1]);
    assert_eq!(expected[1], "return:2,1");
}

#[test]
fn a_save_without_env_uses_the_environment_of_the_chunk() {
    let mut snapshot = imported(&stash_world());
    snapshot.closures[0].upvalues.retain(|up| up.name != "_ENV");
    let fresh = Host::new(SOURCE);
    let thread = restore_in(&fresh, &snapshot).unwrap();
    assert_eq!(fresh.step(&thread, "5"), "return:2,1");
}

#[test]
fn a_saved_upvalue_the_function_no_longer_has_is_refused_with_both_name_lists() {
    let mut snapshot = imported(&stash_world());
    snapshot.closures[0].upvalues[2].name = "total".to_owned();
    let error = restore_in(&Host::new(SOURCE), &snapshot).unwrap_err();
    assert_eq!(
        reason(error),
        "its upvalues are [_ENV, seed], the save has [seed, _ENV, total]"
    );
}

#[test]
fn equal_names_take_the_saved_cells_in_order() {
    let mut snapshot = imported(&stash_world());
    let duplicate = snapshot.closures[0].upvalues[0].clone();
    snapshot.closures[0].upvalues.push(duplicate);
    let error = restore_in(&Host::new(SOURCE), &snapshot).unwrap_err();
    assert!(reason(error).ends_with("the save has [seed, _ENV, count, seed]"));
}

#[test]
fn check_restore_lists_every_refusal_where_restore_all_stops_at_the_first() {
    let mut snapshot = imported(&stash_world());
    let fresh = Host::new(SOURCE);
    let sources = sources(&fresh).restore_sources();
    let check = |snapshot: &Snapshot| {
        check_restore(
            &fresh.lua,
            &fresh.debug,
            snapshot,
            &sources,
            &fresh.permanents,
        )
    };
    assert!(check(&snapshot).is_empty());

    snapshot.closures[0].upvalues[2].name = "total".to_owned();
    let mut lost = snapshot.closures[0].clone();
    lost.function.line = 9000;
    lost.function.end_line = 9001;
    snapshot.closures.push(lost);
    snapshot.cells[0].value = Value::Permanent("nowhere".to_owned());
    snapshot.cells[1].value = Value::Permanent("nowhere".to_owned());
    snapshot.cells[2].value = Value::LightUserData(77);

    let found = check(&snapshot);
    let expected = [
        ("upvalue cell 0", "MissingPermanent"),
        ("upvalue cell 2", "MissingLightUserData"),
        ("closure 0 (", "SourceChanged"),
        ("closure 1 (", "UnknownFunction"),
    ];
    assert_eq!(found.len(), expected.len(), "{found:?}");
    for (refusal, (place, kind)) in found.iter().zip(expected) {
        assert!(refusal.location.starts_with(place), "{refusal}");
        assert!(
            format!("{:?}", refusal.error).starts_with(kind),
            "{refusal}"
        );
    }

    let first = restore_all(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &sources,
        &fresh.permanents,
    );
    assert!(first.is_err());
}

#[test]
fn check_restore_reports_a_frame_whose_chunk_is_unknown() {
    let snapshot = imported(&stash_world());
    let fresh = Host::new(SOURCE);
    let found = check_restore(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &lua_resume::restore::Sources::new(),
        &fresh.permanents,
    );
    let places: Vec<&str> = found.iter().map(|r| r.location.as_str()).collect();
    assert!(
        places.iter().any(|p| p.starts_with("closure 0")),
        "{places:?}"
    );
    assert!(
        places.iter().any(|p| p.starts_with("thread 0 frame 0")),
        "{places:?}"
    );
}
