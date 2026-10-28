//! Reject malformed call positions and marker sequences, including inside nested functions.

mod support;

use support::normalize_source;

use lua_resume::{rewrite::check_normal_form, syntax::parse};

#[test]
fn invalid_forms_are_rejected() {
    for source in [
        "f()",
        "__lrs=1; f()",
        "local __lrs; __lrs=2; f()",
        "local __lrs; __lrs=1; local x=1+f()",
        "local __lrs; __lrs=1; f(g(),h())",
        "local __lrs; __lrs=1; local t={g(),h()}",
        "local __lrs; __lrs=1; t.x=f()",
        "local __lrs; __lrs=1; while f() do end",
        "local __lrs; __lrs=1; local x=1",
        "local __lrs; __lrs=1; local x=function() f() end",
        "for k,v in iter do end",
    ] {
        let block = parse(source.as_bytes()).unwrap();
        assert!(check_normal_form(&block).is_err(), "{source}");
    }
}

#[test]
fn nested_functions_have_independent_site_sequences() {
    let block =
        normalize_source(b"f(); local function nested() g(); return h() end; return j()").unwrap();
    check_normal_form(&block).unwrap();
    let mut missing = block.clone();
    missing.stats.remove(1);
    assert!(check_normal_form(&missing).is_err());
    let mut declaration = block;
    declaration.stats.remove(0);
    assert!(check_normal_form(&declaration).is_err());
}
