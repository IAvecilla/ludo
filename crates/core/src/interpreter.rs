use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{
    BinaryOp, Expr, FnDecl, SeqDecl, Step, StepKind, Stmt, StmtKind, StructDecl, UnaryOp,
};
use crate::prelude::NATIVES;
use crate::value::{Instance, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
    pub line: Option<usize>,
}

struct Binding {
    name: String,
    value: Value,
    next: Env,
}

type Env = Option<Rc<Binding>>;

const MAX_CALL_DEPTH: usize = 200;

#[derive(Default)]
pub struct Interpreter {
    env: Env,
    globals: Env,
    depth: usize,
    structs: HashMap<String, Rc<StructDecl>>,
    sequences: HashMap<String, Rc<SeqDecl>>,
}

pub struct Run {
    decl: Rc<SeqDecl>,
    env: Env,
    step: usize,
    pass: usize,
    passes: usize,
}

impl Run {
    pub fn state(&self) -> Value {
        lookup(&self.env, &self.decl.params[0].name)
            .cloned()
            .expect("a running sequence always has its state")
    }

    pub fn is_done(&self) -> bool {
        self.step == self.decl.body.len()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let mut interpreter = Self::default();
        for native in NATIVES {
            interpreter.bind(native.name.to_string(), Value::Native(*native));
        }
        interpreter.globals = interpreter.env.clone();
        interpreter
    }

    pub fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, RuntimeError> {
        let result = self.execute_stmt(stmt);
        self.globals = self.env.clone();
        result
    }

    fn execute_stmt(&mut self, stmt: &Stmt) -> Result<Option<Value>, RuntimeError> {
        self.execute_kind(&stmt.kind).map_err(|mut error| {
            error.line.get_or_insert(stmt.line);
            error
        })
    }

    fn execute_kind(&mut self, kind: &StmtKind) -> Result<Option<Value>, RuntimeError> {
        match kind {
            StmtKind::Let(name, value) => {
                let value = self.evaluate(value)?;
                self.bind(name.clone(), value);
                Ok(None)
            }
            StmtKind::Fn(decl) => {
                self.bind(decl.name.clone(), Value::Function(decl.clone()));
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
            StmtKind::Expr(expr) => self.evaluate(expr).map(Some),
        }
    }

    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Int(v) => Ok(Value::Int(*v)),
            Expr::Float(v) => Ok(Value::Float(*v)),
            Expr::Bool(v) => Ok(Value::Bool(*v)),
            Expr::Str(v) => Ok(Value::Str(v.clone())),
            Expr::Symbol(v) => Ok(Value::Symbol(v.clone())),
            Expr::Ident(name) => self.variable(name),
            Expr::Block(stmts, tail) => self.block(stmts, tail),
            Expr::If(cond, then, otherwise) => {
                let branch = match self.evaluate(cond)? {
                    Value::Bool(true) => then,
                    Value::Bool(false) => otherwise,
                    other => {
                        return Err(error(format!(
                            "the condition of `if` must be a Bool, got {}",
                            other.type_name()
                        )))
                    }
                };
                self.evaluate(branch)
            }
            Expr::Unary(op, right) => self.unary(*op, right),
            Expr::Binary(left, BinaryOp::And, right) => self.and(left, right),
            Expr::Binary(left, BinaryOp::Or, right) => self.or(left, right),
            Expr::Binary(left, op, right) => self.binary(left, *op, right),
            Expr::Call(callee, args) => self.call(callee, args),
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
        if self.structs.contains_key(name) {
            return Err(error(format!(
                "`{name}` is a struct, not a value: build one with `{name} {{ ... }}`"
            )));
        }
        if self.sequences.contains_key(name) {
            return Err(error(format!(
                "`{name}` is a sequence, not a function: it runs over time and cannot be called"
            )));
        }
        Err(error(format!("undefined variable `{name}`")))
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

        let mut values: Vec<Option<Value>> = match base {
            Some(base) => match self.evaluate(base)? {
                Value::Struct(instance) if instance.name == name => instance
                    .fields
                    .iter()
                    .map(|(_, v)| Some(v.clone()))
                    .collect(),
                other => {
                    return Err(error(format!(
                        "`{name} {{ x | ... }}` needs a {name} as `x`, got {}",
                        other.type_name()
                    )))
                }
            },
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

        let mut instance = Vec::with_capacity(values.len());
        for (field, value) in decl.fields.iter().zip(values) {
            let value = value
                .ok_or_else(|| error(format!("missing field `{}` in `{name}`", field.name)))?;
            instance.push((field.name.clone(), value));
        }
        Ok(Value::Struct(Rc::new(Instance {
            name: name.to_string(),
            fields: instance,
        })))
    }

    fn block(&mut self, stmts: &[Stmt], tail: &Expr) -> Result<Value, RuntimeError> {
        let outer = self.env.clone();
        let result = self.run_block(stmts, tail);
        self.env = outer;
        result
    }

    fn run_block(&mut self, stmts: &[Stmt], tail: &Expr) -> Result<Value, RuntimeError> {
        for stmt in stmts {
            self.execute_stmt(stmt)?;
        }
        self.evaluate(tail)
    }

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Value, RuntimeError> {
        let decl = match self.evaluate(callee)? {
            Value::Function(decl) => decl,
            Value::Native(native) => {
                check_arity(native.name, native.arity, args.len())?;
                let values = self.arguments(args)?;
                return (native.fun)(&values).map_err(error);
            }
            other => {
                return Err(error(format!(
                    "can only call functions, got {}",
                    other.type_name()
                )))
            }
        };

        check_arity(&decl.name, decl.params.len(), args.len())?;
        let values = self.arguments(args)?;

        if self.depth == MAX_CALL_DEPTH {
            return Err(error(format!(
                "stack overflow: more than {MAX_CALL_DEPTH} nested calls"
            )));
        }

        let caller = std::mem::replace(&mut self.env, self.globals.clone());
        self.depth += 1;
        let result = self.run_function(&decl, values);
        self.depth -= 1;
        self.env = caller;
        result
    }

    fn arguments(&mut self, args: &[Expr]) -> Result<Vec<Value>, RuntimeError> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.evaluate(arg)?);
        }
        Ok(values)
    }

    pub fn start(&mut self, name: &str, args: Vec<Value>) -> Result<Run, RuntimeError> {
        let decl = self
            .sequences
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("undefined sequence `{name}`")))?;
        check_arity(&decl.name, decl.params.len(), args.len())?;

        let mut env = self.globals.clone();
        for (param, value) in decl.params.iter().zip(args) {
            env = Some(Rc::new(Binding {
                name: param.name.clone(),
                value,
                next: env,
            }));
        }
        Ok(Run {
            decl,
            env,
            step: 0,
            pass: 0,
            passes: 0,
        })
    }

    pub fn step(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        if dt.is_nan() || dt <= 0.0 {
            return Err(error(format!("a step must last more than 0s, got {dt:?}")));
        }
        let outer = std::mem::replace(&mut self.env, run.env.take());
        let result = self.advance(run, dt);
        run.env = std::mem::replace(&mut self.env, outer);
        result
    }

    fn advance(&mut self, run: &mut Run, dt: f64) -> Result<(), RuntimeError> {
        let decl = run.decl.clone();
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

            if run.pass == 0 {
                run.passes = passes(self.evaluate(duration), dt).map_err(|mut e| {
                    e.line.get_or_insert(step.line);
                    e
                })?;
            }
            run.pass += 1;
            let t = if run.pass == run.passes {
                1.0
            } else {
                run.pass as f64 / run.passes as f64
            };
            self.over_pass(&decl.params[0].name, var.as_deref(), t, body)?;

            if run.pass == run.passes {
                run.pass = 0;
                run.step += 1;
                while let Some(step) = decl.body.get(run.step) {
                    if matches!(step.kind, StepKind::Over { .. }) {
                        break;
                    }
                    self.execute_step(step)?;
                    run.step += 1;
                }
            }
            return Ok(());
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
        let outer = self.env.clone();
        if let Some(var) = var {
            self.bind(var.to_string(), Value::Float(t));
        }
        let result = body.iter().try_for_each(|step| self.execute_step(step));
        let value = self.lookup(state).cloned();
        self.env = outer;
        result?;
        if let Some(value) = value {
            self.bind(state.to_string(), value);
        }
        Ok(())
    }

    fn execute_step(&mut self, step: &Step) -> Result<(), RuntimeError> {
        let result = match &step.kind {
            StepKind::Stmt(kind) => self.execute_kind(kind).map(|_| ()),
            StepKind::Set(target, field, expr) => self.set_field(target, field, expr),
            StepKind::Over { .. } => unreachable!("the parser keeps `over` out of `over`"),
        };
        result.map_err(|mut error| {
            error.line.get_or_insert(step.line);
            error
        })
    }

    fn set_field(&mut self, target: &str, field: &str, expr: &Expr) -> Result<(), RuntimeError> {
        let value = self.evaluate(expr)?;
        let instance = match self.lookup(target) {
            Some(Value::Struct(instance)) => instance.clone(),
            Some(other) => {
                return Err(error(format!(
                    "`{target}.{field} = ...` needs `{target}` to be a struct, got {}",
                    other.type_name()
                )))
            }
            None => unreachable!("a running sequence always has its state"),
        };
        let mut updated = (*instance).clone();
        match updated.fields.iter_mut().find(|(f, _)| f == field) {
            Some((_, slot)) => *slot = value,
            None => return Err(error(format!("`{}` has no field `{field}`", instance.name))),
        }
        self.bind(target.to_string(), Value::Struct(Rc::new(updated)));
        Ok(())
    }

    fn run_function(&mut self, decl: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        for (param, value) in decl.params.iter().zip(args) {
            self.bind(param.name.clone(), value);
        }
        self.evaluate(&decl.body)
    }

    fn bind(&mut self, name: String, value: Value) {
        let next = self.env.take();
        self.env = Some(Rc::new(Binding { name, value, next }));
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        lookup(&self.env, name)
    }

    fn unary(&mut self, op: UnaryOp, right: &Expr) -> Result<Value, RuntimeError> {
        let right = self.evaluate(right)?;
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
                "operand of `not` must be a Bool, got {}",
                v.type_name()
            ))),
        }
    }

    fn and(&mut self, left: &Expr, right: &Expr) -> Result<Value, RuntimeError> {
        if !self.boolean(left, BinaryOp::And)? {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Bool(self.boolean(right, BinaryOp::And)?))
    }

    fn or(&mut self, left: &Expr, right: &Expr) -> Result<Value, RuntimeError> {
        if self.boolean(left, BinaryOp::Or)? {
            return Ok(Value::Bool(true));
        }
        Ok(Value::Bool(self.boolean(right, BinaryOp::Or)?))
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

    fn binary(&mut self, left: &Expr, op: BinaryOp, right: &Expr) -> Result<Value, RuntimeError> {
        let left = self.evaluate(left)?;
        let right = self.evaluate(right)?;

        use BinaryOp::*;
        use Value::{Bool, Float, Int};

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

            (Lt, Int(a), Int(b)) => Ok(Bool(a < b)),
            (Le, Int(a), Int(b)) => Ok(Bool(a <= b)),
            (Gt, Int(a), Int(b)) => Ok(Bool(a > b)),
            (Ge, Int(a), Int(b)) => Ok(Bool(a >= b)),

            (Lt, Float(a), Float(b)) => Ok(Bool(a < b)),
            (Le, Float(a), Float(b)) => Ok(Bool(a <= b)),
            (Gt, Float(a), Float(b)) => Ok(Bool(a > b)),
            (Ge, Float(a), Float(b)) => Ok(Bool(a >= b)),

            (
                Eq | Ne,
                Value::Function(_) | Value::Native(_),
                Value::Function(_) | Value::Native(_),
            ) => Err(error("functions cannot be compared")),

            (Eq, a, b) if a.same_type(&b) => Ok(Bool(a == b)),
            (Ne, a, b) if a.same_type(&b) => Ok(Bool(a != b)),

            (op, a, b) => Err(mismatch(op, &a, &b)),
        }
    }
}

fn lookup<'a>(env: &'a Env, name: &str) -> Option<&'a Value> {
    let mut node = env.as_deref();
    while let Some(binding) = node {
        if binding.name == name {
            return Some(&binding.value);
        }
        node = binding.next.as_deref();
    }
    None
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

fn mismatch(op: BinaryOp, a: &Value, b: &Value) -> RuntimeError {
    let expected = match op {
        BinaryOp::Eq | BinaryOp::Ne => "two values of the same type",
        _ => "two Ints or two Floats",
    };
    error(format!(
        "operands of `{op}` must be {expected}, got {} and {}",
        a.type_name(),
        b.type_name()
    ))
}

fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        line: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse, parse_expr};
    use crate::scanner::scan_tokens;

    fn run(source: &str) -> Result<Value, RuntimeError> {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        let expr = parse_expr(tokens).expect("source should parse cleanly");
        Interpreter::new().evaluate(&expr)
    }

    fn eval(source: &str) -> String {
        run(source)
            .unwrap_or_else(|e| panic!("source should evaluate cleanly: {}", e.message))
            .to_string()
    }

    fn error(source: &str) -> String {
        run(source).unwrap_err().message
    }

    #[test]
    fn evaluates_literals() {
        assert_eq!(eval("42"), "42");
        assert_eq!(eval("1.5"), "1.5");
        assert_eq!(eval("true"), "true");
        assert_eq!(eval(r#""hi""#), "hi");
        assert_eq!(eval(":jump"), ":jump");
    }

    #[test]
    fn a_duration_is_a_float() {
        assert_eq!(eval("0.2s * 2.0"), "0.4");
    }

    #[test]
    fn respects_precedence() {
        assert_eq!(eval("1 + 2 * 3"), "7");
        assert_eq!(eval("(1 + 2) * 3"), "9");
        assert_eq!(eval("10 - 4 - 3"), "3");
    }

    #[test]
    fn int_division_truncates() {
        assert_eq!(eval("7 / 2"), "3");
        assert_eq!(eval("7 % 2"), "1");
    }

    #[test]
    fn float_arithmetic() {
        assert_eq!(eval("7.0 / 2.0"), "3.5");
        assert_eq!(eval("0.5 * 4.0"), "2.0");
    }

    #[test]
    fn ints_and_floats_do_not_mix() {
        assert_eq!(
            error("1 + 2.0"),
            "operands of `+` must be two Ints or two Floats, got Int and Float"
        );
    }

    #[test]
    fn strings_cannot_be_added() {
        assert_eq!(
            error(r#""a" + "b""#),
            "operands of `+` must be two Ints or two Floats, got String and String"
        );
    }

    #[test]
    fn int_division_by_zero_is_an_error() {
        assert_eq!(error("1 / 0"), "division by zero");
        assert_eq!(error("1 % 0"), "division by zero");
    }

    #[test]
    fn float_division_by_zero_is_infinity() {
        assert_eq!(eval("1.0 / 0.0"), "inf");
    }

    #[test]
    fn int_overflow_is_an_error() {
        assert_eq!(error("9223372036854775807 + 1"), "integer overflow in `+`");
    }

    #[test]
    fn negation() {
        assert_eq!(eval("-3"), "-3");
        assert_eq!(eval("--3"), "3");
        assert_eq!(eval("-1.5"), "-1.5");
        assert_eq!(
            error("-true"),
            "operand of `-` must be an Int or a Float, got Bool"
        );
    }

    #[test]
    fn comparison() {
        assert_eq!(eval("1 < 2"), "true");
        assert_eq!(eval("2.5 >= 2.5"), "true");
        assert_eq!(
            error(r#""a" < "b""#),
            "operands of `<` must be two Ints or two Floats, got String and String"
        );
    }

    #[test]
    fn equality_requires_the_same_type() {
        assert_eq!(eval("1 == 1"), "true");
        assert_eq!(eval(r#""a" != "b""#), "true");
        assert_eq!(eval(":jump == :jump"), "true");
        assert_eq!(
            error("1 == 1.0"),
            "operands of `==` must be two values of the same type, got Int and Float"
        );
    }

    #[test]
    fn not_requires_a_bool() {
        assert_eq!(eval("not true"), "false");
        assert_eq!(error("not 0"), "operand of `not` must be a Bool, got Int");
    }

    #[test]
    fn and_or_short_circuit() {
        assert_eq!(eval("false and 1 / 0 == 1"), "false");
        assert_eq!(eval("true or 1 / 0 == 1"), "true");
    }

    #[test]
    fn and_or_require_bools() {
        assert_eq!(
            error("1 and true"),
            "operands of `and` must be Bool, got Int"
        );
        assert_eq!(
            error("false or 1"),
            "operands of `or` must be Bool, got Int"
        );
    }

    #[test]
    fn an_unbound_name_is_an_error() {
        assert_eq!(error("x"), "undefined variable `x`");
    }

    fn session(lines: &[&str]) -> Vec<Result<Option<String>, String>> {
        let mut interpreter = Interpreter::new();
        lines
            .iter()
            .map(|line| {
                let tokens = scan_tokens(line).expect("source should scan cleanly");
                let stmts = parse(tokens).expect("source should parse cleanly");
                interpreter
                    .execute(&stmts[0])
                    .map(|value| value.map(|v| v.to_string()))
                    .map_err(|e| e.message)
            })
            .collect()
    }

    #[test]
    fn let_binds_a_name_for_later_lines() {
        assert_eq!(
            session(&["let x = 10", "let y = x * 2", "y + 1"]),
            vec![Ok(None), Ok(None), Ok(Some("21".into()))]
        );
    }

    #[test]
    fn let_shadows_using_the_previous_value() {
        assert_eq!(
            session(&["let x = 1", "let x = x + 1", "x"]),
            vec![Ok(None), Ok(None), Ok(Some("2".into()))]
        );
    }

    #[test]
    fn shadowing_can_change_the_type() {
        assert_eq!(
            session(&["let x = 1", "let x = :one", "x"]),
            vec![Ok(None), Ok(None), Ok(Some(":one".into()))]
        );
    }

    #[test]
    fn a_failed_let_binds_nothing() {
        assert_eq!(
            session(&["let x = 1 / 0", "x"]),
            vec![
                Err("division by zero".into()),
                Err("undefined variable `x`".into())
            ]
        );
    }

    #[test]
    fn a_failed_let_keeps_the_previous_binding() {
        assert_eq!(
            session(&["let x = 1", "let x = 1 / 0", "x"]),
            vec![
                Ok(None),
                Err("division by zero".into()),
                Ok(Some("1".into()))
            ]
        );
    }

    #[test]
    fn a_runtime_error_records_the_line_of_its_statement() {
        let tokens = scan_tokens("let x = 1\n\nlet y = x / 0").unwrap();
        let stmts = parse(tokens).unwrap();
        let mut interpreter = Interpreter::new();
        interpreter.execute(&stmts[0]).unwrap();
        let error = interpreter.execute(&stmts[1]).unwrap_err();
        assert_eq!(error.line, Some(3));
    }

    #[test]
    fn a_block_evaluates_to_its_last_expression() {
        assert_eq!(
            session(&["{\n  let w = 4\n  let h = 5\n  w * h\n}"]),
            vec![Ok(Some("20".into()))]
        );
    }

    #[test]
    fn names_bound_in_a_block_do_not_leak() {
        assert_eq!(
            session(&["let area = {\n  let w = 4\n  w * 5\n}", "area", "w"]),
            vec![
                Ok(None),
                Ok(Some("20".into())),
                Err("undefined variable `w`".into())
            ]
        );
    }

    #[test]
    fn a_block_reads_the_outer_scope() {
        assert_eq!(
            session(&["let x = 10", "{ x + 1 }"]),
            vec![Ok(None), Ok(Some("11".into()))]
        );
    }

    #[test]
    fn shadowing_inside_a_block_does_not_touch_the_outside() {
        assert_eq!(
            session(&["let x = 1", "let y = {\n  let x = 10\n  x + 1\n}", "y", "x"]),
            vec![
                Ok(None),
                Ok(None),
                Ok(Some("11".into())),
                Ok(Some("1".into()))
            ]
        );
    }

    #[test]
    fn nested_blocks_see_every_enclosing_scope() {
        assert_eq!(
            session(&["let a = 1", "{\n  let b = 2\n  { a + b }\n}"]),
            vec![Ok(None), Ok(Some("3".into()))]
        );
    }

    #[test]
    fn a_failing_block_still_restores_the_outer_scope() {
        assert_eq!(
            session(&["let x = 1", "{\n  let x = 2\n  x / 0\n}", "x"]),
            vec![
                Ok(None),
                Err("division by zero".into()),
                Ok(Some("1".into()))
            ]
        );
    }

    fn runtime_error_line(source: &str) -> Option<usize> {
        let tokens = scan_tokens(source).unwrap();
        let stmts = parse(tokens).unwrap();
        Interpreter::new().execute(&stmts[0]).unwrap_err().line
    }

    #[test]
    fn a_failing_statement_inside_a_block_reports_its_own_line() {
        assert_eq!(
            runtime_error_line("let y = {\n  let x = 1 / 0\n  x\n}"),
            Some(2)
        );
    }

    #[test]
    fn a_failing_tail_expression_reports_the_line_of_the_enclosing_statement() {
        assert_eq!(
            runtime_error_line("let y = {\n  let x = 1\n  x / 0\n}"),
            Some(1)
        );
    }

    fn program(source: &str) -> Vec<Result<Option<String>, String>> {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        let stmts = parse(tokens).expect("source should parse cleanly");
        let mut interpreter = Interpreter::new();
        stmts
            .iter()
            .map(|stmt| {
                interpreter
                    .execute(stmt)
                    .map(|value| value.map(|v| v.to_string()))
                    .map_err(|e| e.message)
            })
            .collect()
    }

    fn last(source: &str) -> Result<Option<String>, String> {
        program(source).pop().expect("program should not be empty")
    }

    #[test]
    fn calls_a_function() {
        assert_eq!(
            last("fn add(a: Int, b: Int) -> Int { a + b }\nadd(2, 3)"),
            Ok(Some("5".into()))
        );
    }

    #[test]
    fn a_declaration_produces_no_value() {
        assert_eq!(program("fn one() -> Int { 1 }"), vec![Ok(None)]);
    }

    #[test]
    fn call_pipe_and_dot_are_the_same_call() {
        let source = "fn add(a: Int, b: Int) -> Int { a + b }";
        for call in ["add(2, 3)", "2 |> add(3)", "2.add(3)"] {
            assert_eq!(last(&format!("{source}\n{call}")), Ok(Some("5".into())));
        }
    }

    #[test]
    fn pipes_chain_through_functions() {
        assert_eq!(
            last("fn double(x: Int) -> Int { x * 2 }\nfn inc(x: Int) -> Int { x + 1 }\n3\n  |> double\n  |> inc"),
            Ok(Some("7".into()))
        );
    }

    #[test]
    fn a_function_body_can_have_several_lines() {
        assert_eq!(
            last("fn area(w: Int, h: Int) -> Int {\n  let a = w * h\n  a\n}\narea(4, 5)"),
            Ok(Some("20".into()))
        );
    }

    #[test]
    fn functions_can_recurse_through_each_other() {
        let source = "fn is_even(n: Int) -> Bool { n == 0 or is_odd(n - 1) }\n\
                      fn is_odd(n: Int) -> Bool { n != 0 and is_even(n - 1) }";
        assert_eq!(
            last(&format!("{source}\nis_even(10)")),
            Ok(Some("true".into()))
        );
        assert_eq!(
            last(&format!("{source}\nis_odd(7)")),
            Ok(Some("true".into()))
        );
        assert_eq!(
            last(&format!("{source}\nis_even(7)")),
            Ok(Some("false".into()))
        );
    }

    #[test]
    fn a_function_sees_top_level_names_declared_before_the_call() {
        assert_eq!(
            last("fn scaled(x: Int) -> Int { x * SCALE }\nlet SCALE = 10\nscaled(3)"),
            Ok(Some("30".into()))
        );
    }

    #[test]
    fn a_function_does_not_see_the_callers_locals() {
        assert_eq!(
            last("fn peek() -> Int { secret }\n{\n  let secret = 1\n  peek()\n}"),
            Err("undefined variable `secret`".into())
        );
    }

    #[test]
    fn parameters_shadow_top_level_names() {
        assert_eq!(
            last("let x = 100\nfn id(x: Int) -> Int { x }\nid(1)"),
            Ok(Some("1".into()))
        );
    }

    #[test]
    fn a_call_does_not_leak_its_parameters() {
        assert_eq!(
            last("fn id(y: Int) -> Int { y }\nid(1)\ny"),
            Err("undefined variable `y`".into())
        );
    }

    #[test]
    fn functions_are_values() {
        assert_eq!(
            last("fn double(x: Int) -> Int { x * 2 }\nlet g = double\ng(4)"),
            Ok(Some("8".into()))
        );
        assert_eq!(
            last("fn double(x: Int) -> Int { x * 2 }\nfn twice(f: Fn, x: Int) -> Int { f(f(x)) }\ntwice(double, 3)"),
            Ok(Some("12".into()))
        );
        assert_eq!(
            last("fn double(x: Int) -> Int { x * 2 }\ndouble"),
            Ok(Some("<fn double>".into()))
        );
    }

    #[test]
    fn checks_the_number_of_arguments() {
        assert_eq!(
            last("fn add(a: Int, b: Int) -> Int { a + b }\nadd(1)"),
            Err("`add` expects 2 arguments, got 1".into())
        );
        assert_eq!(
            last("fn id(x: Int) -> Int { x }\nid(1, 2)"),
            Err("`id` expects 1 argument, got 2".into())
        );
    }

    #[test]
    fn only_functions_can_be_called() {
        assert_eq!(
            last("let x = 1\nx(2)"),
            Err("can only call functions, got Int".into())
        );
    }

    #[test]
    fn functions_cannot_be_compared() {
        assert_eq!(
            last("fn f() -> Int { 1 }\nf == f"),
            Err("functions cannot be compared".into())
        );
    }

    #[test]
    fn infinite_recursion_is_an_error_not_a_crash() {
        assert_eq!(
            last("fn loop_forever(n: Int) -> Int { loop_forever(n + 1) }\nloop_forever(0)"),
            Err(format!(
                "stack overflow: more than {MAX_CALL_DEPTH} nested calls"
            ))
        );
    }

    #[test]
    fn the_scope_is_restored_after_a_failing_call() {
        assert_eq!(
            program("let x = 1\nfn boom(x: Int) -> Int { x / 0 }\nboom(5)\nx"),
            vec![
                Ok(None),
                Ok(None),
                Err("division by zero".into()),
                Ok(Some("1".into()))
            ]
        );
    }

    #[test]
    fn if_picks_a_branch() {
        assert_eq!(eval("if 1 < 2 { :yes } else { :no }"), ":yes");
        assert_eq!(eval("if 1 > 2 { :yes } else { :no }"), ":no");
    }

    #[test]
    fn the_branch_not_taken_is_not_evaluated() {
        assert_eq!(eval("if true { 1 } else { 1 / 0 }"), "1");
        assert_eq!(eval("if false { 1 / 0 } else { 2 }"), "2");
    }

    #[test]
    fn else_if_chains() {
        assert_eq!(
            last("fn sign(n: Int) -> Symbol {\n  if n < 0 { :neg } else if n == 0 { :zero } else { :pos }\n}\nsign(0)"),
            Ok(Some(":zero".into()))
        );
    }

    #[test]
    fn the_condition_must_be_a_bool() {
        assert_eq!(
            error("if 1 { 2 } else { 3 }"),
            "the condition of `if` must be a Bool, got Int"
        );
    }

    #[test]
    fn recursion_terminates_with_if() {
        assert_eq!(
            last("fn fact(n: Int) -> Int { if n == 0 { 1 } else { n * fact(n - 1) } }\nfact(10)"),
            Ok(Some("3628800".into()))
        );
        assert_eq!(
            last("fn fib(n: Int) -> Int { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\nfib(15)"),
            Ok(Some("610".into()))
        );
    }

    const BALL: &str = "struct Ball { x: Float, y: Float }\n";

    #[test]
    fn builds_a_struct() {
        assert_eq!(
            last(&format!("{BALL}Ball {{ y: 2.0, x: 1.0 }}")),
            Ok(Some("Ball { x: 1.0, y: 2.0 }".into()))
        );
        assert_eq!(
            last("struct Label { text: String }\nLabel { text: \"hi\" }"),
            Ok(Some("Label { text: \"hi\" }".into()))
        );
        assert_eq!(
            last("struct Empty {}\nEmpty {}"),
            Ok(Some("Empty {}".into()))
        );
    }

    #[test]
    fn reads_a_field() {
        assert_eq!(
            last(&format!("{BALL}let b = Ball {{ x: 1.0, y: 2.0 }}\nb.y")),
            Ok(Some("2.0".into()))
        );
    }

    #[test]
    fn updates_a_struct_without_changing_the_original() {
        assert_eq!(
            program(&format!(
                "{BALL}let b = Ball {{ x: 1.0, y: 2.0 }}\nBall {{ b | y: 5.0 }}\nb"
            ))[2..],
            [
                Ok(Some("Ball { x: 1.0, y: 5.0 }".into())),
                Ok(Some("Ball { x: 1.0, y: 2.0 }".into()))
            ]
        );
    }

    #[test]
    fn structs_compare_by_value() {
        assert_eq!(
            last(&format!(
                "{BALL}Ball {{ x: 1.0, y: 2.0 }} == Ball {{ y: 2.0, x: 1.0 }}"
            )),
            Ok(Some("true".into()))
        );
        assert_eq!(
            last(&format!(
                "{BALL}struct Paddle {{ y: Float }}\nBall {{ x: 1.0, y: 2.0 }} == Paddle {{ y: 2.0 }}"
            )),
            Err("operands of `==` must be two values of the same type, got Ball and Paddle".into())
        );
    }

    #[test]
    fn struct_errors() {
        let cases = [
            ("Ball { x: 1.0 }", "missing field `y` in `Ball`"),
            ("Ball { x: 1.0, y: 2.0, z: 3.0 }", "`Ball` has no field `z`"),
            ("Paddle { y: 1.0 }", "undefined struct `Paddle`"),
            (
                "Ball { 1 | x: 2.0 }",
                "`Ball { x | ... }` needs a Ball as `x`, got Int",
            ),
            ("Ball { x: 1.0, y: 2.0 }.z", "`Ball` has no field `z`"),
            ("let n = 1\nn.x", "Int has no fields"),
            (
                "Ball",
                "`Ball` is a struct, not a value: build one with `Ball { ... }`",
            ),
        ];
        for (source, message) in cases {
            assert_eq!(
                last(&format!("{BALL}{source}")),
                Err(message.into()),
                "{source}"
            );
        }
    }

    #[test]
    fn a_struct_can_be_redeclared() {
        assert_eq!(
            last(&format!(
                "{BALL}struct Ball {{ r: Float }}\nBall {{ r: 1.0 }}"
            )),
            Ok(Some("Ball { r: 1.0 }".into()))
        );
    }

    #[test]
    fn functions_take_and_return_structs() {
        assert_eq!(
            last(&format!(
                "{BALL}fn fall(b: Ball, dy: Float) -> Ball {{ Ball {{ b | y: b.y + dy }} }}\nBall {{ x: 0.0, y: 0.0 }} |> fall(1.0) |> fall(2.0)"
            )),
            Ok(Some("Ball { x: 0.0, y: 3.0 }".into()))
        );
    }

    #[test]
    fn calls_natives() {
        let cases = [
            ("abs(-3)", "3"),
            ("abs(-1.5)", "1.5"),
            ("min(2, 3)", "2"),
            ("max(2.0, 3.0)", "3.0"),
            ("clamp(5, 0, 3)", "3"),
            ("clamp(-1.0, 0.0, 1.0)", "0.0"),
            ("sqrt(9.0)", "3.0"),
            ("sin(0.0)", "0.0"),
            ("cos(0.0)", "1.0"),
            ("to_float(2)", "2.0"),
            ("to_int(-2.7)", "-2"),
            ("3 |> max(4)", "4"),
        ];
        for (source, value) in cases {
            assert_eq!(last(source), Ok(Some(value.into())), "{source}");
        }
    }

    #[test]
    fn native_errors() {
        let cases = [
            ("abs(true)", "`abs` expects an Int or a Float, got Bool"),
            ("abs(-9223372036854775807 - 1)", "integer overflow in `abs`"),
            (
                "min(1, 2.0)",
                "`min` expects all Ints or all Floats, got Int, Float",
            ),
            (
                "clamp(1, 3, 0)",
                "`clamp` expects its lower bound to be at most its upper bound",
            ),
            ("sqrt(-1.0)", "`sqrt` of a negative number"),
            ("sqrt(4)", "`sqrt` expects a Float, got Int"),
            (
                "to_int(1.0 / 0.0)",
                "`to_int` cannot represent inf as an Int",
            ),
            ("min(1)", "`min` expects 2 arguments, got 1"),
            ("abs == abs", "functions cannot be compared"),
        ];
        for (source, message) in cases {
            assert_eq!(last(source), Err(message.into()), "{source}");
        }
    }

    #[test]
    fn natives_are_ordinary_names() {
        assert_eq!(last("abs"), Ok(Some("<fn abs>".into())));
        assert_eq!(last("let abs = 1\nabs + 1"), Ok(Some("2".into())));
        assert_eq!(
            last("fn f(x: Int) -> Int { abs(x) }\nf(-4)"),
            Ok(Some("4".into()))
        );
    }

    fn sequence(source: &str, name: &str, args: &[&str]) -> (Interpreter, Run) {
        let mut interpreter = Interpreter::new();
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        for stmt in parse(tokens).expect("source should parse cleanly") {
            interpreter
                .execute(&stmt)
                .expect("source should run cleanly");
        }
        let args = args
            .iter()
            .map(|arg| {
                let tokens = scan_tokens(arg).expect("argument should scan cleanly");
                let expr = parse_expr(tokens).expect("argument should parse cleanly");
                interpreter
                    .evaluate(&expr)
                    .expect("argument should evaluate")
            })
            .collect();
        let run = interpreter
            .start(name, args)
            .expect("sequence should start");
        (interpreter, run)
    }

    fn frames(source: &str, args: &[&str], dt: f64, count: usize) -> Vec<String> {
        let (mut interpreter, mut run) = sequence(&format!("{BALL}{source}"), "s", args);
        (0..count)
            .map(|_| {
                interpreter
                    .step(&mut run, dt)
                    .expect("step should run cleanly");
                let done = if run.is_done() { " done" } else { "" };
                match run.state() {
                    Value::Struct(ball) => format!("{}{done}", ball.fields[0].1),
                    other => format!("{other}{done}"),
                }
            })
            .collect()
    }

    const START: &str = "Ball { x: 0.0, y: 0.0 }";

    #[test]
    fn a_sequence_without_time_finishes_in_one_step() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  b.x = 1.0\n  b.x = b.x + 1.0\n}",
                &[START],
                0.25,
                1
            ),
            ["2.0 done"]
        );
    }

    #[test]
    fn wait_takes_as_many_steps_as_its_duration() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  wait 1.0s\n  b.x = 5.0\n}",
                &[START],
                0.25,
                4
            ),
            ["0.0", "0.0", "0.0", "5.0 done"]
        );
    }

    #[test]
    fn over_runs_its_body_once_per_step_with_t() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  over 1.0s as t {\n    b.x = t\n  }\n}",
                &[START],
                0.25,
                4
            ),
            ["0.25", "0.5", "0.75", "1.0 done"]
        );
    }

    #[test]
    fn over_rounds_up_and_ends_on_exactly_one() {
        let steps = frames(
            "sequence s(b: Ball) {\n  over 0.1s as t {\n    b.x = t\n  }\n}",
            &[START],
            1.0 / 60.0,
            6,
        );
        assert_eq!(steps[5], "1.0 done");
        assert!(!steps[4].ends_with("done"));
        let steps = frames(
            "sequence s(b: Ball) {\n  over 0.3s as t {\n    b.x = t\n  }\n}",
            &[START],
            0.25,
            2,
        );
        assert_eq!(steps, ["0.5", "1.0 done"]);
    }

    #[test]
    fn a_zero_duration_still_takes_one_step() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  wait 0.0s\n  b.x = 1.0\n}",
                &[START],
                0.25,
                1
            ),
            ["1.0 done"]
        );
    }

    #[test]
    fn steps_between_waits_run_in_the_step_that_reaches_them() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  b.x = 1.0\n  wait 0.5s\n  b.x = 2.0\n  wait 0.25s\n  b.x = 3.0\n}",
                &[START],
                0.25,
                3
            ),
            ["1.0", "2.0", "3.0 done"]
        );
    }

    #[test]
    fn bindings_live_across_steps() {
        assert_eq!(
            frames(
                "sequence s(b: Ball, to: Float) {\n  let from = b.x\n  over 0.5s as t {\n    let d = to - from\n    b.x = from + d * t\n  }\n}",
                &["Ball { x: 10.0, y: 0.0 }", "20.0"],
                0.25,
                2
            ),
            ["15.0", "20.0 done"]
        );
    }

    #[test]
    fn a_finished_sequence_stays_finished() {
        assert_eq!(
            frames(
                "sequence s(b: Ball) {\n  b.x = b.x + 1.0\n}",
                &[START],
                0.25,
                3
            ),
            ["1.0 done", "1.0 done", "1.0 done"]
        );
    }

    #[test]
    fn a_sequence_can_call_functions() {
        assert_eq!(
            frames(
                "fn half(x: Float) -> Float { x / 2.0 }\nsequence s(b: Ball) {\n  over 0.5s as t {\n    b.x = half(t) |> max(0.3)\n  }\n}",
                &[START],
                0.25,
                2
            ),
            ["0.3", "0.5 done"]
        );
    }

    fn step_error(source: &str, args: &[&str], dt: f64) -> RuntimeError {
        let (mut interpreter, mut run) = sequence(&format!("{BALL}{source}"), "s", args);
        interpreter.step(&mut run, dt).unwrap_err()
    }

    #[test]
    fn sequence_errors() {
        let error = step_error("sequence s(b: Ball) {\n  wait 1\n}", &[START], 0.25);
        assert_eq!(
            error.message,
            "a duration must be a Float in seconds, got Int"
        );
        assert_eq!(error.line, Some(3));

        let error = step_error("sequence s(b: Ball) {\n  wait -1.0s\n}", &[START], 0.25);
        assert_eq!(
            error.message,
            "a duration must be a finite number of seconds, at least 0s, got -1.0"
        );

        let error = step_error("sequence s(b: Ball) {\n  b.z = 1.0\n}", &[START], 0.25);
        assert_eq!(error.message, "`Ball` has no field `z`");
        assert_eq!(error.line, Some(3));

        let error = step_error("sequence s(n: Int) {\n  n.z = 1.0\n}", &["1"], 0.25);
        assert_eq!(
            error.message,
            "`n.z = ...` needs `n` to be a struct, got Int"
        );

        let error = step_error("sequence s(b: Ball) {}", &[START], 0.0);
        assert_eq!(error.message, "a step must last more than 0s, got 0.0");
    }

    #[test]
    fn starting_errors() {
        let mut interpreter = Interpreter::new();
        assert_eq!(
            interpreter.start("s", vec![]).err().unwrap().message,
            "undefined sequence `s`"
        );
        let (mut interpreter, _) = sequence("sequence s(n: Int) {}", "s", &["1"]);
        assert_eq!(
            interpreter.start("s", vec![]).err().unwrap().message,
            "`s` expects 1 argument, got 0"
        );
    }

    #[test]
    fn a_sequence_is_not_a_function() {
        assert_eq!(
            last("sequence s(n: Int) {}\ns(1)"),
            Err("`s` is a sequence, not a function: it runs over time and cannot be called".into())
        );
    }
}
