//! Trees built in code, which have no `Paren` nodes, print with the grouping of the tree.
//!
//! The rewrite builds such trees, so the printer must add parentheses wherever operator
//! priority would otherwise regroup the operands. Each printed expression is evaluated by Lua
//! and compared with the same expression written with explicit parentheses everywhere.

use lua_resume::syntax::ast::{BinOp, Block, Call, Expr, Return, UnOp};
use lua_resume::syntax::print;
use mlua::Lua;

const OPS: [BinOp; 13] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::IDiv,
    BinOp::Mod,
    BinOp::Pow,
    BinOp::Concat,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::BAnd,
    BinOp::BOr,
    BinOp::BXor,
];

fn num(text: &str) -> Expr {
    Expr::Number(text.into())
}

fn bin(op: BinOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary(op, Box::new(left), Box::new(right))
}

/// The expression with parentheses around every operation.
fn explicit(expr: &Expr) -> String {
    match expr {
        Expr::Number(text) => text.clone(),
        Expr::Binary(op, left, right) => {
            format!("({} {} {})", explicit(left), op.text(), explicit(right))
        }
        Expr::Unary(op, operand) => format!("({}{})", op.text(), explicit(operand)),
        other => panic!("unexpected {other:?}"),
    }
}

/// `return expr`, printed by the printer.
fn printed(expr: &Expr) -> String {
    print(&Block {
        stats: Vec::new(),
        ret: Some(Return {
            line: 1,
            values: vec![expr.clone()],
        }),
    })
}

/// What evaluating the source gives: its value, or the fact that it raised an error.
fn evaluate(lua: &Lua, source: &str) -> String {
    match lua.load(source).eval::<mlua::Value>() {
        Ok(value) => format!("{value:?}"),
        Err(_) => "error".into(),
    }
}

fn assert_grouping(lua: &Lua, expr: &Expr) {
    let source = printed(expr);
    let expected = evaluate(lua, &format!("return {}", explicit(expr)));
    assert_eq!(evaluate(lua, &source), expected, "{source}");
}

#[test]
fn binary_operands_keep_their_grouping() {
    let lua = Lua::new();
    for outer in OPS {
        for inner in OPS {
            let left = bin(outer, bin(inner, num("7"), num("3")), num("2"));
            let right = bin(outer, num("7"), bin(inner, num("3"), num("2")));
            assert_grouping(&lua, &left);
            assert_grouping(&lua, &right);
        }
    }
}

#[test]
fn unary_operands_keep_their_grouping() {
    let lua = Lua::new();
    for op in OPS {
        let negated = Expr::Unary(UnOp::Neg, Box::new(bin(op, num("7"), num("2"))));
        let on_left = bin(op, Expr::Unary(UnOp::Neg, Box::new(num("7"))), num("2"));
        assert_grouping(&lua, &negated);
        assert_grouping(&lua, &on_left);
    }
    let double = Expr::Unary(
        UnOp::Neg,
        Box::new(Expr::Unary(UnOp::Neg, Box::new(num("7")))),
    );
    assert_grouping(&lua, &double);
}

#[test]
fn non_prefix_expressions_are_parenthesized_as_callees() {
    let lua = Lua::new();
    let call = Expr::Call(Box::new(Call {
        line: 1,
        callee: Expr::Str(b"ab".to_vec()),
        method: Some("rep".into()),
        args: vec![num("2")],
    }));
    let index = Expr::Index(Box::new(Expr::Table(vec![])), Box::new(num("1")));
    assert_eq!(evaluate(&lua, &printed(&call)), r#"String("abab")"#);
    assert_eq!(evaluate(&lua, &printed(&index)), "Nil");
}
