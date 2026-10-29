//! The host division function the operator hook tests install.

use lua_resume::syntax::OperatorHooks;
use mlua::{Lua, Value};

/// The name the tests give the division hook.
pub const DIVIDE: &str = "__lr_div";

pub fn division_hooks() -> OperatorHooks {
    OperatorHooks::none().with_divide(DIVIDE).unwrap()
}

/// Division as the Stranded engine does it: two integers divide as integers, truncating toward
/// zero, and any float operand makes a float division.
fn host_divide(_: &Lua, (a, b): (Value, Value)) -> mlua::Result<Value> {
    match (a, b) {
        (Value::Integer(_), Value::Integer(0)) => {
            Err(mlua::Error::runtime("attempt to divide by zero"))
        }
        (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a.wrapping_div(b))),
        (Value::Integer(a), Value::Number(b)) => Ok(Value::Number(a as f64 / b)),
        (Value::Number(a), Value::Integer(b)) => Ok(Value::Number(a / b as f64)),
        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a / b)),
        _ => Err(mlua::Error::runtime("attempt to perform arithmetic")),
    }
}

/// Installs the division function under the name `hooks` prints, if it prints one.
pub fn install(lua: &Lua, hooks: &OperatorHooks) {
    if let Some(name) = hooks.divide() {
        let divide = lua.create_function(host_divide).unwrap();
        lua.globals().set(name, divide).unwrap();
    }
}
