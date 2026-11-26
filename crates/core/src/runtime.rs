//! What values do when a program runs, apart from the VM that runs it:
//! runtime errors, operators, fields, structs, patterns and the timing of
//! `over`. The checker shares the hints for names used as the wrong thing.

use std::rc::Rc;

use crate::ast::{BinaryOp, UnaryOp, Variant};
use crate::chunk::Test;
use crate::value::{Instance, Value};

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
/// order they are written.
pub(crate) fn build_struct<'a>(
    name: &str,
    declared: &[String],
    base: Option<Value>,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
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
        slots[index] = Some(given);
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
