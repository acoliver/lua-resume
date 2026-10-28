//! Original compiler decisions are matched to individual source occurrences.

mod support;

use lua_resume::{
    compiled::Compiled,
    rewrite::normalize,
    syntax::{parse, print},
};
use mlua::Lua;

fn constants(count: usize) -> String {
    (0..count)
        .map(|i| format!("'constant{i}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn equivalent(source: &str) -> String {
    let tree = support::normalize_source(source.as_bytes()).unwrap();
    lua_resume::rewrite::check_normal_form(&tree).unwrap();
    let original: String = Lua::new().load(source).eval().unwrap();
    let rewritten = print(&tree);
    let actual: String = Lua::new().load(&rewritten).eval().unwrap();
    assert_eq!(actual, original, "{source}\n{rewritten}");
    actual
}

#[test]
fn repeated_key_occurrences_can_cross_the_operand_limit() {
    let source = format!(
        "local old={{}}; local function f() _ENV={{}}; return 1 end; _ENV=old; _ENV.x=f(); local before=old.x and 'old' or 'new'; local function g() return 'padding','x' end; local constants={{{}}}; _ENV=old; _ENV.x=f(); return before..':'..(old.x and 'old' or 'new')",
        constants(300)
    );
    assert_eq!(equivalent(&source), "new:old");
}

#[test]
fn string_operand_boundary_keeps_each_tables_read_point() {
    for count in [255, 256] {
        for table in ["_ENV", "t"] {
            for key in [".x", "['x']", "[1]", "[1.5]", "[('x')]", "[key]"] {
                let setup = if table == "t" { "local t" } else { "" };
                let source = format!(
                    "{setup}; local old={{}}; local function f() {table}={{}}; return 1 end; {table}=old; local function run() local constants={{{}}}; local key <const> = 'x'; {table}{key}=f() end; run(); return (old.x or old[1] or old[1.5]) and 'old' or 'new'",
                    constants(count)
                );
                let expected = if count == 256 || key == "[1]" || key == "[1.5]" {
                    "old"
                } else {
                    "new"
                };
                assert_eq!(equivalent(&source), expected);
            }
        }
    }
}

#[test]
fn nested_functions_use_their_own_constants_and_multiple_target_order() {
    for count in [255, 256] {
        let source = format!(
            "local t={{}}; local old=t; local function f() t={{}}; return 1,2 end; local function run() local constants={{{}}}; t.x,t[1]=f() end; run(); return (old.x and 'x' or '-')..(old[1] and 'i' or '-')",
            constants(count)
        );
        equivalent(&source);
    }
}

#[test]
fn mismatched_store_counts_fail_explicitly_in_each_function() {
    for (source, other) in [
        ("_ENV.x=1", "return 1"),
        ("return 1", "_ENV.x=1"),
        (
            "local function f() _ENV.x=1 end",
            "local function f() return 1 end",
        ),
    ] {
        let dump = Lua::new().load(other).into_function().unwrap().dump(true);
        let error = normalize(
            &parse(source.as_bytes()).unwrap(),
            &Compiled::from_dump(&dump).unwrap(),
        )
        .unwrap_err();
        assert!(error.message.contains("store occurrences"), "{error}");
    }
}

#[test]
fn mismatched_prototype_lines_fail_explicitly() {
    let source = "\nlocal function f() end";
    let dump = Lua::new()
        .load("local function f() end")
        .into_function()
        .unwrap()
        .dump(true);
    let error = normalize(
        &parse(source.as_bytes()).unwrap(),
        &Compiled::from_dump(&dump).unwrap(),
    )
    .unwrap_err();
    assert!(error.message.contains("linedefined"), "{error}");
}

#[test]
fn implicit_global_targets_use_original_main_function_constants() {
    for count in [255, 256] {
        let source = format!(
            "local constants={{{}}}; local old={{}}; local function f() _ENV={{}}; return 1 end; _ENV=old; x=f(); return old.x and 'old' or 'new'",
            constants(count)
        );
        assert_eq!(
            equivalent(&source),
            if count == 255 { "new" } else { "old" }
        );
    }
}

#[test]
fn same_line_sibling_prototypes_have_independent_operand_decisions() {
    let source = format!(
        "local t={{}}; local old=t; local function f() t={{}}; return 1 end; local function small() local constants={{{}}}; t.x=f() end; local function big() local constants={{{}}}; t.x=f() end; small(); local before=old.x and 'old' or 'new'; t=old; big(); return before..':'..(old.x and 'old' or 'new')",
        constants(255),
        constants(256)
    );
    assert_eq!(equivalent(&source), "new:old");
}

#[test]
fn functions_in_numeric_loop_headers_keep_their_occurrence_identity() {
    let source = format!(
        "local t={{}}; local old=t; local function f() t={{}}; return 1 end; for i=(function() local constants={{{}}}; t.x=f(); return 1 end)(),1 do end; return old.x and 'old' or 'new'",
        constants(256)
    );
    assert_eq!(equivalent(&source), "old");
}

#[test]
fn missing_and_extra_nested_prototypes_fail_explicitly() {
    for (source, other) in [
        ("local function f() end", "return 1"),
        ("return 1", "local function f() end"),
    ] {
        let dump = Lua::new().load(other).into_function().unwrap().dump(true);
        let error = normalize(
            &parse(source.as_bytes()).unwrap(),
            &Compiled::from_dump(&dump).unwrap(),
        )
        .unwrap_err();
        assert!(error.message.contains("function"), "{error}");
    }
}
