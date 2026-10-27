//! The Lua 5.4 syntax tree.
//!
//! The tree keeps every distinction that changes the compiled code: parentheses, method
//! calls, the original text of numerals, and the difference between `a.b` and `a["b"]`.
//! Comments, the spelling of strings and the layout of the source are not kept.

/// A Lua name.
pub type Name = String;

/// A sequence of statements with an optional final `return`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub stats: Vec<Stat>,
    pub ret: Option<Return>,
}

/// `return values`.
#[derive(Clone, Debug, PartialEq)]
pub struct Return {
    pub line: u32,
    pub values: Vec<Expr>,
}

/// A statement and the line it starts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub line: u32,
    pub kind: StatKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatKind {
    /// `targets = values`; each target is a name, field or index.
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    /// A function or method call used as a statement.
    Call(Box<Call>),
    Label(Name),
    Goto(Name),
    Break,
    Do(Block),
    While {
        cond: Expr,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: Expr,
    },
    If {
        arms: Vec<IfArm>,
        otherwise: Option<Block>,
    },
    NumericFor {
        var: Name,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        body: Block,
    },
    GenericFor {
        names: Vec<Name>,
        exprs: Vec<Expr>,
        body: Block,
    },
    /// `function a.b.c:m() ... end`.
    Function {
        name: FuncName,
        body: Box<FuncBody>,
    },
    LocalFunction {
        name: Name,
        body: Box<FuncBody>,
    },
    Local {
        names: Vec<LocalName>,
        values: Vec<Expr>,
    },
}

/// One `if` or `elseif` condition and its block.
#[derive(Clone, Debug, PartialEq)]
pub struct IfArm {
    pub line: u32,
    pub cond: Expr,
    pub body: Block,
}

/// A name declared by `local`, with its attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalName {
    pub name: Name,
    pub attrib: Option<Attrib>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attrib {
    Const,
    Close,
}

/// The name of a `function` statement: a dotted path and an optional method name.
#[derive(Clone, Debug, PartialEq)]
pub struct FuncName {
    pub path: Vec<Name>,
    pub method: Option<Name>,
}

/// Parameters and body of a function, with the lines of `function` and `end`.
#[derive(Clone, Debug, PartialEq)]
pub struct FuncBody {
    pub line: u32,
    pub end_line: u32,
    pub params: Vec<Name>,
    pub vararg: bool,
    pub body: Block,
}

/// A call `callee(args)` or a method call `callee:method(args)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub line: u32,
    pub callee: Expr,
    pub method: Option<Name>,
    pub args: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Nil,
    True,
    False,
    Vararg,
    /// A numeral, as written in the source.
    Number(String),
    /// A string's bytes after escapes are decoded.
    Str(Vec<u8>),
    Function(Box<FuncBody>),
    Table(Vec<Field>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Name(Name),
    /// `obj.name`.
    Field(Box<Expr>, Name),
    /// `obj[key]`.
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Call>),
    /// `(expr)`, which also truncates a call or `...` to one value.
    Paren(Box<Expr>),
}

/// A table constructor entry.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    /// A positional item.
    Item(Expr),
    /// `name = value`.
    Named(Name, Expr),
    /// `[key] = value`.
    Keyed(Expr, Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Lt,
    Gt,
    Le,
    Ge,
    Ne,
    Eq,
    BOr,
    BXor,
    BAnd,
    Shl,
    Shr,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
}

impl BinOp {
    /// Left and right priorities, as in Lua's `lparser.c`.
    pub fn priority(self) -> (u8, u8) {
        match self {
            Self::Or => (1, 1),
            Self::And => (2, 2),
            Self::Lt | Self::Gt | Self::Le | Self::Ge | Self::Ne | Self::Eq => (3, 3),
            Self::BOr => (4, 4),
            Self::BXor => (5, 5),
            Self::BAnd => (6, 6),
            Self::Shl | Self::Shr => (7, 7),
            Self::Concat => (9, 8),
            Self::Add | Self::Sub => (10, 10),
            Self::Mul | Self::Div | Self::IDiv | Self::Mod => (11, 11),
            Self::Pow => (14, 13),
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Le => "<=",
            Self::Ge => ">=",
            Self::Ne => "~=",
            Self::Eq => "==",
            Self::BOr => "|",
            Self::BXor => "~",
            Self::BAnd => "&",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::Concat => "..",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::IDiv => "//",
            Self::Mod => "%",
            Self::Pow => "^",
        }
    }
}

/// The priority unary operators bind their operand with.
pub const UNARY_PRIORITY: u8 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BNot,
}

impl UnOp {
    pub fn text(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "not ",
            Self::Len => "#",
            Self::BNot => "~",
        }
    }
}
