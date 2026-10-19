use std::fmt;

use crate::ast::{BinaryOp, Expr, Stmt, StmtKind, UnaryOp};
use crate::chunk::{Chunk, Op};
use crate::value::Value;

/// A construct the compiler cannot turn into bytecode yet.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub message: String,
    pub line: usize,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[line {}] Compile Error: {}", self.line, self.message)
    }
}

/// Compiles an expression statement into a chunk that leaves its value on
/// the stack and returns it.
pub fn compile(stmt: &Stmt) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler {
        chunk: Chunk::default(),
        line: stmt.line,
    };
    match &stmt.kind {
        StmtKind::Expr(expr) => compiler.expression(expr)?,
        _ => return Err(compiler.unsupported(&format!("`{}`", kind(&stmt.kind)))),
    }
    compiler.emit(Op::Return);
    Ok(compiler.chunk)
}

struct Compiler {
    chunk: Chunk,
    line: usize,
}

impl Compiler {
    fn emit(&mut self, op: Op) {
        self.chunk.write(op, self.line);
    }

    fn constant(&mut self, value: Value) {
        let index = self.chunk.add_constant(value);
        self.emit(Op::Constant(index));
    }

    fn expression(&mut self, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::Int(v) => self.constant(Value::Int(*v)),
            Expr::Float(v) => self.constant(Value::Float(*v)),
            Expr::Str(v) => self.constant(Value::Str(v.clone())),
            Expr::Symbol(v) => self.constant(Value::Symbol(v.clone())),
            Expr::Bool(true) => self.emit(Op::True),
            Expr::Bool(false) => self.emit(Op::False),
            Expr::Unary(op, right) => {
                self.expression(right)?;
                self.emit(match op {
                    UnaryOp::Neg => Op::Negate,
                    UnaryOp::Not => Op::Not,
                });
            }
            Expr::Binary(_, op @ (BinaryOp::And | BinaryOp::Or), _) => {
                return Err(self.unsupported(&format!("`{op}`, which needs jumps,")));
            }
            Expr::Binary(left, op, right) => {
                self.expression(left)?;
                self.expression(right)?;
                self.emit(binary(*op));
            }
            Expr::Ident(name) => {
                return Err(self.unsupported(&format!("the variable `{name}`")));
            }
            Expr::Call(..) => return Err(self.unsupported("a call")),
            Expr::Field(..) => return Err(self.unsupported("a field access")),
            Expr::Block(..) => return Err(self.unsupported("a block")),
            Expr::If(..) => return Err(self.unsupported("`if`")),
            Expr::List(..) => return Err(self.unsupported("a list")),
            Expr::Struct { .. } => return Err(self.unsupported("a struct")),
        }
        Ok(())
    }

    fn unsupported(&self, what: &str) -> CompileError {
        CompileError {
            message: format!("{what} cannot be compiled to bytecode yet"),
            line: self.line,
        }
    }
}

fn binary(op: BinaryOp) -> Op {
    match op {
        BinaryOp::Add => Op::Add,
        BinaryOp::Sub => Op::Sub,
        BinaryOp::Mul => Op::Mul,
        BinaryOp::Div => Op::Div,
        BinaryOp::Rem => Op::Rem,
        BinaryOp::Eq => Op::Equal,
        BinaryOp::Ne => Op::NotEqual,
        BinaryOp::Lt => Op::Less,
        BinaryOp::Le => Op::LessEqual,
        BinaryOp::Gt => Op::Greater,
        BinaryOp::Ge => Op::GreaterEqual,
        BinaryOp::And | BinaryOp::Or => unreachable!("`and` and `or` short circuit"),
    }
}

fn kind(kind: &StmtKind) -> &'static str {
    match kind {
        StmtKind::Let(..) => "let",
        StmtKind::Fn(..) => "fn",
        StmtKind::Struct(..) => "struct",
        StmtKind::Sequence(..) => "sequence",
        StmtKind::Start(..) => "start",
        StmtKind::Expr(..) => "expression",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    fn compiled(source: &str) -> Result<Chunk, CompileError> {
        let statements = parse(scan_tokens(source).unwrap()).unwrap();
        compile(&statements[0])
    }

    #[test]
    fn compiles_operands_before_their_operator() {
        let chunk = compiled("-(1 + 2) * 3").unwrap();
        assert_eq!(
            chunk.code,
            [
                Op::Constant(0),
                Op::Constant(1),
                Op::Add,
                Op::Negate,
                Op::Constant(2),
                Op::Mul,
                Op::Return
            ]
        );
        assert_eq!(
            chunk.constants,
            [Value::Int(1), Value::Int(2), Value::Int(3)]
        );
    }

    #[test]
    fn disassembles_a_chunk() {
        let chunk = compiled("\n\"a\" + \"b\" == \"ab\"").unwrap();
        assert_eq!(
            chunk.to_string(),
            "0000    2 Constant            0 \"a\"\n\
             0001    | Constant            1 \"b\"\n\
             0002    | Add\n\
             0003    | Constant            2 \"ab\"\n\
             0004    | Equal\n\
             0005    | Return\n"
        );
    }
}
