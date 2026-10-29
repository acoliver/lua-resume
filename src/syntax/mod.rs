//! Lua 5.4 source: parsing to a syntax tree and printing it back.

pub mod ast;
mod expr;
mod hooks;
mod lexer;
mod numeral;
mod parser;
mod print;

pub use hooks::{HookNameError, OperatorHooks};
pub use print::{print, print_with};

/// Source that Lua 5.4 would refuse to compile, with the line where parsing stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

/// Parses a Lua 5.4 chunk.
pub fn parse(source: &[u8]) -> Result<ast::Block, ParseError> {
    let tokens = lexer::tokenize(source)?;
    parser::Parser::new(tokens).chunk()
}
