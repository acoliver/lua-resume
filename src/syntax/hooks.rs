//! Host-defined operator semantics that are not call sites.
//!
//! A game's VM may give an operator different semantics from stock Lua 5.4. The Lua 5.3 VM of
//! Stranded, for example, divides two integers as integers. A host that runs such scripts on
//! stock Lua 5.4 prints the source it loads with [`print_with`](super::print_with) and an
//! [`OperatorHooks`], which writes each binary `/` as a call of a host function.
//!
//! The hook is a printing option and the syntax tree keeps `a / b`. Normalization, call
//! ordinals, site numbers, continuation generation and the mapping to Lua 5.3 prototypes all
//! work on the tree, so none of them sees an extra call. The compiled form of the printed
//! source does have one, which is why the dump handed to [`Compiled`](crate::compiled::Compiled)
//! must come from the original source, as it must without hooks.
//!
//! Rules for the host function:
//!
//! - It must return exactly one value and must not yield. A coroutine is never stopped inside
//!   it, so capture never sees its frame. A hook that yields leaves a frame that capture
//!   refuses, because it has no site marker.
//! - It is looked up as a global each time, through the `_ENV` of the code that uses it. Its
//!   name starts with the reserved `__lr` prefix, so no variable of the game can shadow it.
//!
//! Operands are evaluated left, then right, exactly once, before the host function runs, as
//! for the operator. The printed call is parenthesized, so it is neither a tail call nor
//! multiple results.

use crate::rewrite::TEMP_PREFIX;

/// The host functions that replace operators when source is printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperatorHooks {
    divide: Option<String>,
}

/// A hook name the printer cannot use.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("operator hook name `{0}` must be a Lua name that starts with `{TEMP_PREFIX}`")]
pub struct HookNameError(pub String);

impl OperatorHooks {
    /// No hooks: operators print as they are.
    pub fn none() -> Self {
        Self::default()
    }

    /// Prints binary `/` as a call of the global function `name`.
    pub fn with_divide(mut self, name: &str) -> Result<Self, HookNameError> {
        let valid = name.starts_with(TEMP_PREFIX)
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid {
            return Err(HookNameError(name.to_owned()));
        }
        self.divide = Some(name.to_owned());
        Ok(self)
    }

    /// The function that replaces `/`, if any.
    pub fn divide(&self) -> Option<&str> {
        self.divide.as_deref()
    }
}
