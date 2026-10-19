use crate::ast::{BinaryOp, UnaryOp};
use crate::chunk::{Chunk, Op};
use crate::interpreter::{binary, unary, RuntimeError};
use crate::value::Value;

/// A stack machine that runs a chunk. The operators share their meaning with
/// the tree-walking interpreter, so both give the same values and errors.
#[derive(Default)]
pub struct Vm {
    stack: Vec<Value>,

    pub trace: bool,
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(&mut self, chunk: &Chunk) -> Result<Value, RuntimeError> {
        self.stack.clear();
        let mut ip = 0;
        loop {
            if self.trace {
                let stack: Vec<String> = self.stack.iter().map(|v| format!("[ {v} ]")).collect();
                println!("          {}", stack.join(""));
                println!("{}", chunk.disassemble_op(ip));
            }

            let op = chunk.code[ip];
            ip += 1;
            let result = match op {
                Op::Constant(index) => {
                    self.push(chunk.constants[index].clone());
                    Ok(())
                }
                Op::True => {
                    self.push(Value::Bool(true));
                    Ok(())
                }
                Op::False => {
                    self.push(Value::Bool(false));
                    Ok(())
                }
                Op::Negate => self.unary(UnaryOp::Neg),
                Op::Not => self.unary(UnaryOp::Not),
                Op::Add => self.binary(BinaryOp::Add),
                Op::Sub => self.binary(BinaryOp::Sub),
                Op::Mul => self.binary(BinaryOp::Mul),
                Op::Div => self.binary(BinaryOp::Div),
                Op::Rem => self.binary(BinaryOp::Rem),
                Op::Equal => self.binary(BinaryOp::Eq),
                Op::NotEqual => self.binary(BinaryOp::Ne),
                Op::Less => self.binary(BinaryOp::Lt),
                Op::LessEqual => self.binary(BinaryOp::Le),
                Op::Greater => self.binary(BinaryOp::Gt),
                Op::GreaterEqual => self.binary(BinaryOp::Ge),
                Op::Return => return Ok(self.pop()),
            };
            result.map_err(|e| RuntimeError {
                line: Some(chunk.lines[ip - 1]),
                ..e
            })?;
        }
    }

    fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    fn pop(&mut self) -> Value {
        self.stack
            .pop()
            .expect("the compiler never pops an empty stack")
    }

    fn unary(&mut self, op: UnaryOp) -> Result<(), RuntimeError> {
        let right = self.pop();
        self.push(unary(op, right)?);
        Ok(())
    }

    fn binary(&mut self, op: BinaryOp) -> Result<(), RuntimeError> {
        let right = self.pop();
        let left = self.pop();
        self.push(binary(op, left, right)?);
        Ok(())
    }
}
