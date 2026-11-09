use std::collections::HashMap;

use crate::ast::{BinaryOp, UnaryOp};
use crate::chunk::{Chunk, Op, Test};
use crate::interpreter::{binary, condition, error, logical, unary, RuntimeError};
use crate::value::Value;

/// A stack machine that runs a chunk. The operators share their meaning with
/// the tree-walking interpreter, so both give the same values and errors.
#[derive(Default)]
pub struct Vm {
    stack: Vec<Value>,
    /// Looked up by name each time they are used, as in clox, so a reloaded
    /// game sees the new definitions.
    globals: HashMap<String, Value>,

    pub trace: bool,
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs a chunk and returns the value its statement left, if any.
    pub fn run(&mut self, chunk: &Chunk) -> Result<Option<Value>, RuntimeError> {
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
                Op::DefineGlobal(index) => {
                    let value = self.pop();
                    self.globals.insert(chunk.name(index).to_string(), value);
                    Ok(())
                }
                Op::GetGlobal(index) => {
                    let name = chunk.name(index);
                    match self.globals.get(name).cloned() {
                        Some(value) => {
                            self.push(value);
                            Ok(())
                        }
                        None => Err(error(format!("undefined variable `{name}`"))),
                    }
                }
                Op::GetLocal(slot) => {
                    let value = self.stack[slot].clone();
                    self.push(value);
                    Ok(())
                }
                Op::Pop => {
                    self.pop();
                    Ok(())
                }
                Op::PopBelow(count) => {
                    let top = self.pop();
                    self.stack.truncate(self.stack.len() - count);
                    self.push(top);
                    Ok(())
                }
                Op::CheckBool(test) => {
                    let value = self.peek();
                    match test {
                        Test::If => condition(value),
                        Test::And => logical(BinaryOp::And, value),
                        Test::Or => logical(BinaryOp::Or, value),
                    }
                    .map(drop)
                }
                Op::Jump(distance) => {
                    ip += distance;
                    Ok(())
                }
                Op::JumpIfFalse(distance) => {
                    if *self.peek() == Value::Bool(false) {
                        ip += distance;
                    }
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
                Op::Return => return Ok(self.stack.pop()),
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

    fn peek(&self) -> &Value {
        self.stack
            .last()
            .expect("the compiler never reads an empty stack")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile;
    use crate::interpreter::Interpreter;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    /// Runs `source` one statement at a time on the VM and on the
    /// interpreter, which must agree, and shows what each statement gave as
    /// the REPL does.
    fn outputs(source: &str) -> String {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        let statements = parse(tokens).expect("source should parse cleanly");
        let mut vm = Vm::new();
        let mut interpreter = Interpreter::new();
        let mut lines = Vec::new();
        for stmt in &statements {
            let chunk = compile(stmt).unwrap_or_else(|e| panic!("{e}"));
            let result = vm.run(&chunk);
            assert_eq!(
                result,
                interpreter.execute(stmt),
                "the VM and the interpreter disagree on `{stmt}`"
            );
            match result {
                Ok(value) => lines.extend(value.map(|v| v.to_string())),
                Err(e) => lines.push(format!("[line {}] error: {}", e.line.unwrap(), e.message)),
            }
        }
        lines.join("\n")
    }

    fn runs(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(outputs(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn evaluates_expressions() {
        runs(&[
            ("1 + 2 * 3\n-(7 % 4)\n7.0 / 2.0", "7\n-3\n3.5"),
            ("\"a\" + \"b\" == \"ab\"\n!(1 < 2)", "true\nfalse"),
            ("1\n1 / 0", "1\n[line 2] error: division by zero"),
        ]);
    }

    #[test]
    fn globals_are_bound_and_looked_up_by_name() {
        runs(&[
            ("let a = 2\nlet b = a * 3\na + b", "8"),
            ("let a = 1\nlet a = a + 1\na", "2"),
            (
                "x\nlet x = 1\nx\nlet y = z",
                "[line 1] error: undefined variable `x`\n1\n[line 4] error: undefined variable `z`",
            ),
        ]);
    }

    #[test]
    fn blocks_keep_their_locals_on_the_stack() {
        runs(&[
            (
                "let area = {\n  let w = 4\n  w * 5\n}\narea\nw",
                "20\n[line 6] error: undefined variable `w`",
            ),
            (
                "let x = 1\nlet y = {\n  let x = x + 10\n  x\n}\ny\nx",
                "11\n1",
            ),
            (
                "{\n  let a = 1\n  let b = {\n    let a = a + 10\n    a * 2\n  }\n  a + b\n}",
                "23",
            ),
            // A `let` can shadow another in the same block, and the value of
            // an expression statement is dropped.
            ("{\n  let a = 1\n  let a = a + 1\n  a * 10\n  a\n}", "2"),
            // A block can start above a temporary, which moves its locals up.
            ("1 + {\n  let a = 2\n  a * 3\n}", "7"),
        ]);
    }

    #[test]
    fn an_error_in_a_block_reports_the_inner_line() {
        runs(&[
            (
                "let y = {\n  let x = 1 / 0\n  x\n}",
                "[line 2] error: division by zero",
            ),
            (
                "let x = 1\n{\n  let x = 2\n  x / 0\n}\nx",
                "[line 4] error: division by zero\n1",
            ),
            ("{\n  1\n} + true\n", "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Bool"),
        ]);
    }

    #[test]
    fn if_runs_only_the_branch_it_takes() {
        runs(&[
            (
                "if 1 < 2 { :yes } else { 1 / 0 }\nif false { 1 / 0 } else { :no }",
                ":yes\n:no",
            ),
            (
                "let n = 0\nif n < 0 { :neg } else if n == 0 { :zero } else { :pos }",
                ":zero",
            ),
            ("1 + if true { 2 } else { 3 }", "3"),
            // Locals inside a branch sit above the outer ones and the
            // condition is gone by then.
            (
                "let x = 10\n{\n  let a = 1\n  if a > 0 {\n    let b = a + x\n    b * 2\n  } else {\n    0\n  }\n}",
                "22",
            ),
            (
                "if 1 { 2 } else { 3 }",
                "[line 1] error: the condition of `if` must be a Bool, got Int",
            ),
        ]);
    }

    #[test]
    fn and_and_or_short_circuit_on_bools() {
        runs(&[
            (
                "true and false\nfalse or true\n1 < 2 and 2 < 3 or false",
                "false\ntrue\ntrue",
            ),
            ("false and 1 / 0\ntrue or 1 / 0", "false\ntrue"),
            (
                "1 and true\ntrue and 1\nfalse or :no",
                "[line 1] error: operands of `and` must be Bool, got Int\n\
                 [line 2] error: operands of `and` must be Bool, got Int\n\
                 [line 3] error: operands of `or` must be Bool, got Symbol",
            ),
        ]);
    }
}
