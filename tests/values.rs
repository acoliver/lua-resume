//! Values the host names: full userdata, light userdata by handle, and several names for one
//! value. Each is saved as a root table, serialized as JSON and restored in a fresh state.

use lua_resume::capture::{
    CaptureError, Debug, PermanentError, Permanents, Snapshot, Value as Saved, capture_all,
};
use lua_resume::restore::{RestoreError, Sources, restore_all};
use lua_resume_debug::new_lua_with_private_debug;
use mlua::{AnyUserData, LightUserData, Lua, LuaOptions, StdLib, Value};

struct Host {
    lua: Lua,
    debug: Debug,
    permanents: Permanents,
}

impl Host {
    fn new() -> Self {
        let (lua, table) =
            new_lua_with_private_debug(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let debug = Debug::new(&table).unwrap();
        let permanents = Permanents::standard(&lua).unwrap();
        Self {
            lua,
            debug,
            permanents,
        }
    }

    fn door(&mut self, name: &str) -> AnyUserData {
        let door = self.lua.create_any_userdata(7_u32).unwrap();
        self.permanents
            .insert(name, Value::UserData(door.clone()))
            .unwrap();
        door
    }

    fn light(&mut self, handle: u64, address: usize) -> LightUserData {
        let light = LightUserData(std::ptr::without_provenance_mut(address));
        self.permanents.insert_light(handle, light).unwrap();
        light
    }

    fn capture(&self, values: Vec<Value>) -> Result<Snapshot, CaptureError> {
        let table = self.lua.create_sequence_from(values).unwrap();
        let roots = [("values", Value::Table(table))];
        capture_all(&self.lua, &self.debug, &roots, &[], &self.permanents)
    }

    fn restore(&self, snapshot: &Snapshot) -> Result<Vec<Value>, RestoreError> {
        let snapshot: Snapshot =
            serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap();
        let restored = restore_all(
            &self.lua,
            &self.debug,
            &snapshot,
            &Sources::new(),
            &self.permanents,
        )?;
        let Value::Table(table) = &restored.roots[0].1 else {
            panic!("the root is not a table");
        };
        Ok(table
            .clone()
            .sequence_values()
            .map(Result::unwrap)
            .collect())
    }
}

fn first(values: &[Value]) -> &Value {
    values.first().expect("a restored value")
}

fn list(snapshot: &Snapshot) -> &[(Saved, Saved)] {
    let Saved::Table(index) = snapshot.roots[0].1 else {
        panic!("the root is not a table");
    };
    &snapshot.tables[index].entries
}

#[test]
fn a_named_full_userdata_restores_as_the_one_the_new_host_registered() {
    let mut old = Host::new();
    let door = old.door("class:Door");
    let snapshot = old.capture(vec![Value::UserData(door)]).unwrap();
    assert_eq!(
        list(&snapshot)[0].1,
        Saved::Permanent("class:Door".to_owned())
    );

    let mut fresh = Host::new();
    let new_door = fresh.door("class:Door");
    let values = fresh.restore(&snapshot).unwrap();
    let Value::UserData(restored) = first(&values) else {
        panic!("not a userdata");
    };
    assert_eq!(restored.to_pointer(), new_door.to_pointer());
}

#[test]
fn a_light_userdata_is_saved_as_its_handle_and_restored_as_the_new_hosts_pointer() {
    let mut old = Host::new();
    let light = old.light(7, 0x1000);
    let snapshot = old.capture(vec![Value::LightUserData(light)]).unwrap();
    assert_eq!(list(&snapshot)[0].1, Saved::LightUserData(7));

    let mut fresh = Host::new();
    let moved = fresh.light(7, 0x2000);
    let values = fresh.restore(&snapshot).unwrap();
    assert_eq!(first(&values), &Value::LightUserData(moved));
}

#[test]
fn a_light_userdata_without_a_handle_is_refused_and_a_missing_handle_is_reported() {
    let old = Host::new();
    let light = LightUserData(std::ptr::without_provenance_mut(0x1000));
    let error = old.capture(vec![Value::LightUserData(light)]).unwrap_err();
    assert!(matches!(error, CaptureError::Userdata { .. }), "{error}");

    let mut named = Host::new();
    let light = named.light(7, 0x1000);
    let snapshot = named.capture(vec![Value::LightUserData(light)]).unwrap();
    let fresh = Host::new();
    let error = fresh.restore(&snapshot).unwrap_err();
    assert!(
        matches!(error, RestoreError::MissingLightUserData(7)),
        "{error}"
    );
}

#[test]
fn registering_a_handle_or_a_pointer_twice_is_an_error() {
    let mut host = Host::new();
    let light = host.light(1, 0x10);
    let other = LightUserData(std::ptr::without_provenance_mut(0x20));
    assert_eq!(
        host.permanents.insert_light(1, other),
        Err(PermanentError::DuplicateHandle(1))
    );
    assert_eq!(
        host.permanents.insert_light(2, light),
        Err(PermanentError::AlreadyHandled(1))
    );
}

#[test]
fn a_value_with_two_names_is_saved_under_its_first_and_restores_from_either() {
    let mut old = Host::new();
    let door = old.door("class:Door");
    old.permanents.alias("Door", "class:Door").unwrap();
    let snapshot = old.capture(vec![Value::UserData(door)]).unwrap();
    assert_eq!(
        list(&snapshot)[0].1,
        Saved::Permanent("class:Door".to_owned())
    );

    // A save that holds the legacy label restores to the same object.
    let mut legacy = snapshot.clone();
    let Saved::Table(index) = legacy.roots[0].1 else {
        panic!("the root is not a table");
    };
    legacy.tables[index].entries[0].1 = Saved::Permanent("Door".to_owned());
    let mut fresh = Host::new();
    let new_door = fresh.door("class:Door");
    fresh.permanents.alias("Door", "class:Door").unwrap();
    for saved in [&snapshot, &legacy] {
        let values = fresh.restore(saved).unwrap();
        let Value::UserData(restored) = first(&values) else {
            panic!("not a userdata");
        };
        assert_eq!(restored.to_pointer(), new_door.to_pointer());
    }
}

#[test]
fn the_first_registered_name_wins_whatever_order_the_aliases_come_in() {
    let mut host = Host::new();
    let door = host.door("Door");
    host.permanents.alias("class:Door", "Door").unwrap();
    host.permanents.alias("Aardvark", "class:Door").unwrap();
    let snapshot = host.capture(vec![Value::UserData(door)]).unwrap();
    assert_eq!(list(&snapshot)[0].1, Saved::Permanent("Door".to_owned()));
}

#[test]
fn aliases_need_a_known_value_and_a_free_name() {
    let mut host = Host::new();
    host.door("class:Door");
    assert_eq!(
        host.permanents.alias("Door", "class:Gate"),
        Err(PermanentError::UnknownName("class:Gate".to_owned()))
    );
    assert_eq!(
        host.permanents.alias("string", "class:Door"),
        Err(PermanentError::DuplicateName("string".to_owned()))
    );
}

#[test]
fn a_standard_function_also_reachable_as_a_global_keeps_one_name_and_answers_to_both() {
    let build = || {
        let host = Host::new();
        host.lua.load("shortcut = string.format").exec().unwrap();
        let permanents = Permanents::standard(&host.lua).unwrap();
        let format: Value = host.lua.load("return string.format").eval().unwrap();
        let name = permanents.name_of(&format).map(str::to_owned);
        let both = ["shortcut", "string.format"].map(|name| permanents.get(name).cloned());
        (name, both)
    };
    let (name, both) = build();
    assert_eq!(name.as_deref(), Some("shortcut"));
    assert!(both.iter().all(Option::is_some));
    assert_eq!(build().0, name);
}
