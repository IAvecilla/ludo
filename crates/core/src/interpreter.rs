use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    BinaryOp, Expr, FnDecl, SeqDecl, Step, StepKind, Stmt, StmtKind, StructDecl, UnaryOp,
};
use crate::prelude::NATIVES;
use crate::value::{Instance, Native, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
    pub line: Option<usize>,
}

impl RuntimeError {
    fn at(mut self, line: usize) -> Self {
        self.line.get_or_insert(line);
        self
    }
}

const MAX_CALL_DEPTH: usize = 200;

pub struct Interpreter {
    locals: Vec<(String, Value)>,
    frame: usize,
    nested: usize,
    globals: HashMap<String, Value>,
    depth: usize,
    structs: HashMap<String, Rc<StructDecl>>,
    sequences: HashMap<String, Rc<SeqDecl>>,
    runs: Vec<Run>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let mut interpreter = Interpreter {
            locals: Vec::new(),
            frame: 0,
            nested: 0,
            globals: HashMap::new(),
            depth: 0,
            structs: HashMap::new(),
            sequences: HashMap::new(),
            runs: Vec::new(),
        };
        for native in NATIVES {
            interpreter.define(*native);
        }
        interpreter
    }

    pub fn define(&mut self, native: Native) {
        self.globals
            .insert(native.name.to_string(), Value::Native(native));
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
        self.structs
            .iter()
            .map(|(name, decl)| {
                let fields = decl.fields.iter().map(|f| f.name.clone()).collect();
                (name.clone(), fields)
            })
            .collect()
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
            StmtKind::Let(name, value) => self.evaluate(value).map(|value| {
                self.bind(name.clone(), value);
                None
            }),
            StmtKind::Fn(decl) => {
                self.globals
                    .insert(decl.name.clone(), Value::Function(decl.clone()));
                Ok(None)
            }
            StmtKind::Struct(decl) => {
                self.structs.insert(decl.name.clone(), decl.clone());
                Ok(None)
            }
            StmtKind::Sequence(decl) => {
                self.sequences.insert(decl.name.clone(), decl.clone());
                Ok(None)
            }
            StmtKind::Start(name, args) => self
                .arguments(args)
                .and_then(|args| self.launch(name, args))
                .map(|()| None),
            StmtKind::Expr(expr) => self.evaluate(expr).map(Some),
        };
        result.map_err(|e| e.at(stmt.line))
    }

    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
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
            Expr::If(cond, then, otherwise) => match self.evaluate(cond)? {
                Value::Bool(true) => self.evaluate(then),
                Value::Bool(false) => self.evaluate(otherwise),
                other => Err(error(format!(
                    "the condition of `if` must be a Bool, got {}",
                    other.type_name()
                ))),
            },
            Expr::Unary(op, right) => self.unary(*op, right),
            Expr::Binary(left, op @ (BinaryOp::And | BinaryOp::Or), right) => {
                let stop = *op == BinaryOp::Or;
                if self.boolean(left, *op)? == stop {
                    return Ok(Value::Bool(stop));
                }
                Ok(Value::Bool(self.boolean(right, *op)?))
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
            Expr::Field(target, field) => match self.evaluate(target)? {
                Value::Struct(instance) => instance
                    .get(field)
                    .cloned()
                    .ok_or_else(|| error(format!("`{}` has no field `{field}`", instance.name))),
                other => Err(error(format!("{} has no fields", other.type_name()))),
            },
            Expr::Struct { name, base, fields } => self.construct(name, base.as_deref(), fields),
        }
    }

    fn variable(&self, name: &str) -> Result<Value, RuntimeError> {
        if let Some(value) = self.lookup(name) {
            return Ok(value.clone());
        }
        let message = if self.structs.contains_key(name) {
            format!("`{name}` is a struct, not a value: build one with `{name} {{ ... }}`")
        } else if self.sequences.contains_key(name) {
            format!(
                "`{name}` is a sequence, not a function: it runs over time and cannot be called"
            )
        } else {
            format!("undefined variable `{name}`")
        };
        Err(error(message))
    }

    fn construct(
        &mut self,
        name: &str,
        base: Option<&Expr>,
        fields: &[(String, Expr)],
    ) -> Result<Value, RuntimeError> {
        let decl = self
            .structs
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined struct `{name}`")))?;

        let mut values: Vec<Option<Value>> = match base.map(|b| self.evaluate(b)).transpose()? {
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
            None => vec![None; decl.fields.len()],
        };

        for (field, expr) in fields {
            let index = decl
                .fields
                .iter()
                .position(|f| &f.name == field)
                .ok_or_else(|| error(format!("`{name}` has no field `{field}`")))?;
            values[index] = Some(self.evaluate(expr)?);
        }

        let fields = decl
            .fields
            .iter()
            .zip(values)
            .map(|(field, value)| {
                let value = value
                    .ok_or_else(|| error(format!("missing field `{}` in `{name}`", field.name)))?;
                Ok((field.name.clone(), value))
            })
            .collect::<Result<_, RuntimeError>>()?;
        Ok(Value::Struct(Rc::new(Instance {
            name: name.to_string(),
            fields,
        })))
    }

    pub(crate) fn call(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match callee {
            Value::Function(decl) => {
                check_arity(&decl.name, decl.params.len(), args.len())?;
                self.invoke(&decl, args)
            }
            Value::Native(native) => {
                check_arity(native.name, native.arity, args.len())?;
                (native.fun)(self, &args)
            }
            other => Err(error(format!(
                "can only call functions, got {}",
                other.type_name()
            ))),
        }
    }

    fn invoke(&mut self, decl: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if self.depth == MAX_CALL_DEPTH {
            return Err(error(format!(
                "stack overflow: more than {MAX_CALL_DEPTH} nested calls"
            )));
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

    fn bind(&mut self, name: String, value: Value) {
        if self.nested == 0 {
            self.globals.insert(name, value);
        } else {
            self.locals.push((name, value));
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

    fn unary(&mut self, op: UnaryOp, right: &Expr) -> Result<Value, RuntimeError> {
        match (op, self.evaluate(right)?) {
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

    fn boolean(&mut self, expr: &Expr, op: BinaryOp) -> Result<bool, RuntimeError> {
        match self.evaluate(expr)? {
            Value::Bool(v) => Ok(v),
            v => Err(error(format!(
                "operands of `{op}` must be Bool, got {}",
                v.type_name()
            ))),
        }
    }
}

fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    use Value::{Bool, Float, Int, Str};

    match (op, left, right) {
        (Div | Rem, Int(_), Int(0)) => Err(error("division by zero")),

        (Add, Int(a), Int(b)) => checked(a.checked_add(b), op),
        (Sub, Int(a), Int(b)) => checked(a.checked_sub(b), op),
        (Mul, Int(a), Int(b)) => checked(a.checked_mul(b), op),
        (Div, Int(a), Int(b)) => checked(a.checked_div(b), op),
        (Rem, Int(a), Int(b)) => checked(a.checked_rem(b), op),

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

        (Eq | Ne, Value::Function(_) | Value::Native(_), Value::Function(_) | Value::Native(_)) => {
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

fn check_arity(name: &str, expected: usize, got: usize) -> Result<(), RuntimeError> {
    if expected == got {
        return Ok(());
    }
    Err(error(format!(
        "`{name}` expects {expected} argument{}, got {got}",
        if expected == 1 { "" } else { "s" }
    )))
}

fn checked(result: Option<i64>, op: BinaryOp) -> Result<Value, RuntimeError> {
    result
        .map(Value::Int)
        .ok_or_else(|| error(format!("integer overflow in `{op}`")))
}

pub(crate) fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        line: None,
    }
}

struct Run {
    decl: Rc<SeqDecl>,
    locals: Vec<(String, Value)>,
    state: Value,
    step: usize,
    pass: usize,
    passes: usize,
}

impl Run {
    fn is_done(&self) -> bool {
        self.step == self.decl.body.len()
    }

    fn state_name(&self) -> &str {
        &self.decl.params[0].name
    }
}

impl Interpreter {
    fn start(&mut self, name: &str, args: Vec<Value>) -> Result<Run, RuntimeError> {
        let decl = self
            .sequences
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined sequence `{name}`")))?;
        check_arity(&decl.name, decl.params.len(), args.len())?;

        let mut args = args.into_iter();
        let state = args.next().expect("a sequence has at least one parameter");
        let locals = decl.params[1..]
            .iter()
            .map(|param| param.name.clone())
            .zip(args)
            .collect();
        Ok(Run {
            decl,
            locals,
            state,
            step: 0,
            pass: 0,
            passes: 0,
        })
    }

    fn step(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        let caller = std::mem::replace(&mut self.frame, self.locals.len());
        self.locals.append(&mut run.locals);
        self.locals
            .push((run.state_name().to_string(), run.state.clone()));
        self.nested += 1;
        let result = self.advance(run, dt);
        self.nested -= 1;

        for (name, value) in self.locals.split_off(self.frame) {
            if name == run.state_name() {
                run.state = value;
            } else {
                run.locals.push((name, value));
            }
        }
        self.frame = caller;
        result
    }

    pub fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        let mut runs = std::mem::take(&mut self.runs);
        let mut world = world;
        let mut result = Ok(());
        for run in &mut runs {
            run.state = world;
            result = self.step(run, dt);
            world = run.state.clone();
            if result.is_err() {
                break;
            }
        }

        runs.retain(|r| !r.is_done());
        for run in std::mem::take(&mut self.runs) {
            replace(&mut runs, run);
        }
        self.runs = runs;
        result.map(|()| world)
    }

    pub fn adopt_sequences(&mut self, from: &mut Interpreter) {
        let mut runs = std::mem::take(&mut from.runs);
        runs.retain(|r| !self.runs.iter().any(|s| s.decl.name == r.decl.name));
        runs.append(&mut self.runs);
        self.runs = runs;
    }

    pub fn stop_sequences(&mut self) {
        self.runs.clear();
    }

    fn launch(&mut self, name: &str, args: Vec<Value>) -> Result<(), RuntimeError> {
        let run = self.start(name, args)?;
        replace(&mut self.runs, run);
        Ok(())
    }

    fn advance(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        let decl = run.decl.clone();
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
                run.passes = passes(self.evaluate(duration), dt).map_err(|e| e.at(step.line))?;
            }
            run.pass += 1;
            let t = if run.pass == run.passes {
                1.0
            } else {
                run.pass as f64 / run.passes as f64
            };
            self.over_pass(run.state_name(), var.as_deref(), t, body)?;

            spent = true;

            if run.pass < run.passes {
                break;
            }
            run.pass = 0;
            run.step += 1;
        }
        Ok(())
    }

    fn over_pass(
        &mut self,
        state: &str,
        var: Option<&str>,
        t: f64,
        body: &[Step],
    ) -> Result<(), RuntimeError> {
        let outer = self.locals.len();
        if let Some(var) = var {
            self.bind(var.to_string(), Value::Float(t));
        }
        let result = body.iter().try_for_each(|step| self.execute_step(step));
        let value = self.lookup(state).cloned();
        self.locals.truncate(outer);
        result?;
        if let Some(value) = value {
            self.bind(state.to_string(), value);
        }
        Ok(())
    }

    fn execute_step(&mut self, step: &Step) -> Result<(), RuntimeError> {
        match &step.kind {
            StepKind::Stmt(stmt) => self.execute(stmt).map(drop),
            StepKind::Set(target, field, expr) => self.set_field(target, field, expr),
            StepKind::Over { .. } => unreachable!("the parser keeps `over` out of `over`"),
        }
        .map_err(|e| e.at(step.line))
    }

    fn set_field(&mut self, target: &str, field: &str, expr: &Expr) -> Result<(), RuntimeError> {
        let value = self.evaluate(expr)?;
        let mut instance = match self.lookup(target) {
            Some(Value::Struct(instance)) => (**instance).clone(),
            Some(other) => {
                return Err(error(format!(
                    "`{target}.{field} = ...` needs `{target}` to be a struct, got {}",
                    other.type_name()
                )))
            }
            None => unreachable!("a running sequence always has its state"),
        };
        match instance.fields.iter_mut().find(|(f, _)| f == field) {
            Some((_, slot)) => *slot = value,
            None => return Err(error(format!("`{}` has no field `{field}`", instance.name))),
        }
        self.bind(target.to_string(), Value::Struct(Rc::new(instance)));
        Ok(())
    }
}

fn replace(runs: &mut Vec<Run>, run: Run) {
    runs.retain(|r| r.decl.name != run.decl.name);
    runs.push(run);
}

fn passes(duration: Result<Value, RuntimeError>, dt: f64) -> Result<usize, RuntimeError> {
    match duration? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse, parse_expr};
    use crate::scanner::scan_tokens;

    fn load(source: &str) -> (Interpreter, Vec<Result<Option<Value>, RuntimeError>>) {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        let statements = parse(tokens).expect("source should parse cleanly");
        let mut interpreter = Interpreter::new();
        let results = statements
            .iter()
            .map(|stmt| interpreter.execute(stmt))
            .collect();
        (interpreter, results)
    }

    fn outputs(source: &str) -> String {
        let lines: Vec<String> = load(source)
            .1
            .into_iter()
            .filter_map(|result| match result {
                Ok(value) => value.map(|v| v.to_string()),
                Err(error) => Some(format!("error: {}", error.message)),
            })
            .collect();
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
            (
                "1 + 2 * 3\n7 / 2\n7 % 2\n7.0 / 2.0\n-3\n1.0 / 0.0",
                "7\n3\n1\n3.5\n-3\ninf",
            ),
            (
                "1 < 2\n\"a\" + \"b\"\n:jump == :jump\n!true\nfalse and 1 / 0 == 1\ntrue or 1 / 0 == 1",
                "true\nab\ntrue\nfalse\nfalse\ntrue",
            ),
            (
                "1 + 2.0\n1 == 1.0\n1 / 0\n9223372036854775807 + 1\n-true\n1 and true\nx",
                "error: operands of `+` must be two Ints, two Floats or two Strings, got Int and Float\n\
                 error: operands of `==` must be two values of the same type, got Int and Float\n\
                 error: division by zero\n\
                 error: integer overflow in `+`\n\
                 error: operand of `-` must be an Int or a Float, got Bool\n\
                 error: operands of `and` must be Bool, got Int\n\
                 error: undefined variable `x`",
            ),
        ]);
    }

    #[test]
    fn let_and_blocks_scope_names() {
        runs(&[
            ("let x = 1\nlet x = x + 1\nx", "2"),
            ("let x = 1\nlet x = 1 / 0\nx", "error: division by zero\n1"),
            (
                "let area = {\n  let w = 4\n  w * 5\n}\narea\nw",
                "20\nerror: undefined variable `w`",
            ),
            (
                "let x = 1\nlet y = {\n  let x = x + 10\n  x\n}\ny\nx",
                "11\n1",
            ),
            (
                "let x = 1\n{\n  let x = 2\n  x / 0\n}\nx",
                "error: division by zero\n1",
            ),
        ]);
    }

    #[test]
    fn a_runtime_error_reports_the_inner_line() {
        let line = |source: &str| {
            let (_, results) = load(source);
            results.into_iter().find_map(Result::err).unwrap().line
        };
        assert_eq!(line("let x = 1\n\nlet y = x / 0"), Some(3));
        assert_eq!(line("let y = {\n  let x = 1 / 0\n  x\n}"), Some(2));
        assert_eq!(line("f :: (x: Int) -> Int {\n  x / 0\n}\nf(1)"), Some(2));
        assert_eq!(
            line("boom :: (x: Int) -> Int {\n  1 / x\n}\n[0] |> map(boom)"),
            Some(2)
        );
    }

    #[test]
    fn calls_functions() {
        let add = "add :: (a: Int, b: Int) -> Int { a + b }\n";
        let parity = "is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }\n\
                      is_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }\n";
        runs(&[
            (&format!("{add}add(2, 3)\n2 |> add(3)\n2.add(3)"), "5\n5\n5"),
            (&format!("{parity}is_even(10)\nis_odd(7)"), "true\ntrue"),
            ("scaled :: (x: Int) -> Int { x * SCALE }\nlet SCALE = 10\nscaled(3)", "30"),
            (
                "peek :: () -> Int { secret }\n{\n  let secret = 1\n  peek()\n}",
                "error: undefined variable `secret`",
            ),
            ("let x = 100\nid :: (x: Int) -> Int { x }\nid(1)\nx", "1\n100"),
            (
                "double :: (x: Int) -> Int { x * 2 }\ntwice :: (f: Fn, x: Int) -> Int { f(f(x)) }\ntwice(double, 3)\ndouble",
                "12\n<fn double>",
            ),
            (
                &format!("{add}id :: (x: Int) -> Int {{ x }}\nadd(1)\nid(1, 2)\nlet n = 1\nn(2)\nid == id"),
                "error: `add` expects 2 arguments, got 1\n\
                 error: `id` expects 1 argument, got 2\n\
                 error: can only call functions, got Int\n\
                 error: functions cannot be compared",
            ),
            (
                "let x = 1\nboom :: (x: Int) -> Int { x / 0 }\nboom(5)\nx",
                "error: division by zero\n1",
            ),
        ]);
        assert_eq!(
            outputs("loop_forever :: (n: Int) -> Int { loop_forever(n + 1) }\nloop_forever(0)"),
            format!("error: stack overflow: more than {MAX_CALL_DEPTH} nested calls")
        );
    }

    #[test]
    fn if_evaluates_only_the_branch_it_takes() {
        runs(&[
            (
                "if 1 < 2 { :yes } else { 1 / 0 }\nif false { 1 / 0 } else { :no }",
                ":yes\n:no",
            ),
            (
                "sign :: (n: Int) -> Symbol {\n  if n < 0 { :neg } else if n == 0 { :zero } else { :pos }\n}\nsign(0)",
                ":zero",
            ),
            (
                "fact :: (n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }\nfact(10)",
                "3628800",
            ),
            ("if 1 { 2 } else { 3 }", "error: the condition of `if` must be a Bool, got Int"),
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
            (
                &format!("{ball}Paddle :: struct {{ y: Float }}\nlet n = 1\ns :: sequence (n: Int) {{}}\n\
                          Ball {{ x: 1.0, y: 2.0 }} == Paddle {{ y: 2.0 }}\n\
                          Ball {{ x: 1.0 }}\n\
                          Ball {{ x: 1.0, y: 2.0, z: 3.0 }}\n\
                          Wall {{ y: 1.0 }}\n\
                          Ball {{ 1 | x: 2.0 }}\n\
                          n.x\n\
                          Ball\n\
                          s(1)"),
                "error: operands of `==` must be two values of the same type, got Ball and Paddle\n\
                 error: missing field `y` in `Ball`\n\
                 error: `Ball` has no field `z`\n\
                 error: undefined struct `Wall`\n\
                 error: `Ball { x | ... }` needs a Ball as `x`, got Int\n\
                 error: Int has no fields\n\
                 error: `Ball` is a struct, not a value: build one with `Ball { ... }`\n\
                 error: `s` is a sequence, not a function: it runs over time and cannot be called",
            ),
        ]);
    }

    #[test]
    fn calls_natives() {
        runs(&[
            (
                "abs(-3)\nmin(2.0, 1.5)\nclamp(5, 0, 3)\nto_int(-2.7)\nto_float(3)\nto_string(3) + \"!\"\nconcat([1], [2, 3])",
                "3\n1.5\n3\n-2\n3.0\n3!\n[1, 2, 3]",
            ),
            ("[\"a\", :b]\n[1, 2] == [1, 2]\n[:a, :b] |> contains(:b)\n[1] |> contains(1.0)", "[\"a\", :b]\ntrue\ntrue\nfalse"),
            ("abs\nlet abs = 1\nabs + 1", "<fn abs>\n2"),
            ("3 |> print |> abs", "3"),
            (
                "abs(true)\nclamp(1, 3, 0)\nmin(1)\nabs == abs",
                "error: `abs` expects an Int or a Float, got Bool\n\
                 error: `clamp` expects its lower bound to be at most its upper bound\n\
                 error: `min` expects 2 arguments, got 1\n\
                 error: functions cannot be compared",
            ),
        ]);
    }

    #[test]
    fn lists_are_traversed_with_map_filter_and_fold() {
        let fns = "double :: (x: Int) -> Int { x * 2 }\n\
                   even :: (x: Int) -> Bool { x % 2 == 0 }\n\
                   add :: (a: Int, b: Int) -> Int { a + b }\n";
        runs(&[
            (
                &format!("{fns}range(0, 5)\nrange(3, 1)\n[] |> fold(7, add)"),
                "[0, 1, 2, 3, 4]\n[]\n7",
            ),
            (
                &format!("{fns}range(0, 10) |> filter(even) |> map(double) |> fold(0, add)\n[-1, 2] |> map(abs)"),
                "40\n[1, 2]",
            ),
            (&format!("{fns}range(1, 1001) |> fold(0, add)"), "500500"),
            (
                &format!("{fns}1 |> map(double)\n[1] |> filter(double)\n[1] |> map(add)\nrange(0, 1.0)\nrange(0, 2000000)"),
                "error: `map` expects a List, got Int\n\
                 error: `filter` expects a function that returns a Bool, got Int\n\
                 error: `add` expects 2 arguments, got 1\n\
                 error: `range` expects two Ints, got Int and Float\n\
                 error: `range` can make at most 1000000 numbers, got 2000000",
            ),
        ]);
    }

    #[test]
    fn the_host_calls_and_adds_functions() {
        fn twice(_: &mut Interpreter, args: &[Value]) -> Result<Value, RuntimeError> {
            match &args[0] {
                Value::Int(n) => Ok(Value::Int(n * 2)),
                other => Ok(other.clone()),
            }
        }
        let (mut interpreter, _) = load("add :: (a: Int, b: Int) -> Int { a + b }\nlet n = 1");
        interpreter.define(Native {
            name: "twice",
            arity: 1,
            fun: twice,
        });
        let mut call = |name: &str, args: Vec<Value>| {
            interpreter.call_function(name, args).map_err(|e| e.message)
        };
        assert_eq!(
            call("add", vec![Value::Int(2), Value::Int(3)]),
            Ok(Value::Int(5))
        );
        assert_eq!(call("twice", vec![Value::Int(4)]), Ok(Value::Int(8)));
        assert_eq!(
            call("n", vec![]),
            Err("can only call functions, got Int".into())
        );
        assert_eq!(
            call("nope", vec![]),
            Err("undefined function `nope`".into())
        );
        assert_eq!(interpreter.arity("add"), Some(2));
        assert_eq!(interpreter.arity("twice"), Some(1));
        assert_eq!(interpreter.arity("n"), None);
    }

    fn load_with_ball(source: &str) -> Interpreter {
        let source = format!("Ball :: struct {{ x: Float, y: Float }}\n{source}");
        let mut interpreter = Interpreter::new();
        for stmt in parse(scan_tokens(&source).unwrap()).expect("source should parse") {
            interpreter.execute(&stmt).expect("source should run");
        }
        interpreter
    }

    fn start(source: &str, args: &[&str]) -> (Interpreter, Result<Run, RuntimeError>) {
        let mut interpreter = load_with_ball(source);
        let args = args
            .iter()
            .map(|arg| {
                let expr = parse_expr(scan_tokens(arg).unwrap()).unwrap();
                interpreter.evaluate(&expr).unwrap()
            })
            .collect();
        let run = interpreter.start("s", args);
        (interpreter, run)
    }

    fn ball(x: f64) -> Value {
        Value::Struct(Rc::new(Instance {
            name: "Ball".into(),
            fields: vec![
                ("x".into(), Value::Float(x)),
                ("y".into(), Value::Float(0.0)),
            ],
        }))
    }

    fn x(value: &Value) -> String {
        match value {
            Value::Struct(ball) => ball.fields[0].1.to_string(),
            other => other.to_string(),
        }
    }

    fn frames(source: &str, args: &[&str], dt: f64, count: usize) -> String {
        let (mut interpreter, run) = start(source, args);
        let mut run = run.unwrap();
        let frames: Vec<String> = (0..count)
            .map(|_| {
                interpreter.step(&mut run, dt).expect("step should run");
                let done = if run.is_done() { " done" } else { "" };
                format!("{}{done}", x(&run.state))
            })
            .collect();
        frames.join(", ")
    }

    fn worlds(interpreter: &mut Interpreter, x0: f64, count: usize) -> String {
        let mut world = ball(x0);
        let worlds: Vec<String> = (0..count)
            .map(|_| {
                world = interpreter.run_sequences(world.clone(), 0.25).unwrap();
                x(&world)
            })
            .collect();
        worlds.join(", ")
    }

    fn running(interpreter: &Interpreter) -> Vec<&str> {
        interpreter
            .runs
            .iter()
            .map(|r| r.decl.name.as_str())
            .collect()
    }

    const START: &str = "Ball { x: 0.0, y: 0.0 }";

    #[test]
    fn wait_and_over_take_whole_steps() {
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
            let source = format!(
                "half :: (x: Float) -> Float {{ x / 2.0 }}\ns :: sequence (b: Ball) {{\n  {body}\n}}"
            );
            assert_eq!(frames(&source, &[START], 0.25, count), expected, "{body:?}");
        }

        let at_60 = frames(
            "s :: sequence (b: Ball) {\n  over 0.1s as t {\n    b.x = t\n  }\n}",
            &[START],
            1.0 / 60.0,
            6,
        );
        assert!(at_60.ends_with("0.8333333333333334, 1.0 done"), "{at_60}");

        assert_eq!(
            frames(
                "s :: sequence (b: Ball, to: Float) {\n  let from = b.x\n  over 0.5s as t {\n    let d = to - from\n    b.x = from + d * t\n  }\n}",
                &["Ball { x: 10.0, y: 0.0 }", "20.0"],
                0.25,
                2
            ),
            "15.0, 20.0 done"
        );
    }

    #[test]
    fn reports_errors_in_steps() {
        let cases = [
            (
                "b: Ball",
                "wait 1",
                0.25,
                "a duration must be a Float in seconds, got Int",
                Some(3),
            ),
            (
                "b: Ball",
                "wait -1.0s",
                0.25,
                "a duration must be a finite number of seconds, at least 0s, got -1.0",
                Some(3),
            ),
            (
                "b: Ball",
                "b.z = 1.0",
                0.25,
                "`Ball` has no field `z`",
                Some(3),
            ),
            (
                "b: Int",
                "b.z = 1.0",
                0.25,
                "`b.z = ...` needs `b` to be a struct, got Int",
                Some(3),
            ),
        ];
        for (param, body, dt, message, line) in cases {
            let arg = if param == "b: Int" { "1" } else { START };
            let source = format!("s :: sequence ({param}) {{\n  {body}\n}}");
            let (mut interpreter, run) = start(&source, &[arg]);
            let error = interpreter.step(&mut run.unwrap(), dt).unwrap_err();
            assert_eq!(
                (error.message.as_str(), error.line),
                (message, line),
                "{body:?}"
            );
        }

        let message = |run: Result<Run, RuntimeError>| run.err().unwrap().message;
        assert_eq!(
            message(Interpreter::new().start("s", vec![])),
            "undefined sequence `s`"
        );
        assert_eq!(
            message(start("s :: sequence (n: Int) {}", &[]).1),
            "`s` expects 1 argument, got 0"
        );
    }

    #[test]
    fn running_sequences_step_over_the_current_world() {
        let mut interpreter = load_with_ball(
            "push :: sequence (b: Ball, by: Float) {\n  wait 0.5s\n  b.x = b.x + by\n}\nkick :: (b: Ball) -> Ball {\n  start push(b, 10.0)\n  b\n}",
        );
        interpreter.call_function("kick", vec![ball(0.0)]).unwrap();
        assert_eq!(running(&interpreter), ["push"]);
        assert_eq!(worlds(&mut interpreter, 1.0, 2), "1.0, 11.0");
        assert_eq!(running(&interpreter).len(), 0);

        let mut interpreter = load_with_ball(&format!(
            "s :: sequence (b: Ball) {{\n  over 1.0s {{\n    b.x = b.x + 1.0\n  }}\n}}\nstart s({START})"
        ));
        assert_eq!(
            worlds(&mut interpreter, 100.0, 4),
            "101.0, 102.0, 103.0, 104.0"
        );

        let mut interpreter = load_with_ball(&format!(
            "double :: sequence (b: Ball) {{\n  b.x = b.x * 2.0\n}}\ninc :: sequence (b: Ball) {{\n  b.x = b.x + 1.0\n}}\nstart double({START})\nstart inc({START})"
        ));
        assert_eq!(worlds(&mut interpreter, 3.0, 1), "7.0");

        let mut interpreter = load_with_ball(&format!(
            "second :: sequence (b: Ball) {{\n  b.x = b.x + 10.0\n}}\nfirst :: sequence (b: Ball) {{\n  b.x = 1.0\n  start second(b)\n}}\nstart first({START})"
        ));
        assert_eq!(worlds(&mut interpreter, 0.0, 1), "1.0");
        assert_eq!(running(&interpreter), ["second"]);
        assert_eq!(worlds(&mut interpreter, 1.0, 1), "11.0");
    }

    #[test]
    fn starting_a_running_sequence_replaces_it() {
        let restart = format!("start mark({START}, 2.0)");
        let mut interpreter = load_with_ball(&format!(
            "mark :: sequence (b: Ball, to: Float) {{\n  wait 0.5s\n  b.x = to\n}}\nstart mark({START}, 1.0)"
        ));
        assert_eq!(worlds(&mut interpreter, 0.0, 1), "0.0");
        let stmt = &parse(scan_tokens(&restart).unwrap()).unwrap()[0];
        interpreter.execute(stmt).unwrap();
        assert_eq!(running(&interpreter).len(), 1);
        assert_eq!(worlds(&mut interpreter, 0.0, 2), "0.0, 2.0");
    }

    #[test]
    fn a_sequence_does_not_keep_old_states_alive() {
        let mut interpreter = load_with_ball(&format!(
            "drift :: sequence (b: Ball) {{\n  let from = b.x\n  over 10.0s {{\n    b.x = b.x + 1.0\n  }}\n}}\nstart drift({START})"
        ));
        worlds(&mut interpreter, 0.0, 20);
        let names: Vec<&str> = interpreter.runs[0]
            .locals
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["from"]);
        assert!(interpreter.locals.is_empty());
    }
}
