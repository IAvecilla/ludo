use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp};
use crate::chunk::{Function, Op, Shape, Test};
use crate::interpreter::{
    binary, check_arity, condition, error, field, logical, passes, unary, with_field, RuntimeError,
    MAX_CALL_DEPTH,
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

/// A sequence that is running: the next instruction it runs and the values
/// it keeps on the stack between frames of the game, from its state in slot
/// 0 to the counters of an `over` halfway through. It is how a coroutine
/// keeps its place.
struct Run {
    function: Rc<Function>,
    ip: usize,
    slots: Vec<Value>,
    done: bool,
}

/// How `execute` stops.
enum Exit {
    /// The frame it started from returned, with its result if it has one.
    Return(Option<Value>),
    /// A sequence waits for the next frame of the game.
    Yield,
    /// A sequence ran its last step.
    End,
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
    /// Looked up by name like the structs.
    sequences: HashMap<String, Rc<Function>>,
    /// The sequences that are running, in the order they were started.
    runs: Vec<Run>,
    /// While a sequence runs a frame: whether it already made a pass of an
    /// `over`, and how long the frame is.
    spent: bool,
    dt: f64,
    /// How many frames under the calls are not calls themselves: the script
    /// of a statement or a running sequence, which the interpreter does not
    /// count either, or none when the host calls a function directly.
    root: usize,

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
            sequences: HashMap::new(),
            runs: Vec::new(),
            spent: false,
            dt: 0.0,
            root: 0,
            trace: false,
        }
    }

    /// Runs a compiled top-level statement and returns the value it left,
    /// if any.
    pub fn run(&mut self, script: Function) -> Result<Option<Value>, RuntimeError> {
        self.stack.clear();
        self.frames.clear();
        // As in clox, the script sits on the stack like a called function.
        self.root = 1;
        let script = Rc::new(script);
        self.push(Value::Compiled(Rc::clone(&script)));
        self.frames.push(CallFrame {
            function: script,
            ip: 0,
            base: 1,
        });
        match self.execute(0)? {
            Exit::Return(result) => Ok(result),
            Exit::Yield | Exit::End => unreachable!("only sequences stop, from `run_sequences`"),
        }
    }

    /// Runs one frame of every sequence over the world, in the order they
    /// were started, each over what the one before left. Sequences started
    /// on the way join at the end, replacing any with the same name. It
    /// gives back the world as the last one left it.
    pub fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        let mut runs = std::mem::take(&mut self.runs);
        let mut world = world;
        let mut result = Ok(());
        for run in &mut runs {
            run.slots[0] = world;
            result = self.resume(run, dt);
            world = run.slots[0].clone();
            if result.is_err() {
                break;
            }
        }

        runs.retain(|run| !run.done);
        for run in std::mem::take(&mut self.runs) {
            replace(&mut runs, run);
        }
        self.runs = runs;
        result.map(|()| world)
    }

    /// Calls a global function from the host, as `play` does with `init`,
    /// `update` and `draw`.
    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let callee = self
            .globals
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined function `{name}`")))?;
        self.stack.clear();
        self.frames.clear();
        self.root = 0;
        self.call(callee, args)
    }

    /// How many arguments the global function with this name takes, if it
    /// is one.
    pub fn arity(&self, name: &str) -> Option<usize> {
        match self.globals.get(name)? {
            Value::Compiled(function) => Some(function.arity),
            Value::Native(native) => Some(native.arity),
            _ => None,
        }
    }

    /// The fields of every declared struct, for `play` to tell whether a
    /// reload changed the shape of the world.
    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        self.structs.clone()
    }

    /// Takes over the sequences running in `from`, the VM of the code before
    /// a reload, except those this one already started. They keep the code
    /// they started with, and look globals up by name in this VM.
    pub fn adopt_sequences(&mut self, from: &mut Vm) {
        let mut runs = std::mem::take(&mut from.runs);
        runs.retain(|run| {
            !self
                .runs
                .iter()
                .any(|own| own.function.name == run.function.name)
        });
        runs.append(&mut self.runs);
        self.runs = runs;
    }

    pub fn stop_sequences(&mut self) {
        self.runs.clear();
    }

    /// Puts a sequence back on the stack and runs it until it waits for the
    /// next frame or ends, then takes it off again. If it fails it stays as
    /// it was, and the next frame tries again from where it last stopped.
    fn resume(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        self.stack.clear();
        self.frames.clear();
        self.push(Value::Compiled(Rc::clone(&run.function)));
        self.stack.extend(run.slots.iter().cloned());
        self.frames.push(CallFrame {
            function: Rc::clone(&run.function),
            ip: run.ip,
            base: 1,
        });
        self.spent = false;
        self.dt = dt;
        self.root = 1;

        let exit = self.execute(0)?;
        let frame = self
            .frames
            .pop()
            .expect("a sequence stops in its own frame");
        run.ip = frame.ip;
        run.slots = self.stack.split_off(frame.base);
        run.done = matches!(exit, Exit::End);
        self.stack.clear();
        Ok(())
    }

    /// Starts a sequence for `start`, replacing one with the same name.
    fn start(&mut self, name: &str, args: Vec<Value>) -> Result<(), RuntimeError> {
        let function = self
            .sequences
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined sequence `{name}`")))?;
        check_arity(function.name(), function.arity, args.len())?;
        let run = Run {
            function,
            ip: 0,
            slots: args,
            done: false,
        };
        replace(&mut self.runs, run);
        Ok(())
    }

    /// The names of the sequences that are running, in the order they run.
    #[cfg(test)]
    pub(crate) fn running(&self) -> Vec<&str> {
        self.runs.iter().map(|run| run.function.name()).collect()
    }

    /// Runs instructions until the frame that sits on top of `entry` others
    /// returns, and gives back its result. A top-level statement runs from no
    /// frames; a native that calls back into ludo runs from wherever the VM
    /// was.
    fn execute(&mut self, entry: usize) -> Result<Exit, RuntimeError> {
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
                        None if self.sequences.contains_key(name) => Err(error(format!(
                            "`{name}` is a sequence, not a function: it runs over time and cannot be called"
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
                Op::DefineSequence(index) => {
                    let Value::Compiled(sequence) = self.pop() else {
                        unreachable!("a sequence compiles to a function")
                    };
                    self.sequences
                        .insert(chunk.name(index).to_string(), sequence);
                    Ok(())
                }
                Op::Start { name, argc } => {
                    let args = self.stack.split_off(self.stack.len() - argc);
                    self.start(chunk.name(name), args)
                }
                // Running the same instruction again next frame is what
                // makes it wait.
                Op::Yield if self.spent => {
                    self.frame().ip = ip;
                    return Ok(Exit::Yield);
                }
                Op::Yield => Ok(()),
                Op::OverStart => {
                    let duration = self.pop();
                    passes(Ok(duration), self.dt).map(|passes| {
                        self.push(Value::Int(passes as i64));
                        self.push(Value::Int(0));
                    })
                }
                Op::OverPass(_) if self.spent => {
                    self.frame().ip = ip;
                    return Ok(Exit::Yield);
                }
                Op::OverPass(counters) => {
                    self.spent = true;
                    let (passes, pass) = self.counters(base + counters);
                    let pass = pass + 1;
                    self.stack[base + counters + 1] = Value::Int(pass);
                    let t = if pass == passes {
                        1.0
                    } else {
                        pass as f64 / passes as f64
                    };
                    self.push(Value::Float(t));
                    Ok(())
                }
                Op::OverLoop { counters, distance } => {
                    let (passes, pass) = self.counters(base + counters);
                    if pass < passes {
                        self.frame().ip -= distance;
                    }
                    Ok(())
                }
                Op::SetField { state, field } => {
                    let value = self.pop();
                    let target = chunk.name(state);
                    with_field(target, &self.stack[base], chunk.name(field), value)
                        .map(|updated| self.stack[base] = updated)
                }
                Op::SetState => {
                    self.stack[base] = self.pop();
                    Ok(())
                }
                Op::End => return Ok(Exit::End),
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
                        return Ok(Exit::Return(result));
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
                if self.frames.len() - self.root >= MAX_CALL_DEPTH {
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

    /// How many passes the `over` running makes and how many it made, from
    /// the two hidden locals that start at `slot`.
    fn counters(&self, slot: usize) -> (i64, i64) {
        match (&self.stack[slot], &self.stack[slot + 1]) {
            (Value::Int(passes), Value::Int(pass)) => (*passes, *pass),
            _ => unreachable!("`OverStart` leaves two Ints"),
        }
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
        match self.execute(entry)? {
            Exit::Return(Some(result)) => Ok(result),
            _ => unreachable!("a function returns a value, and never waits or ends"),
        }
    }
}

fn replace(runs: &mut Vec<Run>, run: Run) {
    runs.retain(|r| r.function.name != run.function.name);
    runs.push(run);
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

    const BALL: &str = "Ball :: struct { x: Float, y: Float }\n";

    fn ball(x: f64) -> Value {
        Value::Struct(Rc::new(Instance {
            name: "Ball".into(),
            fields: vec![
                ("x".into(), Value::Float(x)),
                ("y".into(), Value::Float(0.0)),
            ],
        }))
    }

    fn x(world: &Value) -> String {
        match world {
            Value::Struct(ball) => ball.fields[0].1.to_string(),
            other => other.to_string(),
        }
    }

    /// The VM and the interpreter side by side, to check they agree after
    /// every statement and every frame of the game.
    struct Both {
        vm: Vm,
        interpreter: Interpreter,
    }

    impl Both {
        /// Both backends with `Ball` and `source` loaded.
        fn new(source: &str) -> Self {
            let mut both = Both {
                vm: Vm::new(),
                interpreter: Interpreter::new(),
            };
            both.run(&format!("{BALL}{source}"));
            both
        }

        fn run(&mut self, source: &str) {
            for stmt in parse(scan_tokens(source).unwrap()).unwrap() {
                let ran = show(self.vm.run(compile(&stmt).unwrap()));
                assert_eq!(
                    ran,
                    show(self.interpreter.execute(&stmt)),
                    "the VM and the interpreter disagree on `{stmt}`"
                );
            }
        }

        /// Runs `count` frames of `dt`, the first over `world` and each next
        /// one over what the last left, as `play` does. Shows the `x` of the
        /// world after each frame, with `done` once no sequence is running,
        /// or the error a frame stops at.
        fn frames(&mut self, world: Value, dt: f64, count: usize) -> String {
            let mut world = world;
            let mut shown = Vec::new();
            for _ in 0..count {
                let result = self.vm.run_sequences(world.clone(), dt);
                assert_eq!(
                    result,
                    self.interpreter.run_sequences(world.clone(), dt),
                    "the VM and the interpreter disagree on a frame"
                );
                assert_eq!(self.vm.running(), self.interpreter.running());
                match result {
                    Ok(next) => {
                        let done = if self.vm.running().is_empty() {
                            " done"
                        } else {
                            ""
                        };
                        shown.push(format!("{}{done}", x(&next)));
                        world = next;
                    }
                    Err(e) => {
                        shown.push(format!("[line {}] error: {}", e.line.unwrap(), e.message));
                        break;
                    }
                }
            }
            shown.join(", ")
        }
    }

    #[test]
    fn wait_and_over_take_whole_frames() {
        let cases = [
            ("b.x = 1.0\n  b.x = b.x + 1.0", 1, "2.0 done"),
            ("wait 1.0s\n  b.x = 5.0", 4, "0.0, 0.0, 0.0, 5.0 done"),
            (
                "over 1.0s as t {\n    b.x = t\n  }",
                4,
                "0.25, 0.5, 0.75, 1.0 done",
            ),
            ("over 0.3s as t {\n    b.x = t\n  }", 2, "0.5, 1.0 done"),
            ("wait 0.0s\n  b.x = 1.0", 1, "1.0 done"),
            (
                "b.x = 1.0\n  wait 0.5s\n  b.x = 2.0\n  wait 0.25s\n  b.x = 3.0",
                3,
                "1.0, 2.0, 3.0 done",
            ),
            ("b.x = b.x + 1.0", 3, "1.0 done, 1.0 done, 1.0 done"),
            (
                "over 0.5s as t {\n    b.x = half(t) |> min(0.3)\n  }",
                2,
                "0.25, 0.3 done",
            ),
        ];
        for (body, count, expected) in cases {
            let mut both = Both::new(&format!(
                "half :: (x: Float) -> Float {{ x / 2.0 }}\ns :: sequence (b: Ball) {{\n  {body}\n}}\nstart s(Ball {{ x: 0.0, y: 0.0 }})"
            ));
            assert_eq!(both.frames(ball(0.0), 0.25, count), expected, "{body:?}");
        }

        let mut both = Both::new(
            "s :: sequence (b: Ball) {\n  over 0.1s as t {\n    b.x = t\n  }\n}\nstart s(Ball { x: 0.0, y: 0.0 })",
        );
        let at_60 = both.frames(ball(0.0), 1.0 / 60.0, 6);
        assert!(at_60.ends_with("0.8333333333333334, 1.0 done"), "{at_60}");

        // Locals of the sequence last between frames; those of the body of
        // `over` last a pass.
        let mut both = Both::new(
            "s :: sequence (b: Ball, to: Float) {\n  let from = b.x\n  over 0.5s as t {\n    let d = to - from\n    b.x = from + d * t\n  }\n}\nstart s(Ball { x: 10.0, y: 0.0 }, 20.0)",
        );
        assert_eq!(both.frames(ball(10.0), 0.25, 2), "15.0, 20.0 done");
    }

    #[test]
    fn running_sequences_step_over_the_current_world() {
        let mut both = Both::new(
            "push :: sequence (b: Ball, by: Float) {\n  wait 0.5s\n  b.x = b.x + by\n}\nkick :: (b: Ball) -> Ball {\n  start push(b, 10.0)\n  b\n}\nkick(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(both.vm.running(), ["push"]);
        assert_eq!(both.frames(ball(1.0), 0.25, 2), "1.0, 11.0 done");

        let mut both = Both::new(
            "s :: sequence (b: Ball) {\n  over 1.0s {\n    b.x = b.x + 1.0\n  }\n}\nstart s(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(
            both.frames(ball(100.0), 0.25, 4),
            "101.0, 102.0, 103.0, 104.0 done"
        );

        let mut both = Both::new(
            "double :: sequence (b: Ball) {\n  b.x = b.x * 2.0\n}\ninc :: sequence (b: Ball) {\n  b.x = b.x + 1.0\n}\nstart double(Ball { x: 0.0, y: 0.0 })\nstart inc(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(both.frames(ball(3.0), 0.25, 1), "7.0 done");

        // A sequence started by another one joins after it, from the next
        // frame on.
        let mut both = Both::new(
            "second :: sequence (b: Ball) {\n  b.x = b.x + 10.0\n}\nfirst :: sequence (b: Ball) {\n  b.x = 1.0\n  start second(b)\n}\nstart first(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(both.frames(ball(0.0), 0.25, 2), "1.0, 11.0 done");
    }

    #[test]
    fn the_duration_is_read_on_the_frame_of_the_first_pass() {
        // `grow` runs first, so `pause` reads `b.x` for its second `over`
        // on the next frame, 0.5 and two passes, not on the first, 0.25.
        let mut both = Both::new(
            "grow :: sequence (b: Ball) {\n  over 1.0s {\n    b.x = b.x + 0.25\n  }\n}\n\
             pause :: sequence (b: Ball) {\n  wait 0.25s\n  over b.x {}\n  b.x = 100.0\n}\n\
             start grow(Ball { x: 0.0, y: 0.0 })\nstart pause(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(
            both.frames(ball(0.0), 0.25, 4),
            "0.25, 0.5, 100.0, 100.25 done"
        );
    }

    #[test]
    fn starting_a_running_sequence_replaces_it() {
        let mut both = Both::new(
            "mark :: sequence (b: Ball, to: Float) {\n  wait 0.5s\n  b.x = to\n}\nstart mark(Ball { x: 0.0, y: 0.0 }, 1.0)",
        );
        assert_eq!(both.frames(ball(0.0), 0.25, 1), "0.0");
        both.run("start mark(Ball { x: 0.0, y: 0.0 }, 2.0)");
        assert_eq!(both.vm.running(), ["mark"]);
        assert_eq!(both.frames(ball(0.0), 0.25, 2), "0.0, 2.0 done");
    }

    #[test]
    fn binding_the_state_again_replaces_it() {
        let mut both = Both::new(
            "s :: sequence (b: Ball) {\n  let b = Ball { x: 5.0, y: 0.0 }\n  b.x = b.x + 1.0\n  over 0.5s {\n    let b = Ball { b | x: b.x * 2.0 }\n  }\n}\nstart s(Ball { x: 0.0, y: 0.0 })",
        );
        assert_eq!(both.frames(ball(0.0), 0.25, 2), "12.0, 24.0 done");
    }

    #[test]
    fn reports_errors_in_sequences() {
        let cases = [
            (
                "b: Ball",
                "wait 1",
                ball(0.0),
                "a duration must be a Float in seconds, got Int",
            ),
            (
                "b: Ball",
                "wait -1.0s",
                ball(0.0),
                "a duration must be a finite number of seconds, at least 0s, got -1.0",
            ),
            ("b: Ball", "b.z = 1.0", ball(0.0), "`Ball` has no field `z`"),
            (
                "b: Int",
                "b.z = 1.0",
                Value::Int(1),
                "`b.z = ...` needs `b` to be a struct, got Int",
            ),
        ];
        for (param, body, world, message) in cases {
            let mut both = Both::new(&format!(
                "s :: sequence ({param}) {{\n  {body}\n}}\nstart s(1)"
            ));
            assert_eq!(
                both.frames(world, 0.25, 1),
                format!("[line 3] error: {message}")
            );
        }

        assert_eq!(
            outputs("s :: sequence (n: Int) {}\nstart s()\nstart t(1)\ns(1)"),
            "[line 2] error: `s` expects 1 argument, got 0\n\
             [line 3] error: undefined sequence `t`\n\
             [line 4] error: `s` is a sequence, not a function: it runs over time and cannot be called"
        );
    }

    #[test]
    fn a_sequence_keeps_only_its_own_slots_between_frames() {
        let mut both = Both::new(
            "drift :: sequence (b: Ball) {\n  let from = b.x\n  over 10.0s {\n    b.x = b.x + 1.0\n  }\n}\nstart drift(Ball { x: 0.0, y: 0.0 })",
        );
        both.frames(ball(0.0), 0.25, 20);
        // The state, `from` and the two counters of the `over` halfway through.
        assert_eq!(both.vm.runs[0].slots.len(), 4);
        assert!(both.vm.stack.is_empty() && both.vm.frames.is_empty());
    }

    /// Runs `test` on a thread with room for the interpreter's 200 nested
    /// calls, which in a debug build need more than the 2 MiB a test thread
    /// gets. The VM keeps its calls in a `Vec` and needs no room.
    fn with_room(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(test)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn the_host_calls_functions() {
        with_room(the_host_calls_functions_with_room);
    }

    fn the_host_calls_functions_with_room() {
        let mut both = Both::new(
            "add :: (a: Int, b: Int) -> Int { a + b }\nlet n = 1\n\
             count :: (n: Int) -> Int { if n == 0 { 0 } else { 1 + count(n - 1) } }",
        );
        let mut call = |name: &str, args: Vec<Value>| {
            let result = both.vm.call_function(name, args.clone());
            assert_eq!(result, both.interpreter.call_function(name, args), "{name}");
            result.map_err(|e| e.message)
        };
        assert_eq!(
            call("add", vec![Value::Int(2), Value::Int(3)]),
            Ok(Value::Int(5))
        );
        assert_eq!(call("abs", vec![Value::Int(-4)]), Ok(Value::Int(4)));
        assert_eq!(
            call("n", vec![]),
            Err("can only call functions, got Int".into())
        );
        assert_eq!(
            call("nope", vec![]),
            Err("undefined function `nope`".into())
        );
        // With no script below, the host's call is the first nested one.
        assert_eq!(call("count", vec![Value::Int(199)]), Ok(Value::Int(199)));
        assert_eq!(
            call("count", vec![Value::Int(200)]),
            Err("stack overflow: more than 200 nested calls".into())
        );

        for name in ["add", "abs", "n", "nope"] {
            assert_eq!(both.vm.arity(name), both.interpreter.arity(name), "{name}");
        }
        assert_eq!(both.vm.arity("add"), Some(2));
        assert_eq!(both.vm.struct_shapes(), both.interpreter.struct_shapes());
    }

    #[test]
    fn a_reload_keeps_running_sequences_with_their_code_and_the_new_globals() {
        let source = |step: &str, times: &str| {
            format!(
                "step :: () -> Float {{ {step} }}\n\
                 s :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + step() * {times}\n  }}\n}}\n"
            )
        };
        let mut old = Both::new(&format!(
            "{}start s(Ball {{ x: 0.0, y: 0.0 }})",
            source("1.0", "1.0")
        ));
        assert_eq!(old.frames(ball(0.0), 0.25, 1), "1.0");

        // The running `s` keeps adding `step()` once, as its old code says,
        // but `step` is the new one.
        let mut new = Both::new(&source("10.0", "2.0"));
        new.vm.adopt_sequences(&mut old.vm);
        new.interpreter.adopt_sequences(&mut old.interpreter);
        assert!(old.vm.running().is_empty());
        assert_eq!(new.frames(ball(1.0), 0.25, 3), "11.0, 21.0, 31.0 done");

        new.run("start s(Ball { x: 0.0, y: 0.0 })");
        new.vm.stop_sequences();
        new.interpreter.stop_sequences();
        assert_eq!(new.frames(ball(0.0), 0.25, 1), "0.0 done");
    }
}
