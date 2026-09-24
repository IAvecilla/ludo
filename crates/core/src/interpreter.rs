use std::rc::Rc;

use crate::ast::{BinaryOp, Expr, FnDecl, Stmt, StmtKind, UnaryOp};
use crate::value::Value;

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
}

impl Interpreter {
    pub fn new() -> Self {
        Self::default()
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
            Expr::Ident(name) => self
                .lookup(name)
                .cloned()
                .ok_or_else(|| error(format!("undefined variable `{name}`"))),
            Expr::Block(stmts, tail) => self.block(stmts, tail),
            Expr::Unary(op, right) => self.unary(*op, right),
            Expr::Binary(left, BinaryOp::And, right) => self.and(left, right),
            Expr::Binary(left, BinaryOp::Or, right) => self.or(left, right),
            Expr::Binary(left, op, right) => self.binary(left, *op, right),
            Expr::Call(callee, args) => self.call(callee, args),
            Expr::Field(..) => Err(error("field access is not supported yet")),
        }
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
            other => {
                return Err(error(format!(
                    "can only call functions, got {}",
                    other.type_name()
                )))
            }
        };

        if args.len() != decl.params.len() {
            return Err(error(format!(
                "`{}` expects {} argument{}, got {}",
                decl.name,
                decl.params.len(),
                if decl.params.len() == 1 { "" } else { "s" },
                args.len()
            )));
        }

        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.evaluate(arg)?);
        }

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
        let mut node = self.env.as_deref();
        while let Some(binding) = node {
            if binding.name == name {
                return Some(&binding.value);
            }
            node = binding.next.as_deref();
        }
        None
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

            (Eq | Ne, Value::Function(_), Value::Function(_)) => {
                Err(error("functions cannot be compared"))
            }

            (Eq, a, b) if a.same_type(&b) => Ok(Bool(a == b)),
            (Ne, a, b) if a.same_type(&b) => Ok(Bool(a != b)),

            (op, a, b) => Err(mismatch(op, &a, &b)),
        }
    }
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
}
