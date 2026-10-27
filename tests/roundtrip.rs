//! The printer's output compiles to the same code as its input.
//!
//! The oracle is Lua itself: the original source and the printed tree are both compiled by
//! stock Lua 5.4 and dumped without debug information, and the dumps must be identical.

use lua_resume::syntax::{parse, print};
use mlua::{ChunkMode, Function, Lua};

fn compile(lua: &Lua, source: &[u8]) -> mlua::Result<Function> {
    lua.load(source)
        .set_mode(ChunkMode::Text)
        .set_name("=chunk")
        .into_function()
}

/// The chunk's bytecode without debug information, as `string.dump(f, true)` writes it.
fn stripped(function: &Function) -> Vec<u8> {
    function.dump(true)
}

fn assert_round_trip(lua: &Lua, source: &[u8]) {
    let original = compile(lua, source).expect("the original compiles");
    let tree = parse(source).expect("the original parses");
    let printed = print(&tree);
    let reprinted = compile(lua, printed.as_bytes())
        .unwrap_or_else(|error| panic!("printed source does not compile: {error}\n{printed}"));
    assert!(
        stripped(&original) == stripped(&reprinted),
        "compiled code differs\n--- printed ---\n{printed}"
    );
    let again = print(&parse(printed.as_bytes()).expect("printed source parses"));
    assert_eq!(printed, again, "printing is not stable");
}

#[test]
fn the_syntax_fixture_compiles_to_the_same_code() {
    let lua = Lua::new();
    assert_round_trip(&lua, include_bytes!("fixtures/syntax.lua"));
}

#[test]
fn edge_cases_compile_to_the_same_code() {
    let lua = Lua::new();
    for source in [
        "",
        "return",
        "return;",
        "local a = {} a.b = 1 ;(a).c = 2",
        "local s = 'a\\\n b'",
        "local s = 'tab\\9end\\0\\255'",
        "x = 1 --[[ comment ]] y = 2 -- trailing",
        "x = a\n(f)()",
        "x = {f = function() end, [1] = 2, 3,}",
        "x = - - - 1",
        "x = 2 ^ - 3",
        "x = not a == b",
        "x = 1 .. 2",
        "local function f() return f end",
        "goto e do end ::e::",
        "x = '\\u{10FFFF}\\u{0}'",
        "x = [=[\r\nline]=]",
        "\n\n\nfunction late() error('x') end",
    ] {
        assert_round_trip(&lua, source.as_bytes());
    }
}

#[test]
fn functions_and_errors_keep_their_lines() {
    let lua = Lua::new();
    let source = b"local t = {}\n\nfunction t.first()\n  return 1\nend\n\nt.second = function()\n\n  error('stop')\nend\nreturn t\n";
    let printed = print(&parse(source).unwrap());
    let lines = |chunk: &[u8]| -> String {
        let module: mlua::Table = compile(&lua, chunk).unwrap().call(()).unwrap();
        let span = |name: &str| {
            let info = module.get::<Function>(name).unwrap().info();
            format!(
                "{}-{}",
                info.line_defined.unwrap(),
                info.last_line_defined.unwrap()
            )
        };
        let error = module
            .get::<Function>("second")
            .unwrap()
            .call::<()>(())
            .unwrap_err()
            .to_string();
        let error = error
            .lines()
            .next()
            .unwrap()
            .trim_start_matches("runtime error: ")
            .to_owned();
        format!("{} {} {error}", span("first"), span("second"))
    };
    assert_eq!(lines(source), "3-5 7-10 chunk:9: stop");
    assert_eq!(lines(printed.as_bytes()), lines(source));
}

#[test]
fn source_lua_refuses_is_refused() {
    let lua = Lua::new();
    for source in [
        "x = ",
        "x = 0x",
        "x = 1e",
        "x = 'open",
        "x = [[open",
        "local x <bad> = 1",
        "f() = 1",
        "return 1 x = 2",
        "x = '\\q'",
        "x = '\\x4'",
        "x = '\\256'",
        "(a)",
        "x = }",
        "for i do end",
    ] {
        assert!(
            compile(&lua, source.as_bytes()).is_err(),
            "Lua accepts {source:?}"
        );
        assert!(
            parse(source.as_bytes()).is_err(),
            "parser accepts {source:?}"
        );
    }
}

#[test]
fn nesting_is_bounded() {
    let deep = format!("x = {}1{}", "(".repeat(500), ")".repeat(500));
    let error = parse(deep.as_bytes()).unwrap_err();
    assert!(error.message.contains("too many syntax levels"), "{error}");
}

/// Every `.lua` file under `LUA_RESUME_CORPUS`, such as a game's original scripts.
#[test]
#[ignore = "needs a local corpus in LUA_RESUME_CORPUS"]
fn a_local_corpus_compiles_to_the_same_code() {
    let root = std::env::var_os("LUA_RESUME_CORPUS").expect("LUA_RESUME_CORPUS");
    let mut files = Vec::new();
    collect(std::path::Path::new(&root), &mut files);
    let lua = Lua::new();
    let mut failures = Vec::new();
    for file in &files {
        let source = std::fs::read(file).unwrap();
        if compile(&lua, &source).is_err() {
            continue;
        }
        let result = std::panic::catch_unwind(|| assert_round_trip(&Lua::new(), &source));
        if result.is_err() {
            failures.push(file.display().to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files differ: {:?}",
        failures.len(),
        files.len(),
        &failures[..failures.len().min(10)]
    );
    eprintln!("{} files compile to the same code", files.len());
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "lua") {
            out.push(path);
        }
    }
}
