use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp};
use crate::chunk::{Function, Op, Shape, Test};
use crate::interpreter::{
    binary, check_arity, condition, error, field, logical, unary, RuntimeError, MAX_CALL_DEPTH,
};
use crate::prelude::NATIVES;
use crate::value::{Caller, Instance, Value};

/// A call in progress: the function, the next instruction it runs and where
/// its frame starts on the stack. clox keeps the function in slot 0; here,
/// as in ploxb, the frame starts at the first argument and the function sits
/// right below it, since ludo has no methods that would need it there.
struct CallFrame {
    function: Rc<Function>,
    ip: usize,
    base: usize,
}

/// A stack machine that runs compiled functions. The operators share their
/// meaning with the tree-walking interpreter, so both give the same values
/// and errors.
pub struct Vm {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    /// Looked up by name each time they are used, as in clox, so a reloaded
    /// game sees the new definitions.
    globals: HashMap<String, Value>,
    /// The fields of each declared struct, in order, looked up by name like
    /// the globals.
    structs: HashMap<String, Vec<String>>,

    pub trace: bool,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        let globals = NATIVES
            .iter()
            .map(|native| (native.name.to_string(), Value::Native(*native)))
            .collect();
        Vm {
            stack: Vec::new(),
            frames: Vec::new(),
            globals,
            structs: HashMap::new(),
            trace: false,
        }
    }

    /// Runs a compiled top-level statement and returns the value it left,
    /// if any.
    pub fn run(&mut self, script: Function) -> Result<Option<Value>, RuntimeError> {
        self.stack.clear();
        self.frames.clear();
        // As in clox, the script sits on the stack like a called function.
        let script = Rc::new(script);
        self.push(Value::Compiled(Rc::clone(&script)));
        self.frames.push(CallFrame {
            function: script,
            ip: 0,
            base: 1,
        });
        self.execute(0)
    }

    /// Runs instructions until the frame that sits on top of `entry` others
    /// returns, and gives back its result. A top-level statement runs from no
    /// frames; a native that calls back into ludo runs from wherever the VM
    /// was.
    fn execute(&mut self, entry: usize) -> Result<Option<Value>, RuntimeError> {
        loop {
            let frame = self.frame();
            let function = Rc::clone(&frame.function);
            let (ip, base) = (frame.ip, frame.base);
            frame.ip += 1;
            let chunk = &function.chunk;

            if self.trace {
                let indent = "    ".repeat(self.frames.len() - 1);
                let stack: Vec<String> = self.stack.iter().map(|v| format!("[ {v} ]")).collect();
                println!("{indent}          {}", stack.join(""));
                println!("{indent}{}", chunk.disassemble_op(ip));
            }

            let result = match chunk.code[ip] {
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
                        None if self.structs.contains_key(name) => Err(error(format!(
                            "`{name}` is a struct, not a value: build one with `{name} {{ ... }}`"
                        ))),
                        None => Err(error(format!("undefined variable `{name}`"))),
                    }
                }
                Op::GetLocal(slot) => {
                    let value = self.stack[base + slot].clone();
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
                    self.frame().ip += distance;
                    Ok(())
                }
                Op::JumpIfFalse(distance) => {
                    if *self.peek() == Value::Bool(false) {
                        self.frame().ip += distance;
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
                Op::List(count) => {
                    let items = self.stack.split_off(self.stack.len() - count);
                    self.push(Value::List(Rc::new(items)));
                    Ok(())
                }
                Op::DefineStruct(shape) => {
                    let Shape { name, fields } = chunk.shapes[shape].clone();
                    self.structs.insert(name, fields);
                    Ok(())
                }
                Op::Struct { shape, base } => {
                    let shape = &chunk.shapes[shape];
                    let values = self.stack.split_off(self.stack.len() - shape.fields.len());
                    let base = base.then(|| self.pop());
                    self.build_struct(shape, base, values)
                        .map(|instance| self.push(instance))
                }
                Op::GetField(index) => {
                    let target = self.pop();
                    field(&target, chunk.name(index)).map(|value| self.push(value))
                }
                Op::Call(argc) => {
                    let callee = self.stack[self.stack.len() - 1 - argc].clone();
                    self.call_value(callee, argc)
                }
                Op::Return => {
                    let frame = self.frames.pop().expect("a frame runs until it returns");
                    // ploxb checks that the stack ends as the compiler counted
                    // it: a function leaves just its result above its
                    // parameters, and a script its value or nothing.
                    let above = self.stack.len() - frame.base - frame.function.arity;
                    debug_assert!(
                        above == 1 || (above == 0 && frame.function.name.is_none()),
                        "inconsistent stack height: {above} values above the parameters of {}",
                        frame.function
                    );
                    let result = (above == 1).then(|| self.pop());
                    self.stack.truncate(frame.base - 1);
                    if self.frames.len() == entry {
                        return Ok(result);
                    }
                    self.push(result.expect("a function always returns a value"));
                    Ok(())
                }
            };
            // An error from deeper calls keeps the line it already has.
            result.map_err(|e| e.at(chunk.lines[ip]))?;
        }
    }

    /// Calls the value below the `argc` arguments on top of the stack, as
    /// clox's `callValue`. A compiled function gets a frame that runs next;
    /// a native runs right away and leaves its result in place of the call.
    fn call_value(&mut self, callee: Value, argc: usize) -> Result<(), RuntimeError> {
        match callee {
            Value::Compiled(function) => {
                check_arity(function.name(), function.arity, argc)?;
                // The script's frame sits under the calls, so this allows as
                // many nested calls as the interpreter does.
                if self.frames.len() > MAX_CALL_DEPTH {
                    return Err(error(format!(
                        "stack overflow: more than {MAX_CALL_DEPTH} nested calls"
                    )));
                }
                let base = self.stack.len() - argc;
                self.frames.push(CallFrame {
                    function,
                    ip: 0,
                    base,
                });
                Ok(())
            }
            Value::Native(native) => {
                check_arity(native.name, native.arity, argc)?;
                let args = self.stack.split_off(self.stack.len() - argc);
                self.pop();
                let result = (native.fun)(self, &args)?;
                self.push(result);
                Ok(())
            }
            other => Err(error(format!(
                "can only call functions, got {}",
                other.type_name()
            ))),
        }
    }

    /// Builds a struct from a literal, with the same checks and errors as the
    /// interpreter. They run once every value is on the stack, so when a
    /// literal has two errors, like an unknown field whose value fails, the
    /// VM reports the value first and the interpreter the field.
    fn build_struct(
        &self,
        literal: &Shape,
        base: Option<Value>,
        values: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let name = &literal.name;
        let declared = self
            .structs
            .get(name)
            .ok_or_else(|| error(format!("undefined struct `{name}`")))?;

        let mut slots: Vec<Option<Value>> = match base {
            Some(Value::Struct(instance)) if instance.name == *name => instance
                .fields
                .iter()
                .map(|(_, value)| Some(value.clone()))
                .collect(),
            Some(other) => {
                return Err(error(format!(
                    "`{name} {{ x | ... }}` needs a {name} as `x`, got {}",
                    other.type_name()
                )))
            }
            None => vec![None; declared.len()],
        };

        for (field, value) in literal.fields.iter().zip(values) {
            let index = declared
                .iter()
                .position(|f| f == field)
                .ok_or_else(|| error(format!("`{name}` has no field `{field}`")))?;
            slots[index] = Some(value);
        }

        let fields = declared
            .iter()
            .zip(slots)
            .map(|(field, value)| {
                let value =
                    value.ok_or_else(|| error(format!("missing field `{field}` in `{name}`")))?;
                Ok((field.clone(), value))
            })
            .collect::<Result<_, RuntimeError>>()?;
        Ok(Value::Struct(Rc::new(Instance {
            name: name.clone(),
            fields,
        })))
    }

    fn frame(&mut self) -> &mut CallFrame {
        self.frames
            .last_mut()
            .expect("the VM only runs inside a frame")
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

/// How a native like `map` calls back into ludo: the call goes on the stack
/// as if the bytecode made it, and the VM runs until it returns.
impl Caller for Vm {
    fn call(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let entry = self.frames.len();
        let argc = args.len();
        self.push(callee.clone());
        self.stack.extend(args);
        self.call_value(callee, argc)?;
        if self.frames.len() == entry {
            // A native already left its result.
            return Ok(self.pop());
        }
        Ok(self
            .execute(entry)?
            .expect("a function always returns a value"))
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
            let script = compile(stmt).unwrap_or_else(|e| panic!("{e}"));
            let shown = show(vm.run(script));
            // A function is a tree in one backend and bytecode in the other,
            // so they are compared as they are shown.
            assert_eq!(
                shown,
                show(interpreter.execute(stmt)),
                "the VM and the interpreter disagree on `{stmt}`"
            );
            lines.extend(shown);
        }
        lines.join("\n")
    }

    fn show(result: Result<Option<Value>, RuntimeError>) -> Option<String> {
        match result {
            Ok(value) => value.map(|v| v.to_string()),
            Err(e) => Some(format!("[line {}] error: {}", e.line.unwrap(), e.message)),
        }
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

    #[test]
    fn calls_functions() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        let parity = "is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }\n\
                      is_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }\n";
        runs(&[
            (
                &format!("{add}add(2, 3)\n2 |> add(3)\n2.add(3)\n1 + add(2, 3) * 2"),
                "5\n5\n5\n11",
            ),
            (&format!("{parity}is_even(10)\nis_odd(7)"), "true\ntrue"),
            // Globals are looked up when the function runs.
            (
                "scaled :: (x: Int) -> Int { x * SCALE }\nlet SCALE = 10\nscaled(3)",
                "30",
            ),
            (
                "fact :: (n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }\nfact(10)",
                "3628800",
            ),
            // A function's locals sit above its parameters, and it does not
            // see the locals of its caller.
            (
                "f :: (x: Int) -> Int {\n  let y = x * 2\n  y + 1\n}\n{\n  let a = 3\n  f(a) + a\n}",
                "10",
            ),
            (
                "peek :: () -> Int { secret }\n{\n  let secret = 1\n  peek()\n}",
                "[line 1] error: undefined variable `secret`",
            ),
            ("let x = 100\nid :: (x: Int) -> Int { x }\nid(1)\nx", "1\n100"),
            (
                "double :: (x: Int) -> Int { x * 2 }\ntwice :: (f: Fn, x: Int) -> Int { f(f(x)) }\ntwice(double, 3)\ndouble",
                "12\n<fn double>",
            ),
        ]);
    }

    #[test]
    fn reports_errors_in_calls() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                &format!("{add}id :: (x: Int) -> Int {{ x }}\nadd(1)\nid(1, 2)\nlet n = 1\nn(2)\nid == id"),
                "[line 3] error: `add` expects 2 arguments, got 1\n\
                 [line 4] error: `id` expects 1 argument, got 2\n\
                 [line 6] error: can only call functions, got Int\n\
                 [line 7] error: functions cannot be compared",
            ),
            // An error inside a function keeps its own line, even when it
            // comes back through a native.
            (
                "boom :: (x: Int) -> Int {\n  1 / x\n}\nboom(0)\nrange(0, 1) |> map(boom)",
                "[line 2] error: division by zero\n[line 2] error: division by zero",
            ),
            (
                "loop_forever :: (n: Int) -> Int { loop_forever(n + 1) }\nloop_forever(0)",
                "[line 1] error: stack overflow: more than 200 nested calls",
            ),
        ]);
    }

    #[test]
    fn natives_call_back_into_ludo() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\n\
                   even :: (x: Int) -> Bool { x % 2 == 0 }\n\
                   add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                "abs(-3)\nmin(2.0, 1.5)\nclamp(5, 0, 3)\nto_string(3) + \"!\"\n3 |> print |> abs",
                "3\n1.5\n3\n3!\n3",
            ),
            ("abs\nlet abs = 1\nabs + 1", "<fn abs>\n2"),
            (
                &format!("{fns}range(0, 10) |> filter(even) |> map(double) |> fold(0, add)\nrange(0, 3) |> map(double)"),
                "40\n[0, 2, 4]",
            ),
            (&format!("{fns}range(1, 1001) |> fold(0, add)"), "500500"),
            (
                &format!("{fns}range(0, 3) |> filter(double)\nrange(0, 3) |> map(add)\nabs(true)\nmin(1)"),
                "[line 4] error: `filter` expects a function that returns a Bool, got Int\n\
                 [line 5] error: `add` expects 2 arguments, got 1\n\
                 [line 6] error: `abs` expects an Int or a Float, got Bool\n\
                 [line 7] error: `min` expects 2 arguments, got 1",
            ),
        ]);
    }

    #[test]
    fn builds_lists() {
        runs(&[
            (
                "[]\n[1, 2 + 3]\n[[1], [2, 3]]\n[1, 2] == [1, 2]",
                "[]\n[1, 5]\n[[1], [2, 3]]\ntrue",
            ),
            (
                "{\n  let a = 1\n  [a, a + 1] |> concat([:b, \"c\"])\n}",
                "[1, 2, :b, \"c\"]",
            ),
            (
                "[-1, 2] |> map(abs)\n[:a, :b] |> contains(:b)",
                "[1, 2]\ntrue",
            ),
        ]);
    }

    #[test]
    fn builds_reads_and_updates_structs() {
        let ball = "Ball :: struct { x: Float, y: Float }\n";
        runs(&[
            (
                &format!("{ball}let b = Ball {{ y: 2.0, x: 1.0 }}\nb\nb.y\nBall {{ b | y: 5.0 }}\nb == Ball {{ x: 1.0, y: 2.0 }}"),
                "Ball { x: 1.0, y: 2.0 }\n2.0\nBall { x: 1.0, y: 5.0 }\ntrue",
            ),
            (
                "Label :: struct { text: String }\nLabel { text: \"hi\" }\nEmpty :: struct {}\nEmpty {}",
                "Label { text: \"hi\" }\nEmpty {}",
            ),
            // A literal looks its struct up when it runs, so a function can
            // build a struct declared after it.
            (
                "origin :: () -> P { P { x: 0, y: 0 } }\nP :: struct { x: Int, y: Int }\norigin()",
                "P { x: 0, y: 0 }",
            ),
            // Nested structs read and updated inside a function, as games do.
            (
                &format!(
                    "{ball}World :: struct {{ ball: Ball, score: Int }}\n\
                     bounce :: (w: World) -> World {{\n  let b = w.ball\n  World {{ w | ball: Ball {{ b | x: -b.x }}, score: w.score + 1 }}\n}}\n\
                     let w = World {{ ball: Ball {{ x: 3.0, y: 0.0 }}, score: 0 }}\nbounce(w)\nbounce(w).ball.x"
                ),
                "World { ball: Ball { x: -3.0, y: 0.0 }, score: 1 }\n-3.0",
            ),
        ]);
    }

    #[test]
    fn reports_errors_in_structs() {
        let ball = "Ball :: struct { x: Float, y: Float }\n";
        runs(&[(
            &format!(
                "{ball}Paddle :: struct {{ y: Float }}\nlet n = 1\n\
                 Ball {{ x: 1.0, y: 2.0 }} == Paddle {{ y: 2.0 }}\n\
                 Ball {{ x: 1.0 }}\n\
                 Ball {{ x: 1.0, y: 2.0, z: 3.0 }}\n\
                 Wall {{ y: 1.0 }}\n\
                 Ball {{ 1 | x: 2.0 }}\n\
                 n.x\n\
                 Ball {{ x: 1.0, y: 2.0 }}.z\n\
                 Ball"
            ),
            "[line 4] error: operands of `==` must be two values of the same type, got Ball and Paddle\n\
             [line 5] error: missing field `y` in `Ball`\n\
             [line 6] error: `Ball` has no field `z`\n\
             [line 7] error: undefined struct `Wall`\n\
             [line 8] error: `Ball { x | ... }` needs a Ball as `x`, got Int\n\
             [line 9] error: Int has no fields\n\
             [line 10] error: `Ball` has no field `z`\n\
             [line 11] error: `Ball` is a struct, not a value: build one with `Ball { ... }`",
        )]);
    }
}
