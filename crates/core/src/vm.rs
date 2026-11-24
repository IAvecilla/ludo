use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::UnaryOp;
use crate::chunk::{Chunk, Function, Op, Shape};
use crate::interpreter::{
    binary, build_struct, check_arity, check_bool, contents, error, field, matches, no_match,
    not_callable, passes, progress, stack_overflow, unary, with_field, Coroutine, Declarations,
    RuntimeError, Sequences, MAX_CALL_DEPTH,
};
use crate::prelude::globals;
use crate::value::{Caller, Value};

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

impl Coroutine for Run {
    fn name(&self) -> &str {
        self.function.name()
    }

    fn is_done(&self) -> bool {
        self.done
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

/// A stack machine that runs compiled functions. The operators share their
/// meaning with the tree-walking interpreter, so both give the same values
/// and errors.
pub struct Vm {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    /// Looked up by name each time they are used, as in clox, so a reloaded
    /// game sees the new definitions.
    globals: HashMap<String, Value>,
    declared: Declarations<Rc<Function>>,
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
            Value::Compiled(function) => Some(function.arity),
            Value::Native(native) => Some(native.arity),
            _ => None,
        }
    }

    /// The fields of every declared struct, for `play` to tell whether a
    /// reload changed the shape of the world.
    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        self.declared.shapes()
    }

    /// Clears the VM and puts `function` on the stack as clox does with the
    /// script, with `slots` above it, to run from `ip`.
    fn enter(&mut self, function: Rc<Function>, ip: usize, slots: &[Value]) {
        self.stack.clear();
        self.frames.clear();
        self.root = 1;
        self.stack.push(Value::Compiled(Rc::clone(&function)));
        self.stack.extend_from_slice(slots);
        self.frames.push(CallFrame {
            function,
            ip,
            base: 1,
        });
    }

    /// Starts a sequence for `start`.
    fn start(&mut self, name: &str, args: Vec<Value>) -> Result<(), RuntimeError> {
        let function = self.declared.sequence(name)?;
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
                let instance = build_struct(name, declared, base, given, Ok)?;
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
                let Value::Compiled(sequence) = self.pop() else {
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
            Value::Compiled(function) => {
                check_arity(function.name(), function.arity, argc)?;
                if self.frames.len() - self.root >= MAX_CALL_DEPTH {
                    return Err(stack_overflow());
                }
                let base = self.stack.len() - argc;
                self.frames.push(CallFrame {
                    function,
                    ip: 0,
                    base,
                });
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

impl Sequences for Vm {
    type Run = Run;

    fn runs(&mut self) -> &mut Vec<Run> {
        &mut self.runs
    }

    /// Puts a sequence back on the stack and runs it until it waits for the
    /// next frame or ends, then takes it off again. If it fails it stays as
    /// it was, and the next frame tries again from where it last stopped.
    fn step(&mut self, run: &mut Run, world: Value, dt: f64) -> Result<Value, RuntimeError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::compile;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    #[test]
    fn a_sequence_keeps_only_its_own_slots_between_frames() {
        let source = "Ball :: struct { x: Float, y: Float }\n\
                      drift :: sequence (b: Ball) {\n  let from = b.x\n  over 10.0s {\n    b.x = b.x + 1.0\n  }\n}\n\
                      start drift(Ball { x: 0.0, y: 0.0 })\n\
                      let ball = Ball { x: 0.0, y: 0.0 }";
        let mut vm = Vm::new();
        for stmt in parse(scan_tokens(source).unwrap()).unwrap() {
            vm.run(compile(&stmt).unwrap()).unwrap();
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
