use std::fmt;

use crate::value::Value;

/// One instruction of the virtual machine. Operands live inside the variant
/// instead of in the bytes that follow the opcode, as they do in clox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Constant(usize),
    True,
    False,
    /// Binds the name in this constant to the value on top of the stack, and
    /// pops it.
    DefineGlobal(usize),
    /// Pushes the value bound to the name in this constant.
    GetGlobal(usize),
    /// Pushes a copy of the local in this stack slot.
    GetLocal(usize),
    /// Discards the value on top of the stack.
    Pop,
    /// Discards this many values under the one on top: the locals of a block
    /// that ends, below the value of the block. clox has no such instruction
    /// because its blocks are statements and leave no value.
    PopBelow(usize),
    /// Fails unless the value on top of the stack is a Bool, which it leaves
    /// there. clox lets any value be false; ludo has no truthiness, so `if`,
    /// `and` and `or` check their values before a jump reads them.
    CheckBool(Test),
    /// Skips this many instructions forward.
    Jump(usize),
    /// Skips this many instructions forward if the Bool on top of the stack
    /// is false, and leaves it there, as in clox.
    JumpIfFalse(usize),
    /// Calls the value below this many arguments on top of the stack, and
    /// leaves the result in place of the call.
    Call(usize),
    /// Builds a list from this many values on top of the stack.
    List(usize),
    /// Declares the struct in this shape. Literals look it up by name when
    /// they run, as globals are, so a struct can be declared after a
    /// function that builds it.
    DefineStruct(usize),
    /// Builds a struct from the values of the fields its shape lists, on top
    /// of the stack, and, if `base`, the struct it updates right below them.
    Struct {
        shape: usize,
        base: bool,
    },
    /// Replaces the struct on top of the stack with its field named by this
    /// constant.
    GetField(usize),
    Negate,
    Not,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    /// Ends the function and leaves its result in place of the call. A
    /// function body always leaves a result; a top-level `let` or
    /// declaration leaves none.
    Return,
}

/// A function compiled to bytecode, clox's `ObjFunction`. A top-level
/// statement compiles into one too, with no name, as clox does with the
/// whole script.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Function {
    pub name: Option<String>,
    pub arity: usize,
    pub chunk: Chunk,
}

impl Function {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("script")
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "<fn {name}>"),
            None => f.write_str("<script>"),
        }
    }
}

/// What needs the Bool that `CheckBool` checks, for its error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Test {
    If,
    And,
    Or,
}

impl fmt::Display for Test {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Test::If => "if",
            Test::And => "and",
            Test::Or => "or",
        })
    }
}

/// A struct as an instruction names it: the struct and some of its fields. A
/// declaration lists all of them in order; a literal lists the ones it
/// gives, in the order they are written. clox has no such thing, since its
/// instances get their fields one at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub name: String,
    pub fields: Vec<String>,
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {{ {} }}", self.name, self.fields.join(", "))
    }
}

/// A compiled piece of code: the instructions, the line each one came from
/// and the constants and struct shapes they refer to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub lines: Vec<usize>,
    pub constants: Vec<Value>,
    pub shapes: Vec<Shape>,
}

impl Chunk {
    pub fn write(&mut self, op: Op, line: usize) {
        self.code.push(op);
        self.lines.push(line);
    }

    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.push(value);
        self.constants.len() - 1
    }

    pub fn add_shape(&mut self, shape: Shape) -> usize {
        self.shapes.push(shape);
        self.shapes.len() - 1
    }

    /// The name of a global, which instructions refer to by the index of a
    /// string constant, as in clox.
    pub fn name(&self, index: usize) -> &str {
        match &self.constants[index] {
            Value::Str(name) => name,
            other => unreachable!("globals are named by a string constant, got {other}"),
        }
    }

    pub fn disassemble_op(&self, offset: usize) -> String {
        let line = if offset > 0 && self.lines[offset] == self.lines[offset - 1] {
            "   |".to_string()
        } else {
            format!("{:4}", self.lines[offset])
        };
        let op = match self.code[offset] {
            Op::Constant(index) => self.constant_op("Constant", index),
            Op::DefineGlobal(index) => self.constant_op("DefineGlobal", index),
            Op::GetGlobal(index) => self.constant_op("GetGlobal", index),
            Op::GetLocal(slot) => format!("{:<16} {slot:4}", "GetLocal"),
            Op::PopBelow(count) => format!("{:<16} {count:4}", "PopBelow"),
            Op::Call(argc) => format!("{:<16} {argc:4}", "Call"),
            Op::List(count) => format!("{:<16} {count:4}", "List"),
            Op::DefineStruct(shape) => {
                format!("{:<16} {shape:4} {}", "DefineStruct", self.shapes[shape])
            }
            Op::Struct { shape, base } => {
                let shown = &self.shapes[shape];
                let base = if base { "_ | " } else { "" };
                let fields = shown.fields.join(", ");
                format!(
                    "{:<16} {shape:4} {} {{ {base}{fields} }}",
                    "Struct", shown.name
                )
            }
            Op::GetField(index) => self.constant_op("GetField", index),
            Op::CheckBool(test) => format!("{:<16} {test:>4}", "CheckBool"),
            Op::Jump(jump) => format!("{:<16} {jump:4} -> {}", "Jump", offset + 1 + jump),
            Op::JumpIfFalse(jump) => {
                format!("{:<16} {jump:4} -> {}", "JumpIfFalse", offset + 1 + jump)
            }
            op => format!("{op:?}"),
        };
        format!("{offset:04} {line} {op}")
    }

    fn constant_op(&self, name: &str, index: usize) -> String {
        let constant = match &self.constants[index] {
            Value::Str(v) => format!("{v:?}"),
            v => v.to_string(),
        };
        format!("{name:<16} {index:4} {constant}")
    }
}

/// The disassembler: one line per instruction, as in clox.
impl fmt::Display for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for offset in 0..self.code.len() {
            writeln!(f, "{}", self.disassemble_op(offset))?;
        }
        Ok(())
    }
}
