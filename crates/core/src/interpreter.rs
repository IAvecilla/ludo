use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
}

#[derive(Default)]
pub struct Interpreter;

impl Interpreter {
    pub fn new() -> Self {
        Self
    }

    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Int(v) => Ok(Value::Int(*v)),
            Expr::Float(v) => Ok(Value::Float(*v)),
            Expr::Bool(v) => Ok(Value::Bool(*v)),
            Expr::Str(v) => Ok(Value::Str(v.clone())),
            Expr::Symbol(v) => Ok(Value::Symbol(v.clone())),
            Expr::Ident(name) => Err(error(format!("undefined variable `{name}`"))),
            Expr::Unary(op, right) => self.unary(*op, right),
            Expr::Binary(left, BinaryOp::And, right) => self.and(left, right),
            Expr::Binary(left, BinaryOp::Or, right) => self.or(left, right),
            Expr::Binary(left, op, right) => self.binary(left, *op, right),
            Expr::Call(..) => Err(error("function calls are not supported yet")),
            Expr::Field(..) => Err(error("field access is not supported yet")),
        }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_expr;
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
    fn variables_do_not_exist_yet() {
        assert_eq!(error("x"), "undefined variable `x`");
    }
}
