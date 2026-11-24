use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    Arm, BinaryOp, Expr, FnDecl, Pattern, SeqDecl, Step, StepKind, Stmt, StmtKind, UnaryOp, Variant,
};
use crate::chunk::Test;
use crate::prelude::globals;
use crate::value::{Caller, Instance, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
    pub line: Option<usize>,
}

impl RuntimeError {
    /// Gives the error this line, unless a deeper call already gave it one.
    pub(crate) fn at(mut self, line: usize) -> Self {
        self.line.get_or_insert(line);
        self
    }
}

pub(crate) fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        line: None,
    }
}

pub(crate) const MAX_CALL_DEPTH: usize = 200;

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

// What follows is the meaning of ludo when it runs, which the VM shares, so
// both backends give the same values and errors.

/// What a program declares besides its globals, which both backends look up
/// by name when they run, so a function can use what is declared after it:
/// the fields of its structs, the symbols of its enums and its sequences.
pub(crate) struct Declarations<S> {
    pub structs: HashMap<String, Rc<[String]>>,
    pub enums: HashMap<String, Vec<String>>,
    pub sequences: HashMap<String, S>,
}

impl<S> Default for Declarations<S> {
    fn default() -> Self {
        Declarations {
            structs: HashMap::new(),
            enums: HashMap::new(),
            sequences: HashMap::new(),
        }
    }
}

impl<S: Clone> Declarations<S> {
    pub fn fields(&self, name: &str) -> Result<&Rc<[String]>, RuntimeError> {
        self.structs
            .get(name)
            .ok_or_else(|| error(format!("undefined struct `{name}`")))
    }

    pub fn sequence(&self, name: &str) -> Result<S, RuntimeError> {
        self.sequences
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined sequence `{name}`")))
    }

    pub fn shapes(&self) -> HashMap<String, Vec<String>> {
        let shapes = self.structs.iter();
        shapes.map(|(name, f)| (name.clone(), f.to_vec())).collect()
    }

    /// The error for a name that is not a variable, which says what it is if
    /// it is declared as something else.
    pub fn undefined(&self, name: &str) -> RuntimeError {
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

pub(crate) fn struct_hint(name: &str) -> String {
    format!("`{name}` is a struct, not a value: build one with `{name} {{ ... }}`")
}

pub(crate) fn sequence_hint(name: &str) -> String {
    format!("`{name}` is a sequence, not a function: it runs over time and cannot be called")
}

pub(crate) fn enum_hint(name: &str, symbol: &str) -> String {
    format!("`{name}` is an enum, not a value: its values are its symbols, like `:{symbol}`")
}

pub(crate) fn check_arity(name: &str, expected: usize, got: usize) -> Result<(), RuntimeError> {
    if expected == got {
        return Ok(());
    }
    Err(error(format!(
        "`{name}` expects {expected} argument{}, got {got}",
        if expected == 1 { "" } else { "s" }
    )))
}

pub(crate) fn stack_overflow() -> RuntimeError {
    error(format!(
        "stack overflow: more than {MAX_CALL_DEPTH} nested calls"
    ))
}

pub(crate) fn not_callable(value: &Value) -> RuntimeError {
    error(format!(
        "can only call functions, got {}",
        value.type_name()
    ))
}

/// The Bool that `if`, `when`, `and` and `or` need: ludo has no truthiness.
pub(crate) fn check_bool(test: Test, value: &Value) -> Result<bool, RuntimeError> {
    match (value, test) {
        (Value::Bool(v), _) => Ok(*v),
        (v, Test::If | Test::When) => Err(error(format!(
            "the condition of `{test}` must be a Bool, got {}",
            v.type_name()
        ))),
        (v, Test::And | Test::Or) => Err(error(format!(
            "operands of `{test}` must be Bool, got {}",
            v.type_name()
        ))),
    }
}

pub(crate) fn unary(op: UnaryOp, right: Value) -> Result<Value, RuntimeError> {
    match (op, right) {
        (UnaryOp::Neg, Value::Int(v)) => v
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| error("integer overflow in `-`")),
        (UnaryOp::Neg, Value::Float(v)) => Ok(Value::Float(-v)),
        (UnaryOp::Not, Value::Bool(v)) => Ok(Value::Bool(!v)),
        (UnaryOp::Neg, v) => Err(error(format!(
            "operand of `-` must be an Int or a Float, got {}",
            v.type_name()
        ))),
        (UnaryOp::Not, v) => Err(error(format!(
            "operand of `!` must be a Bool, got {}",
            v.type_name()
        ))),
    }
}

pub(crate) fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    use Value::{Bool, Float, Int, Str};

    let checked = |result: Option<i64>| {
        result
            .map(Int)
            .ok_or_else(|| error(format!("integer overflow in `{op}`")))
    };
    match (op, left, right) {
        (Div | Rem, Int(_), Int(0)) => Err(error("division by zero")),

        (Add, Int(a), Int(b)) => checked(a.checked_add(b)),
        (Sub, Int(a), Int(b)) => checked(a.checked_sub(b)),
        (Mul, Int(a), Int(b)) => checked(a.checked_mul(b)),
        (Div, Int(a), Int(b)) => checked(a.checked_div(b)),
        (Rem, Int(a), Int(b)) => checked(a.checked_rem(b)),

        (Add, Float(a), Float(b)) => Ok(Float(a + b)),
        (Sub, Float(a), Float(b)) => Ok(Float(a - b)),
        (Mul, Float(a), Float(b)) => Ok(Float(a * b)),
        (Div, Float(a), Float(b)) => Ok(Float(a / b)),
        (Rem, Float(a), Float(b)) => Ok(Float(a % b)),

        (Add, Str(a), Str(b)) => Ok(Str(a + &b)),

        (Lt, Int(a), Int(b)) => Ok(Bool(a < b)),
        (Le, Int(a), Int(b)) => Ok(Bool(a <= b)),
        (Gt, Int(a), Int(b)) => Ok(Bool(a > b)),
        (Ge, Int(a), Int(b)) => Ok(Bool(a >= b)),

        (Lt, Float(a), Float(b)) => Ok(Bool(a < b)),
        (Le, Float(a), Float(b)) => Ok(Bool(a <= b)),
        (Gt, Float(a), Float(b)) => Ok(Bool(a > b)),
        (Ge, Float(a), Float(b)) => Ok(Bool(a >= b)),

        (Eq | Ne, a, b) if a.is_function() && b.is_function() => {
            Err(error("functions cannot be compared"))
        }

        (Eq, a, b) if a.same_type(&b) => Ok(Bool(a == b)),
        (Ne, a, b) if a.same_type(&b) => Ok(Bool(a != b)),

        (op, a, b) => {
            let expected = match op {
                Eq | Ne => "two values of the same type",
                Add => "two Ints, two Floats or two Strings",
                _ => "two Ints or two Floats",
            };
            Err(error(format!(
                "operands of `{op}` must be {expected}, got {} and {}",
                a.type_name(),
                b.type_name()
            )))
        }
    }
}

/// The value of a field of a struct.
pub(crate) fn field(target: &Value, name: &str) -> Result<Value, RuntimeError> {
    match target {
        Value::Struct(instance) => instance
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("`{}` has no field `{name}`", instance.name))),
        other => Err(error(format!("{} has no fields", other.type_name()))),
    }
}

/// Builds a struct of type `name`, whose `declared` fields come from those
/// of `base`, if it updates one, and the `fields` a literal gives in the
/// order they are written. `value` gives the value of each of these once its
/// field is known to be declared.
pub(crate) fn build_struct<'a, T>(
    name: &str,
    declared: &[String],
    base: Option<Value>,
    fields: impl IntoIterator<Item = (&'a str, T)>,
    mut value: impl FnMut(T) -> Result<Value, RuntimeError>,
) -> Result<Value, RuntimeError> {
    let mut slots: Vec<Option<Value>> = match base {
        Some(Value::Struct(instance)) if instance.name == name => instance
            .fields
            .iter()
            .map(|(_, v)| Some(v.clone()))
            .collect(),
        Some(other) => {
            return Err(error(format!(
                "`{name} {{ x | ... }}` needs a {name} as `x`, got {}",
                other.type_name()
            )))
        }
        None => vec![None; declared.len()],
    };

    for (field, given) in fields {
        let index = declared
            .iter()
            .position(|f| f == field)
            .ok_or_else(|| error(format!("`{name}` has no field `{field}`")))?;
        slots[index] = Some(value(given)?);
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
        name: name.to_string(),
        fields,
    })))
}

/// The state of a sequence with one of its fields replaced, for
/// `target.field = value`.
pub(crate) fn with_field(
    target: &str,
    state: &Value,
    field: &str,
    value: Value,
) -> Result<Value, RuntimeError> {
    let Value::Struct(instance) = state else {
        return Err(error(format!(
            "`{target}.{field} = ...` needs `{target}` to be a struct, got {}",
            state.type_name()
        )));
    };
    let mut instance = (**instance).clone();
    match instance.fields.iter_mut().find(|(f, _)| f == field) {
        Some((_, slot)) => *slot = value,
        None => return Err(error(format!("`{}` has no field `{field}`", instance.name))),
    }
    Ok(Value::Struct(Rc::new(instance)))
}

/// Whether a value matches a literal pattern: a value of the literal's type
/// equal to it. A value of another type does not match; it is no error.
pub(crate) fn matches(value: &Value, literal: &Value) -> bool {
    value.same_type(literal) && value == literal
}

/// What a value of an `Option` or a `Result` holds if it is this variant:
/// `Some(Some(x))` for a match with `x` inside, `Some(None)` for `None`, and
/// `None` for another variant or another type, which is no match.
pub(crate) fn contents(variant: Variant, value: &Value) -> Option<Option<Value>> {
    match (variant, value) {
        (Variant::Some, Value::Option(Some(held)))
        | (Variant::Ok, Value::Result(Ok(held)))
        | (Variant::Error, Value::Result(Err(held))) => Some(Some((**held).clone())),
        (Variant::None, Value::Option(None)) => Some(None),
        _ => None,
    }
}

pub(crate) fn no_match(value: &Value) -> RuntimeError {
    let shown = match value {
        Value::Str(v) => format!("{v:?}"),
        v => v.to_string(),
    };
    error(format!("no branch of `case` matches {shown}"))
}

/// How many passes, one per frame, an `over` of this duration makes.
pub(crate) fn passes(duration: Value, dt: f64) -> Result<usize, RuntimeError> {
    match duration {
        Value::Float(d) if d >= 0.0 && d.is_finite() => {
            Ok(((d / dt - 1e-9).ceil() as usize).max(1))
        }
        Value::Float(d) => Err(error(format!(
            "a duration must be a finite number of seconds, at least 0s, got {d:?}"
        ))),
        other => Err(error(format!(
            "a duration must be a Float in seconds, got {}",
            other.type_name()
        ))),
    }
}

/// How far along an `over` is after this pass, `t`: exactly 1 at the last.
pub(crate) fn progress(pass: usize, passes: usize) -> f64 {
    if pass == passes {
        1.0
    } else {
        pass as f64 / passes as f64
    }
}

/// A sequence that is running, however a backend keeps its place.
pub trait Coroutine {
    fn name(&self) -> &str;
    fn is_done(&self) -> bool;
}

/// How both backends run sequences; only how one takes a step differs.
pub trait Sequences {
    type Run: Coroutine;

    /// The sequences that are running, in the order they were started.
    fn runs(&mut self) -> &mut Vec<Self::Run>;

    /// Runs one frame of a sequence over the world, until it waits for the
    /// next frame or ends, and gives back the world as it leaves it.
    fn step(&mut self, run: &mut Self::Run, world: Value, dt: f64) -> Result<Value, RuntimeError>;

    /// Starts a sequence, replacing one with the same name.
    fn launch(&mut self, run: Self::Run) {
        let runs = self.runs();
        runs.retain(|r| r.name() != run.name());
        runs.push(run);
    }

    /// Runs one frame of every sequence over the world, in the order they
    /// were started, each over what the one before left. Sequences started
    /// on the way join at the end. It gives back the world as the last one
    /// left it; after an error, the rest wait for the next frame.
    fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        let mut runs = std::mem::take(self.runs());
        let mut result = Ok(world);
        for run in &mut runs {
            result = result.and_then(|world| self.step(run, world, dt));
        }
        runs.retain(|run| !run.is_done());
        let started = std::mem::replace(self.runs(), runs);
        for run in started {
            self.launch(run);
        }
        result
    }

    /// Takes over the sequences running in `from`, the code before a reload,
    /// except those this one already started. They keep the code they
    /// started with, and look globals up by name here.
    fn adopt_sequences(&mut self, from: &mut Self) {
        let mut runs = std::mem::take(from.runs());
        let own = self.runs();
        runs.retain(|run| !own.iter().any(|r| r.name() == run.name()));
        runs.append(own);
        *own = runs;
    }

    fn stop_sequences(&mut self) {
        self.runs().clear();
    }

    /// The names of the sequences that are running, in the order they run.
    #[cfg(test)]
    fn running(&mut self) -> Vec<String> {
        self.runs().iter().map(|r| r.name().to_string()).collect()
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
