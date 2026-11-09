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

/// Compiles a top-level statement into a chunk. An expression returns its
/// value; a `let` binds a global and returns nothing. The VM keeps the
/// globals from one chunk to the next.
pub fn compile(stmt: &Stmt) -> Result<Chunk, CompileError> {
    let mut compiler = Compiler {
        chunk: Chunk::default(),
        line: stmt.line,
        locals: Vec::new(),
        scope_depth: 0,
        height: 0,
    };
    match &stmt.kind {
        StmtKind::Expr(expr) => compiler.expression(expr)?,
        _ => compiler.statement(stmt)?,
    }
    compiler.emit(Op::Return);
    Ok(compiler.chunk)
}

/// A local variable while its block is being compiled.
struct Local {
    name: String,
    /// How many blocks deep it was declared, to know which block ends it.
    depth: usize,
    /// Where it lives on the stack.
    slot: usize,
}

struct Compiler {
    chunk: Chunk,
    line: usize,
    /// The locals in scope, innermost last, so the newest of several with
    /// the same name is found first.
    locals: Vec<Local>,
    scope_depth: usize,
    /// How many values the stack holds at this point of the code. In clox a
    /// block is a statement, so it starts with only locals on the stack and a
    /// local's slot is its index among them. In ludo a block is an
    /// expression and can start above temporaries, like the `1` in
    /// `1 + { let a = 2 ... }`, so a local's slot comes from this height.
    height: usize,
}

impl Compiler {
    fn emit(&mut self, op: Op) {
        self.chunk.write(op, self.line);
        self.height = self
            .height
            .checked_add_signed(stack_effect(op))
            .expect("the compiler never pops an empty stack");
    }

    /// Compiles a statement, which leaves the stack as it found it except
    /// for the new local of a `let` inside a block.
    fn statement(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        let outer = std::mem::replace(&mut self.line, stmt.line);
        match &stmt.kind {
            StmtKind::Let(name, value) if self.scope_depth == 0 => {
                let global = self.identifier_constant(name);
                self.expression(value)?;
                self.emit(Op::DefineGlobal(global));
            }
            StmtKind::Let(name, value) => {
                // The value stays where it was pushed and becomes the local.
                // It is declared only now, so its own initializer still sees
                // the outer variable with that name.
                self.expression(value)?;
                self.locals.push(Local {
                    name: name.clone(),
                    depth: self.scope_depth,
                    slot: self.height - 1,
                });
            }
            StmtKind::Expr(expr) => {
                self.expression(expr)?;
                self.emit(Op::Pop);
            }
            other => return Err(self.unsupported(&format!("`{}`", kind(other)))),
        }
        self.line = outer;
        Ok(())
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    /// Drops the locals of the block that ends, which sit right below its
    /// value on the stack.
    fn end_scope(&mut self) {
        self.scope_depth -= 1;
        let count = self
            .locals
            .iter()
            .rev()
            .take_while(|local| local.depth > self.scope_depth)
            .count();
        self.locals.truncate(self.locals.len() - count);
        if count > 0 {
            self.emit(Op::PopBelow(count));
        }
    }

    /// The slot of the innermost local with this name. Without one, the name
    /// is a global.
    fn resolve_local(&self, name: &str) -> Option<usize> {
        self.locals
            .iter()
            .rev()
            .find(|local| local.name == name)
            .map(|local| local.slot)
    }

    fn constant(&mut self, value: Value) {
        let index = self.chunk.add_constant(value);
        self.emit(Op::Constant(index));
    }

    /// Stores a global's name as a string constant, for the instructions
    /// that look it up when they run.
    fn identifier_constant(&mut self, name: &str) -> usize {
        self.chunk.add_constant(Value::Str(name.to_string()))
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
            Expr::Ident(name) => match self.resolve_local(name) {
                Some(slot) => self.emit(Op::GetLocal(slot)),
                None => {
                    let global = self.identifier_constant(name);
                    self.emit(Op::GetGlobal(global));
                }
            },
            Expr::Block(statements, tail, line) => {
                self.begin_scope();
                for stmt in statements {
                    self.statement(stmt)?;
                }
                let outer = std::mem::replace(&mut self.line, *line);
                self.expression(tail)?;
                self.end_scope();
                self.line = outer;
            }
            Expr::Call(..) => return Err(self.unsupported("a call")),
            Expr::Field(..) => return Err(self.unsupported("a field access")),
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

/// How many values an instruction pushes minus how many it pops.
fn stack_effect(op: Op) -> isize {
    match op {
        Op::Constant(_) | Op::True | Op::False | Op::GetGlobal(_) | Op::GetLocal(_) => 1,
        // `Return` ends the chunk, so nothing after it counts the stack.
        Op::Negate | Op::Not | Op::Return => 0,
        Op::PopBelow(count) => -(count as isize),
        Op::DefineGlobal(_)
        | Op::Pop
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Rem
        | Op::Equal
        | Op::NotEqual
        | Op::Less
        | Op::LessEqual
        | Op::Greater
        | Op::GreaterEqual => -1,
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

    #[test]
    fn names_globals_with_string_constants() {
        let chunk = compiled("let speed = 2 * dt").unwrap();
        assert_eq!(
            chunk.to_string(),
            "0000    1 Constant            1 2\n\
             0001    | GetGlobal           2 \"dt\"\n\
             0002    | Mul\n\
             0003    | DefineGlobal        0 \"speed\"\n\
             0004    | Return\n"
        );
    }

    #[test]
    fn locals_live_in_stack_slots_above_temporaries() {
        let chunk = compiled("let r = 1 + {\n  let a = 2\n  a * a\n}").unwrap();
        assert_eq!(
            chunk.to_string(),
            "0000    1 Constant            1 1\n\
             0001    2 Constant            2 2\n\
             0002    3 GetLocal            1\n\
             0003    | GetLocal            1\n\
             0004    | Mul\n\
             0005    | PopBelow            1\n\
             0006    1 Add\n\
             0007    | DefineGlobal        0 \"r\"\n\
             0008    | Return\n"
        );
    }
}
