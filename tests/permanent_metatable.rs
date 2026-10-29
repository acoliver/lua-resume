//! A table whose metatable is a permanent class table is saved by the class's name and
//! restored with the host's own class table.

use lua_resume::capture::{
    Debug, Metatable, Permanents, Snapshot, TableData, Value as Saved, capture_all,
};
use lua_resume::restore::{Sources, restore_all};
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{Lua, LuaOptions, StdLib, Table, Value};

struct Host {
    lua: Lua,
    debug: Debug,
    permanents: Permanents,
}

fn host() -> Host {
    let (lua, table) = new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
    let debug = Debug::new(&table).unwrap();
    let permanents = Permanents::standard(&lua).unwrap();
    Host {
        lua,
        debug,
        permanents,
    }
}

const SOURCE: &str = "Account = {}
Account.__index = Account
function Account.describe(self) return 'account ' .. self.name end
function make()
  local plain = {}
  setmetatable(plain, {__index = {kind = 'local'}})
  return {
    owner = setmetatable({name = 'ada'}, Account),
    other = setmetatable({name = 'bob'}, Account),
    plain = plain,
  }
end
";

/// A host that ran `SOURCE` and registered its `Account` table as `class:Account`.
fn class_host() -> Host {
    let mut host = host();
    host.lua.load(SOURCE).exec().unwrap();
    let class: Table = host.lua.globals().get("Account").unwrap();
    host.permanents
        .insert("class:Account", Value::Table(class))
        .unwrap();
    host
}

fn captured(host: &Host) -> Snapshot {
    let make: mlua::Function = host.lua.globals().get("make").unwrap();
    let world: Table = make.call(()).unwrap();
    let roots = [("world", Value::Table(world))];
    capture_all(&host.lua, &host.debug, &roots, &[], &host.permanents).unwrap()
}

fn table_of(snapshot: &Snapshot, root: &Saved, key: &str) -> TableData {
    let Saved::Table(world) = root else {
        panic!("the root is not a table");
    };
    let (_, Saved::Table(found)) = snapshot.tables[*world]
        .entries
        .iter()
        .find(|(name, _)| matches!(name, Saved::String(bytes) if bytes == key.as_bytes()))
        .unwrap_or_else(|| panic!("no field {key}"))
    else {
        panic!("{key} is not a table");
    };
    snapshot.tables[*found].clone()
}

#[test]
fn a_permanent_metatable_is_saved_by_name_and_a_plain_one_by_index() {
    let snapshot = captured(&class_host());
    let root = &snapshot.roots[0].1;
    assert_eq!(
        table_of(&snapshot, root, "owner").metatable,
        Some(Metatable::Permanent("class:Account".to_owned()))
    );
    assert!(matches!(
        table_of(&snapshot, root, "plain").metatable,
        Some(Metatable::Table(_))
    ));
    // The class table is never entered: its methods are not in the snapshot.
    assert_eq!(snapshot.closures.len(), 0);
}

#[test]
fn a_restored_table_has_the_hosts_permanent_as_its_metatable() {
    let snapshot = captured(&class_host());
    let json = serde_json::to_string(&snapshot).unwrap();
    let snapshot: Snapshot = serde_json::from_str(&json).unwrap();
    let fresh = class_host();
    let restored = restore_all(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &Sources::new(),
        &fresh.permanents,
    )
    .unwrap();
    let Value::Table(world) = &restored.roots[0].1 else {
        panic!("the root is not a table");
    };
    let class: Table = fresh.lua.globals().get("Account").unwrap();
    let owner: Table = world.get("owner").unwrap();
    let other: Table = world.get("other").unwrap();
    for table in [&owner, &other] {
        let meta = table.metatable().unwrap();
        assert!(meta.to_pointer() == class.to_pointer());
    }
    let describe: mlua::Function = owner.get("describe").unwrap();
    assert_eq!(
        describe.call::<String>(owner.clone()).unwrap(),
        "account ada"
    );
}

#[test]
fn a_missing_class_permanent_is_refused_by_name() {
    let snapshot = captured(&class_host());
    let fresh = host();
    let error = restore_all(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &Sources::new(),
        &fresh.permanents,
    )
    .err()
    .expect("the class is not registered");
    assert!(error.to_string().contains("class:Account"), "{error}");
}

#[test]
fn a_permanent_that_is_not_a_table_cannot_be_a_metatable() {
    let snapshot = Snapshot {
        tables: vec![TableData {
            entries: Vec::new(),
            metatable: Some(Metatable::Permanent("print".to_owned())),
        }],
        roots: vec![("t".to_owned(), Saved::Table(0))],
        ..Snapshot::default()
    };
    let fresh = host();
    let error = restore_all(
        &fresh.lua,
        &fresh.debug,
        &snapshot,
        &Sources::new(),
        &fresh.permanents,
    )
    .err()
    .expect("a function is not a metatable");
    assert!(error.to_string().contains("print"), "{error}");
}

#[test]
fn a_snapshot_written_before_permanent_metatables_still_loads() {
    let old = r#"{"threads":[],"tables":[
        {"entries":[],"metatable":null},
        {"entries":[[{"String":[110]},{"Integer":1}]],"metatable":0}],
        "closures":[],"cells":[]}"#;
    let snapshot: Snapshot = serde_json::from_str(old).unwrap();
    assert_eq!(snapshot.tables[0].metatable, None);
    assert_eq!(snapshot.tables[1].metatable, Some(Metatable::Table(0)));
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(json.contains(r#""metatable":0"#), "{json}");
    assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), snapshot);
}

#[test]
fn a_permanent_metatable_survives_json_as_a_string() {
    let table = TableData {
        entries: Vec::new(),
        metatable: Some(Metatable::Permanent("class:Account".to_owned())),
    };
    let json = serde_json::to_string(&table).unwrap();
    assert!(json.contains(r#""metatable":"class:Account""#), "{json}");
    assert_eq!(serde_json::from_str::<TableData>(&json).unwrap(), table);
}
