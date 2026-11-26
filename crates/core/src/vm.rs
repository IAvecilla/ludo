use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Stmt, UnaryOp};
use crate::chunk::{Chunk, Function, Op, Shape};
use crate::compiler::compile;
use crate::prelude::globals;
use crate::runtime::{
    binary, build_struct, check_arity, check_bool, contents, enum_hint, error, field, matches,
    no_match, not_callable, passes, progress, sequence_hint, stack_overflow, struct_hint, unary,
    with_field, RuntimeError, MAX_CALL_DEPTH,
};
use crate::value::{Closure, Value};

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
pub struct Run {
    function: Rc<Function>,
    ip: usize,
    slots: Vec<Value>,
    done: bool,
}

impl Run {
    fn name(&self) -> &str {
        self.function.name()
    }
}

/// What a program declares besides its globals, looked up by name when it
/// runs, so a function can use what is declared after it: the fields of its
/// structs, the symbols of its enums and its sequences.
#[derive(Default)]
struct Declarations {
    structs: HashMap<String, Rc<[String]>>,
    enums: HashMap<String, Vec<String>>,
    sequences: HashMap<String, Rc<Function>>,
}

impl Declarations {
    fn fields(&self, name: &str) -> Result<&Rc<[String]>, RuntimeError> {
        self.structs
            .get(name)
            .ok_or_else(|| error(format!("undefined struct `{name}`")))
    }

    /// The error for a name that is not a variable, which says what it is if
    /// it is declared as something else.
    fn undefined(&self, name: &str) -> RuntimeError {
        error(if self.structs.contains_key(name) {
            struct_hint(name)
        } else if self.sequences.contains_key(name) {
            sequence_hint(name)
        } else if let Some(symbols) = self.enums.get(name) {
            enum_hint(name, &symbols[0])
        } else {
            format!("undefined variable `{name}`")
        })
    }
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

/// A stack machine that runs compiled functions.
pub struct Vm {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    /// Looked up by name each time they are used, as in clox, so a reloaded
    /// game sees the new definitions.
    globals: HashMap<String, Value>,
    declared: Declarations,
    runs: Vec<Run>,
    /// While a sequence runs a frame: whether it already made a pass of an
    /// `over`, and how long the frame is.
    spent: bool,
    dt: f64,
    /// How many frames under the calls are not calls themselves: the script
    /// of a statement or a running sequence, or none when the host calls a
    /// function directly.
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
        Vm {
            stack: Vec::new(),
            frames: Vec::new(),
            globals: globals(),
            declared: Declarations::default(),
            runs: Vec::new(),
            spent: false,
            dt: 0.0,
            root: 0,
            trace: false,
        }
    }

    /// Compiles a top-level statement and runs it, as clox's `interpret`,
    /// giving the value it left, if any.
    pub fn interpret(&mut self, stmt: &Stmt) -> Result<Option<Value>, RuntimeError> {
        self.run(compile(stmt))
    }

    /// Runs a compiled top-level statement and returns the value it left,
    /// if any.
    pub fn run(&mut self, script: Function) -> Result<Option<Value>, RuntimeError> {
        self.enter(Rc::new(script), 0, &[]);
        match self.execute(0)? {
            Exit::Return(result) => Ok(result),
            Exit::Yield | Exit::End => unreachable!("only sequences stop, from `run_sequences`"),
        }
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
            Value::Function(function) => Some(function.arity),
            Value::Closure(closure) => Some(closure.function.arity),
            Value::Native(native) => Some(native.arity),
            _ => None,
        }
    }

    /// The fields of every declared struct, for `play` to tell whether a
    /// reload changed the shape of the world.
    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        let shapes = self.declared.structs.iter();
        shapes.map(|(name, f)| (name.clone(), f.to_vec())).collect()
    }

    /// Runs one frame of every sequence over the world, in the order they
    /// were started, each over what the one before left. Sequences started
    /// on the way join at the end. It gives back the world as the last one
    /// left it; after an error, the rest wait for the next frame.
    pub fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        let mut runs = std::mem::take(&mut self.runs);
        let mut result = Ok(world);
        for run in &mut runs {
            result = result.and_then(|world| self.resume(run, world, dt));
        }
        runs.retain(|run| !run.done);
        let started = std::mem::replace(&mut self.runs, runs);
        for run in started {
            self.launch(run);
        }
        result
    }

    /// Takes over the sequences running in `from`, the code before a reload,
    /// except those this one already started. They keep the code they
    /// started with, and look globals up by name here.
    pub fn adopt_sequences(&mut self, from: &mut Vm) {
        let mut runs = std::mem::take(&mut from.runs);
        runs.retain(|run| !self.runs.iter().any(|r| r.name() == run.name()));
        runs.append(&mut self.runs);
        self.runs = runs;
    }

    pub fn stop_sequences(&mut self) {
        self.runs.clear();
    }

    /// Starts a sequence, replacing one with the same name.
    fn launch(&mut self, run: Run) {
        self.runs.retain(|r| r.name() != run.name());
        self.runs.push(run);
    }

    /// Puts a sequence back on the stack and runs it until it waits for the
    /// next frame or ends, then takes it off again. If it fails it stays as
    /// it was, and the next frame tries again from where it last stopped.
    fn resume(&mut self, run: &mut Run, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        run.slots[0] = world;
        self.enter(Rc::clone(&run.function), run.ip, &run.slots);
        self.spent = false;
        self.dt = dt;

        let exit = self.execute(0)?;
        let frame = self
            .frames
            .pop()
            .expect("a sequence stops in its own frame");
        run.ip = frame.ip;
        run.slots = self.stack.split_off(frame.base);
        run.done = matches!(exit, Exit::End);
        self.stack.clear();
        Ok(run.slots[0].clone())
    }

    /// Clears the VM and puts `function` on the stack as clox does with the
    /// script, with `slots` above it, to run from `ip`.
    fn enter(&mut self, function: Rc<Function>, ip: usize, slots: &[Value]) {
        self.stack.clear();
        self.frames.clear();
        self.root = 1;
        self.stack.push(Value::Function(Rc::clone(&function)));
        self.stack.extend_from_slice(slots);
        self.frames.push(CallFrame {
            function,
            ip,
            base: 1,
        });
    }

    /// Starts a sequence for `start`.
    fn start(&mut self, name: &str, args: Vec<Value>) -> Result<(), RuntimeError> {
        let function = self
            .declared
            .sequences
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined sequence `{name}`")))?;
        check_arity(function.name(), function.arity, args.len())?;
        self.launch(Run {
            function,
            ip: 0,
            slots: args,
            done: false,
        });
        Ok(())
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

            match self.instruction(chunk, ip, base, entry) {
                Ok(None) => {}
                Ok(Some(exit)) => return Ok(exit),
                // An error from deeper calls keeps the line it already has.
                Err(e) => return Err(e.at(chunk.lines[ip])),
            }
        }
    }

    /// Runs the instruction at `ip`, of the frame whose slots start at
    /// `base`, and says whether `execute` stops there.
    fn instruction(
        &mut self,
        chunk: &Chunk,
        ip: usize,
        base: usize,
        entry: usize,
    ) -> Result<Option<Exit>, RuntimeError> {
        match chunk.code[ip] {
            Op::Constant(index) => self.push(chunk.constants[index].clone()),
            Op::True => self.push(Value::Bool(true)),
            Op::False => self.push(Value::Bool(false)),
            Op::DefineGlobal(index) => {
                let value = self.pop();
                self.globals.insert(chunk.name(index).to_string(), value);
            }
            Op::GetGlobal(index) => {
                let name = chunk.name(index);
                match self.globals.get(name) {
                    Some(value) => self.push(value.clone()),
                    None => return Err(self.declared.undefined(name)),
                }
            }
            Op::GetLocal(slot) => self.push(self.stack[base + slot].clone()),
            Op::Pop => {
                self.pop();
            }
            Op::PopBelow(count) => {
                let top = self.pop();
                self.stack.truncate(self.stack.len() - count);
                self.push(top);
            }
            Op::CheckBool(test) => {
                check_bool(test, self.peek())?;
            }
            Op::Jump(distance) => self.frame().ip += distance,
            Op::JumpIfFalse(distance) => {
                if *self.peek() == Value::Bool(false) {
                    self.frame().ip += distance;
                }
            }
            Op::Negate => {
                let right = self.pop();
                self.push(unary(UnaryOp::Neg, right)?);
            }
            Op::Not => {
                let right = self.pop();
                self.push(unary(UnaryOp::Not, right)?);
            }
            Op::Binary(op) => {
                let right = self.pop();
                let left = self.pop();
                self.push(binary(op, left, right)?);
            }
            Op::List(count) => {
                let items = self.stack.split_off(self.stack.len() - count);
                self.push(Value::List(Rc::new(items)));
            }
            Op::DefineStruct(shape) => {
                let Shape { name, fields } = &chunk.shapes[shape];
                let fields = Rc::from(fields.as_slice());
                self.declared.structs.insert(name.clone(), fields);
            }
            Op::Struct { shape, base } => {
                let Shape { name, fields } = &chunk.shapes[shape];
                let values = self.stack.split_off(self.stack.len() - fields.len());
                let base = base.then(|| self.pop());
                let declared = self.declared.fields(name)?;
                let given = fields.iter().map(String::as_str).zip(values);
                let instance = build_struct(name, declared, base, given)?;
                self.push(instance);
            }
            Op::DefineEnum(shape) => {
                let Shape { name, fields } = chunk.shapes[shape].clone();
                self.declared.enums.insert(name, fields);
            }
            Op::MatchConstant(index) => {
                let value = self.pop();
                self.push(Value::Bool(matches(&value, &chunk.constants[index])));
            }
            Op::NoMatch => return Err(no_match(self.peek())),
            Op::MatchVariant(variant) => {
                let value = self.pop();
                let matched = contents(variant, &value);
                if let Some(Some(held)) = &matched {
                    self.push(held.clone());
                }
                self.push(Value::Bool(matched.is_some()));
            }
            Op::GetField(index) => {
                let target = self.pop();
                self.push(field(&target, chunk.name(index))?);
            }
            Op::DefineSequence(index) => {
                let Value::Function(sequence) = self.pop() else {
                    unreachable!("a sequence compiles to a function")
                };
                let name = chunk.name(index).to_string();
                self.declared.sequences.insert(name, sequence);
            }
            Op::Start { name, argc } => {
                let args = self.stack.split_off(self.stack.len() - argc);
                self.start(chunk.name(name), args)?;
            }
            // Running the same instruction again next frame is what makes it
            // wait.
            Op::Yield | Op::OverPass(_) if self.spent => {
                self.frame().ip = ip;
                return Ok(Some(Exit::Yield));
            }
            Op::Yield => {}
            Op::OverStart => {
                let duration = self.pop();
                let passes = passes(duration, self.dt)?;
                self.push(Value::Int(passes as i64));
                self.push(Value::Int(0));
            }
            Op::OverPass(counters) => {
                self.spent = true;
                let (passes, pass) = self.counters(base + counters);
                self.stack[base + counters + 1] = Value::Int(pass as i64 + 1);
                self.push(Value::Float(progress(pass + 1, passes)));
            }
            Op::OverLoop { counters, distance } => {
                let (passes, pass) = self.counters(base + counters);
                if pass < passes {
                    self.frame().ip -= distance;
                }
            }
            Op::SetField { state, field } => {
                let value = self.pop();
                let target = chunk.name(state);
                self.stack[base] = with_field(target, &self.stack[base], chunk.name(field), value)?;
            }
            Op::SetState => self.stack[base] = self.pop(),
            Op::End => return Ok(Some(Exit::End)),
            Op::Call(argc) => {
                let callee = self.stack[self.stack.len() - 1 - argc].clone();
                self.call_value(callee, argc)?;
            }
            Op::Closure(count) => {
                let captured = self.stack.split_off(self.stack.len() - count);
                let Value::Function(function) = self.pop() else {
                    unreachable!("a closure is made of a compiled function")
                };
                self.push(Value::Closure(Rc::new(Closure { function, captured })));
            }
            Op::Return => {
                let frame = self.frames.pop().expect("a frame runs until it returns");
                // ploxb checks that the stack ends as the compiler counted
                // it: a function leaves just its result above its
                // parameters, and a script its value or nothing.
                let function = &frame.function;
                let above =
                    self.stack.len() - frame.base - function.arity - function.captured.len();
                debug_assert!(
                    above == 1 || (above == 0 && function.name.is_none()),
                    "inconsistent stack height: {above} values above the parameters of {}",
                    frame.function
                );
                let result = (above == 1).then(|| self.pop());
                self.stack.truncate(frame.base - 1);
                if self.frames.len() == entry {
                    return Ok(Some(Exit::Return(result)));
                }
                self.push(result.expect("a function always returns a value"));
            }
        }
        Ok(None)
    }

    /// Calls the value below the `argc` arguments on top of the stack, as
    /// clox's `callValue`. A compiled function gets a frame that runs next;
    /// a native runs right away and leaves its result in place of the call.
    fn call_value(&mut self, callee: Value, argc: usize) -> Result<(), RuntimeError> {
        match callee {
            Value::Function(function) => {
                check_arity(function.name(), function.arity, argc)?;
                self.call_frame(function, argc, &[])?;
            }
            Value::Closure(closure) => {
                check_arity(closure.function.name(), closure.function.arity, argc)?;
                self.call_frame(Rc::clone(&closure.function), argc, &closure.captured)?;
            }
            Value::Native(native) => {
                check_arity(native.name, native.arity, argc)?;
                let args = self.stack.split_off(self.stack.len() - argc);
                self.pop();
                let result = (native.fun)(self, &args)?;
                self.push(result);
            }
            other => return Err(not_callable(&other)),
        }
        Ok(())
    }

    /// Gives a compiled function a frame that starts at its first argument.
    /// What a closure keeps goes on the stack right after the arguments, in
    /// the slots where its body reads them.
    fn call_frame(
        &mut self,
        function: Rc<Function>,
        argc: usize,
        captured: &[Value],
    ) -> Result<(), RuntimeError> {
        if self.frames.len() - self.root >= MAX_CALL_DEPTH {
            return Err(stack_overflow());
        }
        let base = self.stack.len() - argc;
        self.stack.extend_from_slice(captured);
        self.frames.push(CallFrame {
            function,
            ip: 0,
            base,
        });
        Ok(())
    }

    /// How many passes the `over` running makes and how many it made, from
    /// the two hidden locals that start at `slot`.
    fn counters(&self, slot: usize) -> (usize, usize) {
        match (&self.stack[slot], &self.stack[slot + 1]) {
            (Value::Int(passes), Value::Int(pass)) => (*passes as usize, *pass as usize),
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
}

impl Vm {
    /// How a native like `map` calls back into ludo: the call goes on the
    /// stack as if the bytecode made it, and the VM runs until it returns.
    pub(crate) fn call(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
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

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;
    use crate::value::Instance;

    /// A VM that runs ludo source as the REPL and `play` do.
    struct Machine(Vm);

    impl Machine {
        fn new(source: &str) -> Self {
            let mut machine = Machine(Vm::new());
            machine.run(source);
            machine
        }

        /// Runs `source` and shows what each statement gave, as the REPL
        /// does.
        fn run(&mut self, source: &str) -> String {
            let statements = parse(scan_tokens(source).unwrap()).unwrap();
            let shown = statements
                .iter()
                .filter_map(|stmt| match self.0.interpret(stmt) {
                    Ok(value) => value.map(|v| v.to_string()),
                    Err(e) => Some(error(&e)),
                });
            shown.collect::<Vec<_>>().join("\n")
        }

        fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
            self.0.call_function(name, args).map_err(|e| e.message)
        }

        /// Runs `count` frames of `dt`, the first over `world` and each next
        /// one over what the last left, as `play` does. Shows the `x` of the
        /// world after each frame, with `done` once no sequence is running,
        /// or the error a frame stops at.
        fn frames(&mut self, world: Value, dt: f64, count: usize) -> String {
            let mut world = world;
            let mut shown = Vec::new();
            for _ in 0..count {
                match self.0.run_sequences(world.clone(), dt) {
                    Ok(next) => {
                        let done = if self.0.runs.is_empty() { " done" } else { "" };
                        shown.push(format!("{}{done}", x(&next)));
                        world = next;
                    }
                    Err(e) => {
                        shown.push(error(&e));
                        break;
                    }
                }
            }
            shown.join(", ")
        }

        fn running(&self) -> Vec<&str> {
            self.0.runs.iter().map(Run::name).collect()
        }
    }

    fn error(e: &RuntimeError) -> String {
        format!("[line {}] error: {}", e.line.unwrap(), e.message)
    }

    fn runs(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(Machine::new("").run(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn evaluates_expressions() {
        runs(&[
            (
                "1 + 2 * 3\n7 / 2\n7 % 2\n7.0 / 2.0\n-(7 % 4)\n1.0 / 0.0\n\
                 1 < 2\n\"a\" + \"b\"\n:jump == :jump\n!true\nfalse and 1 / 0 == 1\ntrue or 1 / 0 == 1",
                "7\n3\n1\n3.5\n-3\ninf\ntrue\nab\ntrue\nfalse\nfalse\ntrue",
            ),
            (
                "1 + 2.0\n1 == 1.0\n1 / 0\n9223372036854775807 + 1\n-true\n\
                 1 and true\ntrue and 1\nfalse or :no\nx\nif 1 { 2 } else { 3 }",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Float\n\
                 [line 2] error: operands of `==` must be two values of the same type, got Int and Float\n\
                 [line 3] error: division by zero\n\
                 [line 4] error: integer overflow in `+`\n\
                 [line 5] error: operand of `-` must be an Int or a Float, got Bool\n\
                 [line 6] error: operands of `and` must be Bool, got Int\n\
                 [line 7] error: operands of `and` must be Bool, got Int\n\
                 [line 8] error: operands of `or` must be Bool, got Symbol\n\
                 [line 9] error: undefined variable `x`\n\
                 [line 10] error: the condition of `if` must be a Bool, got Int",
            ),
            (
                "if 1 < 2 { :yes } else { 1 / 0 }\nif false { 1 / 0 } else { :no }\n\
                 let n = 0\nif n < 0 { :neg } else if n == 0 { :zero } else { :pos }\n1 + if true { 2 } else { 3 }",
                ":yes\n:no\n:zero\n3",
            ),
        ]);
    }

    #[test]
    fn let_binds_names_that_blocks_scope() {
        runs(&[
            (
                "let x = 1\nlet x = x + 1\nx\nlet x = 1 / 0\nx",
                "2\n[line 4] error: division by zero\n2",
            ),
            (
                "let area = {\n  let w = 4\n  w * 5\n}\narea\nw",
                "20\n[line 6] error: undefined variable `w`",
            ),
            (
                "let x = 1\nlet y = {\n  let x = x + 10\n  x\n}\ny\nx",
                "11\n1",
            ),
            // An error inside a block reports its own line.
            (
                "let x = 1\n{\n  let x = 2\n  x / 0\n}\nx",
                "[line 4] error: division by zero\n1",
            ),
            (
                "{\n  let a = 1\n  let b = {\n    let a = a + 10\n    a * 2\n  }\n  a + b\n}",
                "23",
            ),
            ("{\n  let a = 1\n  let a = a + 1\n  a * 10\n  a\n}", "2"),
            // A block can start above a temporary.
            ("1 + {\n  let a = 2\n  a * 3\n}", "7"),
            (
                "let x = 10\n{\n  let a = 1\n  if a > 0 {\n    let b = a + x\n    b * 2\n  } else {\n    0\n  }\n}",
                "22",
            ),
            (
                "{\n  1\n} + true\n",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Bool",
            ),
        ]);
    }

    #[test]
    fn calls_functions() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                &format!("{add}add(2, 3)\n2 |> add(3)\n2.add(3)\n1 + add(2, 3) * 2"),
                "5\n5\n5\n11",
            ),
            (
                "is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }\n\
                 is_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }\nis_even(10)\nis_odd(7)",
                "true\ntrue",
            ),
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
    fn closures_keep_the_locals_around_them() {
        runs(&[
            (
                "let double = (x: Int) -> Int { x * 2 }\ndouble(21)\ndouble",
                "42\n<fn>",
            ),
            (
                "adder :: (n: Int) -> Fn { (x: Int) -> Int { x + n } }\nadder(3)(4)\n[1, 2] |> map(adder(10))",
                "7\n[11, 12]",
            ),
            (
                "scaled :: (xs: List, k: Int) -> List {\n  xs |> map((x: Int) -> Int { x * k })\n}\nscaled([1, 2, 3], 10)",
                "[10, 20, 30]",
            ),
            // A later `let` of the same name is another local.
            (
                "{\n  let base = 100\n  let f = (x: Int) -> Int { x + base }\n  let base = 0\n  f(1)\n}",
                "101",
            ),
            (
                "let curry = {\n  let a = 1\n  (b: Int) -> Fn { (c: Int) -> Int { a + b + c } }\n}\ncurry(10)(100)",
                "111",
            ),
            // A local of the body hides the one kept with its name.
            (
                "{\n  let x = 1\n  let f = () -> Int {\n    let x = 2\n    x\n  }\n  f() + x\n}",
                "3",
            ),
            (
                "let pick = case Some(5) {\n  Some(n) -> (x: Int) -> Int { x + n }\n  None -> (x: Int) -> Int { x }\n}\npick(1)",
                "6",
            ),
            // The top level is looked up by name when it runs.
            ("let k = 1\nlet f = () -> Int { k }\nlet k = 2\nf()", "2"),
            (
                "let f = (x: Int) -> Int {\n  10 / x\n}\n[1, 0] |> map(f)",
                "[line 2] error: division by zero",
            ),
            (
                "let f = (x: Int) -> Int { x }\nf(1, 2)",
                "[line 2] error: the function expects 1 argument, got 2",
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
            // comes back through a native, and leaves the caller's names.
            (
                "let x = 1\nboom :: (x: Int) -> Int {\n  1 / x\n}\nboom(0)\nrange(0, 1) |> map(boom)\nx",
                "[line 3] error: division by zero\n[line 3] error: division by zero\n1",
            ),
            (
                "loop_forever :: (n: Int) -> Int { loop_forever(n + 1) }\nloop_forever(0)",
                "[line 1] error: stack overflow: more than 200 nested calls",
            ),
        ]);
    }

    #[test]
    fn natives_work_on_numbers_and_lists() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\n\
                   even :: (x: Int) -> Bool { x % 2 == 0 }\n\
                   add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                "abs(-3)\nmin(2.0, 1.5)\nclamp(5, 0, 3)\nto_int(-2.7)\nto_float(3)\nto_string(3) + \"!\"\n3 |> print |> abs",
                "3\n1.5\n3\n-2\n3.0\n3!\n3",
            ),
            (
                "[\"a\", :b]\n[[1], [2, 3]]\n[1, 2] == [1, 2]\nconcat([1], [2, 3])\n[:a, :b] |> contains(:b)\n[1] |> contains(1.0)",
                "[\"a\", :b]\n[[1], [2, 3]]\ntrue\n[1, 2, 3]\ntrue\nfalse",
            ),
            (
                "{\n  let a = 1\n  [a, a + 1] |> concat([:b, \"c\"])\n}",
                "[1, 2, :b, \"c\"]",
            ),
            // Natives are globals like any other.
            ("abs\nlet abs = 1\nabs + 1", "<fn abs>\n2"),
            (
                &format!(
                    "{fns}range(0, 5)\nrange(3, 1)\n[] |> fold(7, add)\n\
                     range(0, 10) |> filter(even) |> map(double) |> fold(0, add)\n[-1, 2] |> map(abs)\nrange(1, 1001) |> fold(0, add)"
                ),
                "[0, 1, 2, 3, 4]\n[]\n7\n40\n[1, 2]\n500500",
            ),
            (
                "big :: (n: Int) -> Bool { n > 5 }\nfind(range(0, 10), big)\nfind(range(0, 3), big)",
                "Some(6)\nNone",
            ),
            (
                "parse_int(\"42\")\nparse_int(\" -7 \")\nparse_int(\"4x\")",
                "Ok(42)\nOk(-7)\nError(\"\\\"4x\\\" is not an Int\")",
            ),
        ]);
    }

    #[test]
    fn reports_errors_in_natives() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\nadd :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[(
            &format!(
                "{fns}abs(true)\nclamp(1, 3, 0)\nmin(1)\nabs == abs\n1 |> map(double)\n[1] |> filter(double)\n\
                 [1] |> map(add)\nrange(0, 1.0)\nrange(0, 2000000)\nfind([1], abs)\nparse_int(4)"
            ),
            "[line 3] error: `abs` expects an Int or a Float, got Bool\n\
             [line 4] error: `clamp` expects its lower bound to be at most its upper bound\n\
             [line 5] error: `min` expects 2 arguments, got 1\n\
             [line 6] error: functions cannot be compared\n\
             [line 7] error: `map` expects a List, got Int\n\
             [line 8] error: `filter` expects a function that returns a Bool, got Int\n\
             [line 9] error: `add` expects 2 arguments, got 1\n\
             [line 10] error: `range` expects two Ints, got Int and Float\n\
             [line 11] error: `range` can make at most 1000000 numbers, got 2000000\n\
             [line 12] error: `find` expects a function that returns a Bool, got Int\n\
             [line 13] error: `parse_int` expects a String, got Int",
        )]);
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
            (
                &format!(
                    "{ball}World :: struct {{ ball: Ball, score: Int }}\n\
                     bounce :: (w: World) -> World {{\n  let b = w.ball\n  World {{ w | ball: Ball {{ b | x: -b.x }}, score: w.score + 1 }}\n}}\n\
                     let w = World {{ ball: Ball {{ x: 3.0, y: 0.0 }}, score: 0 }}\nbounce(w)"
                ),
                "World { ball: Ball { x: -3.0, y: 0.0 }, score: 1 }",
            ),
            (
                &format!(
                    "{ball}Paddle :: struct {{ y: Float }}\nlet n = 1\nState :: enum {{ :idle }}\n\
                     Ball {{ x: 1.0, y: 2.0 }} == Paddle {{ y: 2.0 }}\n\
                     Ball {{ x: 1.0 }}\n\
                     Ball {{ x: 1.0, y: 2.0, z: 3.0 }}\n\
                     Wall {{ y: 1.0 }}\n\
                     Ball {{ 1 | x: 2.0 }}\n\
                     n.x\n\
                     Ball {{ x: 1.0, y: 2.0 }}.z\n\
                     Ball\n\
                     State"
                ),
                "[line 5] error: operands of `==` must be two values of the same type, got Ball and Paddle\n\
                 [line 6] error: missing field `y` in `Ball`\n\
                 [line 7] error: `Ball` has no field `z`\n\
                 [line 8] error: undefined struct `Wall`\n\
                 [line 9] error: `Ball { x | ... }` needs a Ball as `x`, got Int\n\
                 [line 10] error: Int has no fields\n\
                 [line 11] error: `Ball` has no field `z`\n\
                 [line 12] error: `Ball` is a struct, not a value: build one with `Ball { ... }`\n\
                 [line 13] error: `State` is an enum, not a value: its values are its symbols, like `:idle`",
            ),
        ]);
    }

    #[test]
    fn case_takes_the_first_branch_that_matches() {
        runs(&[
            (
                "let s = :serving\ncase s {\n  :idle -> 1\n  :serving -> 2\n}",
                "2",
            ),
            // Guards, names that bind the value, and `_` for the rest.
            (
                "describe :: (n: Int) -> String {\n  case n {\n    0 -> \"zero\"\n    m when m < 0 -> \"negative\"\n    m when m % 2 == 0 -> to_string(m) + \" is even\"\n    _ -> \"odd\"\n  }\n}\n\
                 describe(0)\ndescribe(-3)\ndescribe(4)\ndescribe(7)",
                "zero\nnegative\n4 is even\nodd",
            ),
            // A value of another type is no match, and no error.
            ("case 1 {\n  1.0 -> :float\n  \"1\" -> :string\n  1 -> :int\n}", ":int"),
            // Inside an expression, with locals around it and in a branch.
            (
                "{\n  let a = 10\n  1 + case a {\n    n when n > 5 -> {\n      let d = n * 2\n      d + a\n    }\n    _ -> 0\n  }\n}",
                "31",
            ),
            ("case true { false -> 0, true -> 1 }", "1"),
            (
                "case :c {\n  :a -> 1\n  :b -> 2\n}\ncase \"x\" { \"y\" -> 1 }",
                "[line 1] error: no branch of `case` matches :c\n[line 5] error: no branch of `case` matches \"x\"",
            ),
            (
                "case 1 {\n  n when n -> 1\n  _ -> 2\n}",
                "[line 2] error: the condition of `when` must be a Bool, got Int",
            ),
            ("case 1 {\n  :a -> 1\n  _ -> 1 / 0\n}", "[line 3] error: division by zero"),
        ]);
    }

    #[test]
    fn options_and_results_are_built_compared_and_read_by_case() {
        runs(&[
            (
                "Some(1)\nNone\nOk(\"yes\")\nError(:nope)\n[Some(1), None]\nSome(Some(2))\n\
                 Some(1) == Some(1)\nSome(1) == None\nOk(1) == Error(1)\nrange(0, 3) |> map(Some)\nSome",
                "Some(1)\nNone\nOk(\"yes\")\nError(:nope)\n[Some(1), None]\nSome(Some(2))\n\
                 true\nfalse\nfalse\n[Some(0), Some(1), Some(2)]\n<fn Some>",
            ),
            (
                "Some(1) + 1\nSome()\nSome(1) == Ok(1)",
                "[line 1] error: operands of `+` must be two Ints, two Floats or two Strings, got Option and Int\n\
                 [line 2] error: `Some` expects 1 argument, got 0\n\
                 [line 3] error: operands of `==` must be two values of the same type, got Option and Result",
            ),
            (
                "describe :: (r: Result[Int, String]) -> String {\n  case r {\n    Ok(0) -> \"zero\"\n    Ok(n) when n < 0 -> \"negative\"\n    Ok(n) -> to_string(n)\n    Error(reason) -> \"error: \" + reason\n  }\n}\n\
                 describe(Ok(0))\ndescribe(Ok(-1))\ndescribe(Ok(7))\ndescribe(Error(\"bad\"))",
                "zero\nnegative\n7\nerror: bad",
            ),
            // Nested patterns, and a guard that fails after a binding.
            (
                "case Some(Ok(3)) { Some(Error(_)) -> 1, Some(Ok(n)) -> n, None -> 0 }\n\
                 case Some(5) { Some(x) when x > 10 -> :big, Some(x) -> x, None -> 0 }\n\
                 case None { Some(x) when x > 1 -> 1, _ -> 2 }\ncase None {\n  Some(x) -> x\n}",
                "3\n5\n2\n[line 4] error: no branch of `case` matches None",
            ),
        ]);
    }

    const BALL: &str = "Ball :: struct { x: Float, y: Float }\n";
    const START: &str = "Ball { x: 0.0, y: 0.0 }";

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

    /// A VM with `Ball` and the sequence `s` with these parameters
    /// and steps, started over a ball.
    fn sequence(params: &str, steps: &str, args: &str) -> Machine {
        Machine::new(&format!(
            "{BALL}half :: (x: Float) -> Float {{ x / 2.0 }}\n\
             s :: sequence ({params}) {{\n  {steps}\n}}\nstart s({START}{args})"
        ))
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
            (
                "over 0.5s as t {\n    b.x = half(t) |> min(0.3)\n  }",
                2,
                "0.25, 0.3 done",
            ),
            // Binding the state again replaces it.
            (
                "let b = Ball { x: 5.0, y: 0.0 }\n  b.x = b.x + 1.0\n  over 0.5s {\n    let b = Ball { b | x: b.x * 2.0 }\n  }",
                2,
                "12.0, 24.0 done",
            ),
        ];
        for (steps, count, expected) in cases {
            let frames = sequence("b: Ball", steps, "").frames(ball(0.0), 0.25, count);
            assert_eq!(frames, expected, "{steps:?}");
        }

        let mut vm = sequence("b: Ball", "over 0.1s as t {\n    b.x = t\n  }", "");
        let at_60 = vm.frames(ball(0.0), 1.0 / 60.0, 6);
        assert!(at_60.ends_with("0.8333333333333334, 1.0 done"), "{at_60}");

        // Locals of the sequence last between frames; those of the body of
        // `over` last a pass.
        let mut vm = sequence(
            "b: Ball, to: Float",
            "let from = b.x\n  over 0.5s as t {\n    let d = to - from\n    b.x = from + d * t\n  }",
            ", 20.0",
        );
        assert_eq!(vm.frames(ball(10.0), 0.25, 2), "15.0, 20.0 done");
    }

    #[test]
    fn running_sequences_step_over_the_current_world() {
        let mut vm = Machine::new(&format!(
            "{BALL}push :: sequence (b: Ball, by: Float) {{\n  wait 0.5s\n  b.x = b.x + by\n}}\n\
             kick :: (b: Ball) -> Ball {{\n  start push(b, 10.0)\n  b\n}}\nkick({START})"
        ));
        assert_eq!(vm.running(), ["push"]);
        assert_eq!(vm.frames(ball(1.0), 0.25, 2), "1.0, 11.0 done");

        let mut vm = sequence("b: Ball", "over 1.0s {\n    b.x = b.x + 1.0\n  }", "");
        assert_eq!(
            vm.frames(ball(100.0), 0.25, 4),
            "101.0, 102.0, 103.0, 104.0 done"
        );

        // They run in the order they were started, and one started by
        // another joins after it, from the next frame on.
        let mut vm = Machine::new(&format!(
            "{BALL}double :: sequence (b: Ball) {{\n  b.x = b.x * 2.0\n}}\n\
             inc :: sequence (b: Ball) {{\n  b.x = b.x + 1.0\n}}\nstart double({START})\nstart inc({START})"
        ));
        assert_eq!(vm.frames(ball(3.0), 0.25, 1), "7.0 done");
        let mut vm = Machine::new(&format!(
            "{BALL}second :: sequence (b: Ball) {{\n  b.x = b.x + 10.0\n}}\n\
             first :: sequence (b: Ball) {{\n  b.x = 1.0\n  start second(b)\n}}\nstart first({START})"
        ));
        assert_eq!(vm.frames(ball(0.0), 0.25, 2), "1.0, 11.0 done");

        // `grow` runs first, so `pause` reads `b.x` for its second `over` on
        // the next frame, 0.5 and two passes, not on the first, 0.25.
        let mut vm = Machine::new(&format!(
            "{BALL}grow :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + 0.25\n  }}\n}}\n\
             pause :: sequence (b: Ball) {{\n  wait 0.25s\n  over b.x {{}}\n  b.x = 100.0\n}}\n\
             start grow({START})\nstart pause({START})"
        ));
        assert_eq!(
            vm.frames(ball(0.0), 0.25, 4),
            "0.25, 0.5, 100.0, 100.25 done"
        );
    }

    #[test]
    fn starting_a_running_sequence_replaces_it() {
        let mut vm = sequence("b: Ball, to: Float", "wait 0.5s\n  b.x = to", ", 1.0");
        assert_eq!(vm.frames(ball(0.0), 0.25, 1), "0.0");
        vm.run(&format!("start s({START}, 2.0)"));
        assert_eq!(vm.running(), ["s"]);
        assert_eq!(vm.frames(ball(0.0), 0.25, 2), "0.0, 2.0 done");
    }

    #[test]
    fn reports_errors_in_sequences() {
        let cases = [
            ("wait 1", "a duration must be a Float in seconds, got Int"),
            (
                "wait -1.0s",
                "a duration must be a finite number of seconds, at least 0s, got -1.0",
            ),
            ("b.z = 1.0", "`Ball` has no field `z`"),
        ];
        for (steps, message) in cases {
            let frames = sequence("b: Ball", steps, "").frames(ball(0.0), 0.25, 1);
            assert_eq!(frames, format!("[line 4] error: {message}"));
        }
        let frames = sequence("b: Int", "b.z = 1.0", "").frames(Value::Int(1), 0.25, 1);
        assert_eq!(
            frames,
            "[line 4] error: `b.z = ...` needs `b` to be a struct, got Int"
        );

        assert_eq!(
            Machine::new("").run("s :: sequence (n: Int) {}\nstart s()\nstart t(1)\ns(1)"),
            "[line 2] error: `s` expects 1 argument, got 0\n\
             [line 3] error: undefined sequence `t`\n\
             [line 4] error: `s` is a sequence, not a function: it runs over time and cannot be called"
        );
    }

    #[test]
    fn a_reload_keeps_running_sequences_with_their_code_and_the_new_globals() {
        let source = |step: &str, times: &str, start: &str| {
            format!(
                "{BALL}step :: () -> Float {{ {step} }}\n\
                 s :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + step() * {times}\n  }}\n}}\n{start}"
            )
        };
        let start = format!("start s({START})");
        let reload = |old: &mut Machine, new: &mut Machine| {
            new.0.adopt_sequences(&mut old.0);
            assert!(old.running().is_empty());
        };

        let mut old = Machine::new(&source("1.0", "1.0", &start));
        assert_eq!(old.frames(ball(0.0), 0.25, 1), "1.0");
        // The running `s` keeps adding `step()` once, as its old code says,
        // but `step` is the new one.
        let mut new = Machine::new(&source("10.0", "2.0", ""));
        reload(&mut old, &mut new);
        assert_eq!(new.frames(ball(1.0), 0.25, 3), "11.0, 21.0, 31.0 done");

        // One that the new code started already wins over the old one.
        let mut old = Machine::new(&source("1.0", "1.0", &start));
        let mut new = Machine::new(&source("10.0", "2.0", &start));
        reload(&mut old, &mut new);
        assert_eq!(new.frames(ball(0.0), 0.25, 1), "20.0");

        new.0.stop_sequences();
        assert_eq!(new.frames(ball(0.0), 0.25, 1), "0.0 done");
    }

    #[test]
    fn the_host_calls_functions() {
        let mut vm = Machine::new(&format!(
            "{BALL}add :: (a: Int, b: Int) -> Int {{ a + b }}\nlet n = 1\n\
                 count :: (n: Int) -> Int {{ if n == 0 {{ 0 }} else {{ 1 + count(n - 1) }} }}"
        ));
        let int = Value::Int;
        assert_eq!(vm.call("add", vec![int(2), int(3)]), Ok(int(5)));
        assert_eq!(vm.call("abs", vec![int(-4)]), Ok(int(4)));
        assert_eq!(
            vm.call("n", vec![]),
            Err("can only call functions, got Int".into())
        );
        assert_eq!(
            vm.call("nope", vec![]),
            Err("undefined function `nope`".into())
        );
        // With no script below, the host's call is the first nested one.
        assert_eq!(vm.call("count", vec![int(199)]), Ok(int(199)));
        assert_eq!(
            vm.call("count", vec![int(200)]),
            Err("stack overflow: more than 200 nested calls".into())
        );

        let shapes = HashMap::from([("Ball".to_string(), vec!["x".into(), "y".into()])]);
        assert_eq!(vm.0.arity("add"), Some(2));
        assert_eq!(vm.0.arity("abs"), Some(1));
        assert_eq!(vm.0.arity("n"), None);
        assert_eq!(vm.0.arity("nope"), None);
        assert_eq!(vm.0.struct_shapes(), shapes);
    }

    #[test]
    fn a_sequence_keeps_its_closures_between_frames() {
        let mut vm = Machine::new(&format!(
            "{BALL}s :: sequence (b: Ball) {{\n  let by = 5.0\n  let add = (x: Float) -> Float {{ x + by }}\n  wait 0.5s\n  b.x = add(b.x)\n}}\nstart s(Ball {{ x: 0.0, y: 0.0 }})"
        ));
        assert_eq!(vm.frames(ball(1.0), 0.25, 2), "1.0, 6.0 done");
    }

    #[test]
    fn a_sequence_keeps_only_its_own_slots_between_frames() {
        let source = "Ball :: struct { x: Float, y: Float }\n\
                      drift :: sequence (b: Ball) {\n  let from = b.x\n  over 10.0s {\n    b.x = b.x + 1.0\n  }\n}\n\
                      start drift(Ball { x: 0.0, y: 0.0 })\n\
                      let ball = Ball { x: 0.0, y: 0.0 }";
        let mut vm = Vm::new();
        for stmt in parse(scan_tokens(source).unwrap()).unwrap() {
            vm.interpret(&stmt).unwrap();
        }
        let mut world = vm.globals["ball"].clone();
        for _ in 0..20 {
            world = vm.run_sequences(world, 0.25).unwrap();
        }
        // The state, `from` and the two counters of the `over` halfway through.
        assert_eq!(vm.runs[0].slots.len(), 4);
        assert!(vm.stack.is_empty() && vm.frames.is_empty());
    }
}
