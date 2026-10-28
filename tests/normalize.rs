//! Differential execution of original and normalized Lua on stock Lua 5.4.

mod support;

use support::normalize_source;

use lua_resume::{rewrite::check_normal_form, syntax::print};
use mlua::Lua;

fn normalized(source: &str) -> String {
    let block = normalize_source(source.as_bytes()).unwrap();
    check_normal_form(&block).unwrap();
    print(&block)
}

fn execute(source: &str) -> Result<String, String> {
    Lua::new()
        .load(source)
        .set_name("=fixture")
        .into_function()
        .expect("fixture compiles")
        .call::<String>(())
        .map_err(|error| error.to_string().lines().next().unwrap().to_owned())
}

fn equivalent(source: &str) {
    let printed = normalized(source);
    assert_eq!(
        execute(source),
        execute(&printed),
        "{source}\n--- normalized ---\n{printed}"
    );
}

#[test]
fn expression_order_and_multiple_results() {
    for expression in [
        "a + bump()",
        "(a) + bump()",
        "a .. bump()",
        "t.x + bump()",
        "t[bump()]",
        "f(a, bump())",
        "f(bump(), bump())",
        "f(a, bump() + 1)",
        "({a, bump()})[1]",
        "(function() return a, bump() end)()",
    ] {
        let expression = if expression.starts_with('{') {
            format!("({expression})")
        } else {
            expression.to_owned()
        };
        let source = format!(
            "local a = 1; local t = {{x=3}}; local trace = ''; local function bump() trace = trace .. 'b'; a = 10; return 1, 2 end; local function f(...) return table.concat({{...}}, ',') end; local r = {expression}; return tostring(r) .. ':' .. trace"
        );
        equivalent(&source);
    }
}

#[test]
fn branches_loops_and_gotos() {
    for source in [
        "local i = 0; local function c() i=i+1; return i<3 end; while c() do end; return tostring(i)",
        "local i = 0; repeat local x = i; i=i+1 until (function() return x==2 end)(); return tostring(i)",
        "local function c() return true end; if false then return 'a' elseif c() then return 'b' else return 'c' end",
        "local s = ''; for k,v in ipairs({2,3}) do s=s..k..v end; return s",
        "local i = 0; ::again:: i=i+1; local x = tostring(i); if i<3 then goto again end; return x",
    ] {
        equivalent(source);
    }
}

#[test]
fn reserved_variable_names_are_rejected() {
    for source in [
        "local __lr1 = 0",
        "function f(__lrs) end",
        "return __lr_global",
        "for __lr1 = 1,2 do end",
    ] {
        assert!(normalize_source(source.as_bytes()).is_err(), "{source}");
    }
    normalize_source(b"return {__lr_field = 1}").unwrap();
}

#[test]
fn errors_keep_explicit_messages_and_lines() {
    equivalent(
        "local function fail()\n error('stop')\nend\nlocal x = 1 + fail()\nreturn tostring(x)",
    );
}

#[test]
fn syntax_fixture_normalizes_and_compiles() {
    let source = include_str!("fixtures/syntax.lua");
    Lua::new().load(normalized(source)).into_function().unwrap();
}

#[test]
#[ignore = "needs a local corpus in LUA_RESUME_CORPUS"]
fn a_local_corpus_normalizes_and_compiles() {
    let root = std::env::var_os("LUA_RESUME_CORPUS").expect("LUA_RESUME_CORPUS");
    let mut paths = vec![std::path::PathBuf::from(root)];
    let mut count = 0;
    let mut failures = Vec::new();
    let lua = Lua::new();
    while let Some(path) = paths.pop() {
        if path.is_dir() {
            paths.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        } else if path.extension().is_some_and(|ext| ext == "lua") {
            count += 1;
            let source = std::fs::read(&path).unwrap();
            let result = (|| -> Result<(), String> {
                let tree = normalize_source(&source).map_err(|e| e.to_string())?;
                check_normal_form(&tree).map_err(|e| e.to_string())?;
                lua.load(print(&tree))
                    .into_function()
                    .map_err(|e| e.to_string())?;
                Ok(())
            })();
            if let Err(error) = result {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
    }
    assert!(count > 0, "empty corpus");
    assert!(
        failures.is_empty(),
        "{} of {count} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{count} files normalized and compiled");
}

#[test]
fn differential_fixture_keeps_results_and_call_trace() {
    let source = include_str!("fixtures/normalize.lua");
    assert!(execute(source).is_ok());
    equivalent(source);
}

#[test]
fn large_constructors_use_private_spill_storage() {
    let fields = (1..=350)
        .map(|i| format!("f({i})"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "local function f(x) return x, x+1 end; local t={{{fields}}}; return table.concat(t, ',')"
    );
    assert!(normalized(&source).contains("local __lrs, __lrt"));
    equivalent(&source);
}
