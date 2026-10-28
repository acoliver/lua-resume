//! The load-time reader accepts public Lua dumps and rejects damaged or foreign inputs.

use lua_resume::{compiled::Compiled, rewrite::normalize, syntax::parse};
use mlua::Lua;

#[test]
fn stripped_and_debug_dumps_produce_the_same_analysis() {
    let lua = Lua::new();
    let function = lua
        .load("local t={x=1}; return function() t.x=2 end")
        .into_function()
        .unwrap();
    let stripped = Compiled::from_dump(&function.dump(true)).unwrap();
    let debug = Compiled::from_dump(&function.dump(false)).unwrap();
    assert_eq!(stripped, debug);
    normalize(
        &parse(b"local t={x=1}; return function() t.x=2 end").unwrap(),
        &stripped,
    )
    .unwrap();
}

#[test]
fn every_truncated_prefix_is_rejected() {
    let dump = Lua::new()
        .load("local t={x='s', [1]=3.5}; return function() t.x=true end")
        .into_function()
        .unwrap()
        .dump(false);
    for end in 0..dump.len() {
        let error = Compiled::from_dump(&dump[..end]).unwrap_err();
        assert!(error.offset <= end, "{end}: {error}");
    }
    Compiled::from_dump(&dump).unwrap();
}

#[test]
fn foreign_headers_and_trailing_bytes_are_rejected() {
    let dump = Lua::new()
        .load("return 1")
        .into_function()
        .unwrap()
        .dump(true);
    for offset in [0, 4, 5, 6, 12, 13, 14, 15, 23] {
        let mut damaged = dump.clone();
        damaged[offset] ^= 0xff;
        assert!(Compiled::from_dump(&damaged).is_err(), "offset {offset}");
    }
    let mut trailing = dump;
    trailing.push(0);
    assert!(
        Compiled::from_dump(&trailing)
            .unwrap_err()
            .message
            .contains("after the main function")
    );
}
