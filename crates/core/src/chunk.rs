use std::fmt;

use crate::value::Value;

/// One instruction of the virtual machine. Operands live inside the variant
/// instead of in the bytes that follow the opcode, as they do in clox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Constant(usize),
    True,
    False,
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
    Return,
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

    pub fn disassemble_op(&self, offset: usize) -> String {
        let line = if offset > 0 && self.lines[offset] == self.lines[offset - 1] {
            "   |".to_string()
        } else {
            format!("{:4}", self.lines[offset])
        };
        let op = match self.code[offset] {
            Op::Constant(index) => {
                let constant = match &self.constants[index] {
                    Value::Str(v) => format!("{v:?}"),
                    v => v.to_string(),
                };
                format!("{:<16} {index:4} {constant}", "Constant")
            }
            op => format!("{op:?}"),
        };
        format!("{offset:04} {line} {op}")
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
