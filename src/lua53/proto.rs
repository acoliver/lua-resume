//! The parts of a Lua 5.3 function prototype that frame mapping reads.

/// A constant from a standard chunk.
#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
}

/// An upvalue descriptor. `name` is empty when the prototype carries no debug names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upvalue {
    pub in_stack: bool,
    pub index: u8,
    pub name: Vec<u8>,
}

/// A local variable and the half-open range of instructions where it is active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalVar {
    pub name: Vec<u8>,
    pub start_pc: u32,
    pub end_pc: u32,
}

/// A Lua 5.3 function prototype. `K` is the type of a constant and `C` the type of a nested
/// prototype, so a host that keeps its own constants or shares prototypes by reference can use
/// its own types.
#[derive(Clone, Debug, PartialEq)]
pub struct Proto<K, C> {
    pub source: Vec<u8>,
    pub line_defined: u32,
    pub last_line_defined: u32,
    pub num_params: u8,
    pub is_vararg: bool,
    pub max_stack: u8,
    pub code: Vec<u32>,
    pub constants: Vec<K>,
    pub children: Vec<C>,
    pub upvalues: Vec<Upvalue>,
    pub line_info: Vec<u32>,
    pub locals: Vec<LocalVar>,
}

impl<K, C> Proto<K, C> {
    /// The same prototype without constants and nested prototypes, which frame mapping does
    /// not read.
    pub fn erased(&self) -> Proto<(), ()> {
        Proto {
            source: self.source.clone(),
            line_defined: self.line_defined,
            last_line_defined: self.last_line_defined,
            num_params: self.num_params,
            is_vararg: self.is_vararg,
            max_stack: self.max_stack,
            code: self.code.clone(),
            constants: Vec::new(),
            children: Vec::new(),
            upvalues: self.upvalues.clone(),
            line_info: self.line_info.clone(),
            locals: self.locals.clone(),
        }
    }
}
