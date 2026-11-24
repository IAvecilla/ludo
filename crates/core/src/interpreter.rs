use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Arm, BinaryOp, Expr, FnDecl, Pattern, SeqDecl, Step, StepKind, Stmt, StmtKind};
use crate::chunk::Test;
use crate::prelude::globals;
use crate::runtime::{
    binary, build_struct, check_arity, check_bool, contents, error, field, matches, no_match,
    not_callable, passes, progress, stack_overflow, unary, with_field, Coroutine, Declarations,
    RuntimeError, Sequences, MAX_CALL_DEPTH,
};
use crate::value::{Caller, Value};

/// A tree-walking interpreter, as jlox. Locals live in one list for every
/// call, and `frame` is where those of the running function start.
pub struct Interpreter {
    locals: Vec<(String, Value)>,
    frame: usize,
    /// How many blocks deep the code runs: a `let` at 0 binds a global.
    nested: usize,
    depth: usize,
    globals: HashMap<String, Value>,
    declared: Declarations<Rc<SeqDecl>>,
    runs: Vec<Run>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            locals: Vec::new(),
            frame: 0,
            nested: 0,
            depth: 0,
            globals: globals(),
            declared: Declarations::default(),
            runs: Vec::new(),
        }
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let callee = self
            .globals
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined function `{name}`")))?;
        self.call(callee, args)
    }

    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        self.declared.shapes()
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        match self.globals.get(name)? {
            Value::Function(decl) => Some(decl.params.len()),
            Value::Native(native) => Some(native.arity),
            _ => None,
        }
    }

    pub fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, RuntimeError> {
        let result = match &stmt.kind {
            StmtKind::Expr(expr) => {
                return self.evaluate(expr).map(Some).map_err(|e| e.at(stmt.line))
            }
            StmtKind::Let(name, value) => self.evaluate(value).map(|value| self.bind(name, value)),
            StmtKind::Fn(decl) => {
                self.globals
                    .insert(decl.name.clone(), Value::Function(decl.clone()));
                Ok(())
            }
            StmtKind::Struct(decl) => {
                let fields = decl.fields.iter().map(|f| f.name.clone()).collect();
                self.declared.structs.insert(decl.name.clone(), fields);
                Ok(())
            }
            StmtKind::Enum(decl) => {
                let symbols = decl.symbols.clone();
                self.declared.enums.insert(decl.name.clone(), symbols);
                Ok(())
            }
            // A union only tells the checker what a type can be.
            StmtKind::Union(_) => Ok(()),
            StmtKind::Sequence(decl) => {
                let (name, sequence) = (decl.name.clone(), decl.clone());
                self.declared.sequences.insert(name, sequence);
                Ok(())
            }
            StmtKind::Start(name, args) => {
                self.arguments(args).and_then(|args| self.start(name, args))
            }
        };
        result.map(|()| None).map_err(|e| e.at(stmt.line))
    }

    fn evaluate(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Int(v) => Ok(Value::Int(*v)),
            Expr::Float(v) => Ok(Value::Float(*v)),
            Expr::Bool(v) => Ok(Value::Bool(*v)),
            Expr::Str(v) => Ok(Value::Str(v.clone())),
            Expr::Symbol(v) => Ok(Value::Symbol(v.clone())),
            Expr::Ident(name) => self.variable(name),
            Expr::List(items) => Ok(Value::List(Rc::new(self.arguments(items)?))),
            Expr::Block(statements, tail, line) => {
                let outer = self.locals.len();
                self.nested += 1;
                let result = statements
                    .iter()
                    .try_for_each(|stmt| self.execute(stmt).map(drop))
                    .and_then(|()| self.evaluate(tail).map_err(|e| e.at(*line)));
                self.nested -= 1;
                self.locals.truncate(outer);
                result
            }
            Expr::If(cond, then, otherwise) => {
                if self.boolean(cond, Test::If)? {
                    self.evaluate(then)
                } else {
                    self.evaluate(otherwise)
                }
            }
            Expr::Unary(op, right) => {
                let right = self.evaluate(right)?;
                unary(*op, right)
            }
            Expr::Binary(left, op @ (BinaryOp::And | BinaryOp::Or), right) => {
                // The value that ends the evaluation early: false for `and`.
                let (test, stop) = match op {
                    BinaryOp::And => (Test::And, false),
                    _ => (Test::Or, true),
                };
                if self.boolean(left, test)? == stop {
                    return Ok(Value::Bool(stop));
                }
                Ok(Value::Bool(self.boolean(right, test)?))
            }
            Expr::Binary(left, op, right) => {
                let left = self.evaluate(left)?;
                let right = self.evaluate(right)?;
                binary(*op, left, right)
            }
            Expr::Call(callee, args) => {
                let callee = self.evaluate(callee)?;
                let args = self.arguments(args)?;
                self.call(callee, args)
            }
            Expr::Field(target, name) => {
                let target = self.evaluate(target)?;
                field(&target, name)
            }
            Expr::Struct { name, base, fields } => self.construct(name, base.as_deref(), fields),
            Expr::Case(value, arms) => self.case(value, arms),
        }
    }

    fn variable(&self, name: &str) -> Result<Value, RuntimeError> {
        match self.lookup(name) {
            Some(value) => Ok(value.clone()),
            None => Err(self.declared.undefined(name)),
        }
    }

    fn construct(
        &mut self,
        name: &str,
        base: Option<&Expr>,
        fields: &[(String, Expr)],
    ) -> Result<Value, RuntimeError> {
        let declared = Rc::clone(self.declared.fields(name)?);
        let base = base.map(|b| self.evaluate(b)).transpose()?;
        let fields = fields.iter().map(|(field, expr)| (field.as_str(), expr));
        build_struct(name, &declared, base, fields, |expr| self.evaluate(expr))
    }

    fn case(&mut self, value: &Expr, arms: &[Arm]) -> Result<Value, RuntimeError> {
        let value = self.evaluate(value)?;
        for arm in arms {
            let outer = self.locals.len();
            self.nested += 1;
            let result = self.arm(arm, &value);
            self.nested -= 1;
            self.locals.truncate(outer);
            if let Some(result) = result? {
                return Ok(result);
            }
        }
        Err(no_match(&value))
    }

    fn invoke(&mut self, decl: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if self.depth == MAX_CALL_DEPTH {
            return Err(stack_overflow());
        }
        let caller = std::mem::replace(&mut self.frame, self.locals.len());
        for (param, value) in decl.params.iter().zip(args) {
            self.locals.push((param.name.clone(), value));
        }
        self.depth += 1;
        self.nested += 1;
        let result = self.evaluate(&decl.body);
        self.nested -= 1;
        self.depth -= 1;
        self.locals.truncate(self.frame);
        self.frame = caller;
        result
    }

    fn arguments(&mut self, args: &[Expr]) -> Result<Vec<Value>, RuntimeError> {
        args.iter().map(|arg| self.evaluate(arg)).collect()
    }

    fn bind(&mut self, name: &str, value: Value) {
        if self.nested == 0 {
            self.globals.insert(name.to_string(), value);
        } else {
            self.locals.push((name.to_string(), value));
        }
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        self.locals[self.frame..]
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, value)| value)
            .or_else(|| self.globals.get(name))
    }

    /// The value of a branch of a `case`, if its pattern matches `value` and
    /// its guard holds.
    fn arm(&mut self, arm: &Arm, value: &Value) -> Result<Option<Value>, RuntimeError> {
        if !bind(&arm.pattern, value, &mut self.locals) {
            return Ok(None);
        }
        if let Some(guard) = &arm.guard {
            if !self
                .boolean(guard, Test::When)
                .map_err(|e| e.at(arm.line))?
            {
                return Ok(None);
            }
        }
        self.evaluate(&arm.body)
            .map(Some)
            .map_err(|e| e.at(arm.line))
    }

    /// Evaluates an expression that must give a Bool. It runs in a frame of
    /// its own so that its temporaries do not grow the frame of `evaluate`,
    /// which deep recursion stacks up.
    fn boolean(&mut self, expr: &Expr, test: Test) -> Result<bool, RuntimeError> {
        check_bool(test, &self.evaluate(expr)?)
    }
}

impl Caller for Interpreter {
    fn call(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match callee {
            Value::Function(decl) => {
                check_arity(&decl.name, decl.params.len(), args.len())?;
                self.invoke(&decl, args)
            }
            Value::Native(native) => {
                check_arity(native.name, native.arity, args.len())?;
                (native.fun)(self, &args)
            }
            Value::Compiled(_) => unreachable!("only the VM makes compiled functions"),
            other => Err(not_callable(&other)),
        }
    }
}

/// Whether a value matches a pattern, pushing what the pattern binds to
/// `bindings`.
fn bind(pattern: &Pattern, value: &Value, bindings: &mut Vec<(String, Value)>) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Bind(name) => {
            bindings.push((name.clone(), value.clone()));
            true
        }
        Pattern::Literal(literal) => matches(value, &Value::from(literal)),
        Pattern::Variant(variant, inner) => match (contents(*variant, value), inner) {
            (Some(Some(held)), Some(inner)) => bind(inner, &held, bindings),
            (Some(None), None) => true,
            _ => false,
        },
    }
}

/// A sequence that is running: the step it is at, how far along an `over`
/// it is, and its locals, except the state, which is the world of the game.
pub struct Run {
    decl: Rc<SeqDecl>,
    locals: Vec<(String, Value)>,
    step: usize,
    pass: usize,
    passes: usize,
}

impl Coroutine for Run {
    fn name(&self) -> &str {
        &self.decl.name
    }

    fn is_done(&self) -> bool {
        self.step == self.decl.body.len()
    }
}

impl Sequences for Interpreter {
    type Run = Run;

    fn runs(&mut self) -> &mut Vec<Run> {
        &mut self.runs
    }

    /// Runs the steps of the sequence, with its locals and the world as its
    /// state, until it makes a pass of an `over` and meets another, or ends.
    fn step(&mut self, run: &mut Run, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        let decl = Rc::clone(&run.decl);
        let state = decl.params[0].name.as_str();
        let caller = std::mem::replace(&mut self.frame, self.locals.len());
        self.locals.append(&mut run.locals);
        self.locals.push((state.to_string(), world));
        self.nested += 1;
        let result = self.advance(run, dt);
        self.nested -= 1;

        // The newest binding of the state's name is the world it leaves.
        let mut world = None;
        for (name, value) in self.locals.split_off(self.frame) {
            if name == state {
                world = Some(value);
            } else {
                run.locals.push((name, value));
            }
        }
        self.frame = caller;
        result.map(|()| world.expect("a running sequence always has its state"))
    }
}

impl Interpreter {
    fn start(&mut self, name: &str, args: Vec<Value>) -> Result<(), RuntimeError> {
        let decl = self.declared.sequence(name)?;
        check_arity(&decl.name, decl.params.len(), args.len())?;
        // The state comes from the world of each frame instead.
        let params = decl.params[1..].iter().map(|param| param.name.clone());
        let locals = params.zip(args.into_iter().skip(1)).collect();
        self.launch(Run {
            decl,
            locals,
            step: 0,
            pass: 0,
            passes: 0,
        });
        Ok(())
    }

    fn advance(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        let decl = Rc::clone(&run.decl);
        let mut spent = false;
        while let Some(step) = decl.body.get(run.step) {
            let StepKind::Over {
                duration,
                var,
                body,
            } = &step.kind
            else {
                self.execute_step(step)?;
                run.step += 1;
                continue;
            };
            if spent {
                break;
            }

            if run.pass == 0 {
                let duration = self.evaluate(duration);
                run.passes = duration
                    .and_then(|d| passes(d, dt))
                    .map_err(|e| e.at(step.line))?;
            }
            run.pass += 1;
            let t = progress(run.pass, run.passes);
            self.over_pass(&decl.params[0].name, var.as_deref(), t, body)?;
            spent = true;

            if run.pass < run.passes {
                break;
            }
            run.pass = 0;
            run.step += 1;
        }
        Ok(())
    }

    /// Runs the body of an `over` once, as a block that only keeps the state.
    fn over_pass(
        &mut self,
        state: &str,
        var: Option<&str>,
        t: f64,
        body: &[Step],
    ) -> Result<(), RuntimeError> {
        let outer = self.locals.len();
        if let Some(var) = var {
            self.bind(var, Value::Float(t));
        }
        let result = body.iter().try_for_each(|step| self.execute_step(step));
        let value = self.lookup(state).cloned();
        self.locals.truncate(outer);
        result?;
        if let Some(value) = value {
            self.bind(state, value);
        }
        Ok(())
    }

    fn execute_step(&mut self, step: &Step) -> Result<(), RuntimeError> {
        match &step.kind {
            StepKind::Stmt(stmt) => self.execute(stmt).map(drop),
            StepKind::Set(target, field, expr) => self.evaluate(expr).and_then(|value| {
                let state = self
                    .lookup(target)
                    .expect("a running sequence always has its state");
                let updated = with_field(target, state, field, value)?;
                self.bind(target, updated);
                Ok(())
            }),
            StepKind::Over { .. } => unreachable!("the parser keeps `over` out of `over`"),
        }
        .map_err(|e| e.at(step.line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    #[test]
    fn a_sequence_keeps_only_its_own_locals_between_frames() {
        let source = "Ball :: struct { x: Float, y: Float }\n\
                      drift :: sequence (b: Ball) {\n  let from = b.x\n  over 10.0s {\n    b.x = b.x + 1.0\n  }\n}\n\
                      start drift(Ball { x: 0.0, y: 0.0 })\n\
                      let ball = Ball { x: 0.0, y: 0.0 }";
        let mut interpreter = Interpreter::new();
        for stmt in parse(scan_tokens(source).unwrap()).unwrap() {
            interpreter.execute(&stmt).unwrap();
        }
        let mut world = interpreter.globals["ball"].clone();
        for _ in 0..20 {
            world = interpreter.run_sequences(world, 0.25).unwrap();
        }
        let names: Vec<&str> = interpreter.runs[0]
            .locals
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["from"]);
        assert!(interpreter.locals.is_empty());
    }
}
