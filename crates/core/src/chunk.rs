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
    /// Ends the chunk with the value on top of the stack, if the statement
    /// left one: an expression does, a `let` does not.
    Return,
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

/// A compiled piece of code: the instructions, the line each one came from
/// and the constants they refer to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chunk {
    pub code: Vec<Op>,
    pub lines: Vec<usize>,
    pub constants: Vec<Value>,
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
