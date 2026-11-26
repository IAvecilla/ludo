//! What values do when a program runs, apart from the VM that runs it:
//! runtime errors, operators, fields, structs, patterns and the timing of
//! `over`. The checker shares the hints for names used as the wrong thing.

use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp, Variant};
use crate::chunk::Test;
use crate::value::{Instance, Layout, Value};

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

pub(crate) fn struct_hint(name: &str) -> String {
    format!("`{name}` is a struct, not a value: build one with `{name} {{ ... }}`")
}

pub(crate) fn sequence_hint(name: &str) -> String {
    format!("`{name}` is a sequence, not a function: it runs over time and cannot be called")
}

pub(crate) fn enum_hint(name: &str, symbol: &str) -> String {
    format!("`{name}` is an enum, not a value: its values are its symbols, like `:{symbol}`")
}

/// A function written where it is used has no name, so its error says "the
/// function".
pub(crate) fn check_arity(name: &str, expected: usize, got: usize) -> Result<(), RuntimeError> {
    if expected == got {
        return Ok(());
    }
    let who = match name {
        "" => "the function".to_string(),
        name => format!("`{name}`"),
    };
    Err(error(format!(
        "{who} expects {expected} argument{}, got {got}",
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

// The checker has made sure that fields are only read and set on structs
// that have them, and that a literal names fields of its struct, all of them
// when it updates no other.

/// The value of a field of a struct.
pub(crate) fn field(target: &Value, name: &str) -> Value {
    let Value::Struct(instance) = target else {
        unreachable!("only structs have fields")
    };
    instance
        .get(name)
        .expect("the struct has the field")
        .clone()
}

/// Builds a struct with this `layout` from the `values` a literal gives to
/// the fields it `names`, and those of `base` for the rest, if it updates
/// one.
pub(crate) fn build_struct(
    layout: &Rc<Layout>,
    base: Option<Value>,
    names: &[impl AsRef<str>],
    values: Vec<Value>,
) -> Value {
    let declared = layout.fields.iter().map(String::as_str);
    let fields = match base {
        // With no base the literal gives every field, and usually in the
        // order the struct declares them: then its values are the fields.
        None if declared.eq(names.iter().map(AsRef::as_ref)) => values,
        base => {
            let mut fields = match base {
                Some(Value::Struct(base)) => base.values.clone(),
                // Each one is replaced below.
                _ => vec![Value::Bool(false); layout.fields.len()],
            };
            for (name, value) in names.iter().zip(values) {
                fields[slot(layout, name.as_ref())] = value;
            }
            fields
        }
    };
    Value::Struct(Rc::new(Instance {
        layout: Rc::clone(layout),
        values: fields,
    }))
}

/// A struct with one of its fields replaced, for `target.field = value` in a
/// sequence.
pub(crate) fn with_field(state: &Value, field: &str, value: Value) -> Value {
    let Value::Struct(instance) = state else {
        unreachable!("the state of a sequence whose fields are set is a struct")
    };
    let mut instance = (**instance).clone();
    instance.values[slot(&instance.layout, field)] = value;
    Value::Struct(Rc::new(instance))
}

fn slot(layout: &Layout, field: &str) -> usize {
    layout.position(field).expect("the struct has the field")
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
