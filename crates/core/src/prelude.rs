/// This module defines the standard native functions available in the ludo language.
use std::rc::Rc;

use crate::interpreter::{error, RuntimeError};
use crate::value::{Caller, Native, Value};

const MAX_RANGE: i64 = 1_000_000;

pub const NATIVES: &[Native] = &[
    native("Some", 1, some),
    native("Ok", 1, ok),
    native("Error", 1, error_value),
    native("print", 1, print),
    native("map", 2, map),
    native("filter", 2, filter),
    native("find", 2, find),
    native("fold", 3, fold),
    native("range", 2, range),
    native("abs", 1, abs),
    native("min", 2, min),
    native("clamp", 3, clamp),
    native("to_float", 1, to_float),
    native("to_int", 1, to_int),
    native("to_string", 1, to_string),
    native("parse_int", 1, parse_int),
    native("concat", 2, concat),
    native("contains", 2, contains),
];

const fn native(
    name: &'static str,
    arity: usize,
    fun: fn(&mut dyn Caller, &[Value]) -> Result<Value, RuntimeError>,
) -> Native {
    Native { name, arity, fun }
}

/// The globals that are values rather than functions.
pub fn constants() -> [(&'static str, Value); 1] {
    [("None", Value::Option(None))]
}

fn some(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(Value::Option(Some(Rc::new(args[0].clone()))))
}

fn ok(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(Value::Result(Ok(Rc::new(args[0].clone()))))
}

fn error_value(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(Value::Result(Err(Rc::new(args[0].clone()))))
}

fn print(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    println!("{}", args[0]);
    Ok(args[0].clone())
}

fn map(caller: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::List(items) = &args[0] else {
        return Err(not_a_list("map", &args[0]));
    };
    let mapped = items
        .iter()
        .map(|item| caller.call(args[1].clone(), vec![item.clone()]))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::List(Rc::new(mapped)))
}

fn filter(caller: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::List(items) = &args[0] else {
        return Err(not_a_list("filter", &args[0]));
    };
    let mut kept = Vec::new();
    for item in items.iter() {
        match caller.call(args[1].clone(), vec![item.clone()])? {
            Value::Bool(true) => kept.push(item.clone()),
            Value::Bool(false) => {}
            other => {
                return Err(error(format!(
                    "`filter` expects a function that returns a Bool, got {}",
                    other.type_name()
                )))
            }
        }
    }
    Ok(Value::List(Rc::new(kept)))
}

/// The first item for which the function gives true, if any.
fn find(caller: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::List(items) = &args[0] else {
        return Err(not_a_list("find", &args[0]));
    };
    for item in items.iter() {
        match caller.call(args[1].clone(), vec![item.clone()])? {
            Value::Bool(true) => return Ok(Value::Option(Some(Rc::new(item.clone())))),
            Value::Bool(false) => {}
            other => {
                return Err(error(format!(
                    "`find` expects a function that returns a Bool, got {}",
                    other.type_name()
                )))
            }
        }
    }
    Ok(Value::Option(None))
}

fn fold(caller: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::List(items) = &args[0] else {
        return Err(not_a_list("fold", &args[0]));
    };
    items.iter().try_fold(args[1].clone(), |acc, item| {
        caller.call(args[2].clone(), vec![acc, item.clone()])
    })
}

fn range(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match (&args[0], &args[1]) {
        (Value::Int(from), Value::Int(to)) if to - from > MAX_RANGE => Err(error(format!(
            "`range` can make at most {MAX_RANGE} numbers, got {}",
            to - from
        ))),
        (Value::Int(from), Value::Int(to)) => {
            Ok(Value::List(Rc::new((*from..*to).map(Value::Int).collect())))
        }
        (a, b) => Err(error(format!(
            "`range` expects two Ints, got {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

fn abs(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Int(v) => v
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| error("integer overflow in `abs`")),
        Value::Float(v) => Ok(Value::Float(v.abs())),
        v => Err(error(format!(
            "`abs` expects an Int or a Float, got {}",
            v.type_name()
        ))),
    }
}

fn min(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.min(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.min(*b))),
        (a, b) => Err(numbers("min", &[a, b])),
    }
}

fn clamp(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match (&args[0], &args[1], &args[2]) {
        (Value::Int(x), Value::Int(lo), Value::Int(hi)) if lo <= hi => {
            Ok(Value::Int(*x.clamp(lo, hi)))
        }
        (Value::Float(x), Value::Float(lo), Value::Float(hi)) if lo <= hi => {
            Ok(Value::Float(x.clamp(*lo, *hi)))
        }
        (Value::Int(_), Value::Int(_), Value::Int(_))
        | (Value::Float(_), Value::Float(_), Value::Float(_)) => Err(error(
            "`clamp` expects its lower bound to be at most its upper bound",
        )),
        (x, lo, hi) => Err(numbers("clamp", &[x, lo, hi])),
    }
}

fn to_float(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Int(v) => Ok(Value::Float(*v as f64)),
        v => Err(error(format!(
            "`to_float` expects an Int, got {}",
            v.type_name()
        ))),
    }
}

fn to_int(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match &args[0] {
        Value::Float(v) if v.is_finite() && v.abs() < i64::MAX as f64 => {
            Ok(Value::Int(v.trunc() as i64))
        }
        Value::Float(v) => Err(error(format!("`to_int` cannot represent {v:?} as an Int"))),
        v => Err(error(format!(
            "`to_int` expects a Float, got {}",
            v.type_name()
        ))),
    }
}

fn to_string(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    Ok(Value::Str(args[0].to_string()))
}

/// The Int a String spells, or an error that says it does not.
fn parse_int(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::Str(text) = &args[0] else {
        return Err(error(format!(
            "`parse_int` expects a String, got {}",
            args[0].type_name()
        )));
    };
    Ok(Value::Result(match text.trim().parse::<i64>() {
        Ok(n) => Ok(Rc::new(Value::Int(n))),
        Err(_) => Err(Rc::new(Value::Str(format!("{text:?} is not an Int")))),
    }))
}

fn concat(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    match (&args[0], &args[1]) {
        (Value::List(a), Value::List(b)) => {
            let mut items = Vec::with_capacity(a.len() + b.len());
            items.extend(a.iter().cloned());
            items.extend(b.iter().cloned());
            Ok(Value::List(Rc::new(items)))
        }
        (a, b) => Err(error(format!(
            "`concat` expects two Lists, got {} and {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

fn contains(_: &mut dyn Caller, args: &[Value]) -> Result<Value, RuntimeError> {
    let Value::List(items) = &args[0] else {
        return Err(not_a_list("contains", &args[0]));
    };
    Ok(Value::Bool(items.contains(&args[1])))
}

fn not_a_list(name: &str, value: &Value) -> RuntimeError {
    error(format!(
        "`{name}` expects a List, got {}",
        value.type_name()
    ))
}

fn numbers(name: &str, args: &[&Value]) -> RuntimeError {
    let types: Vec<&str> = args.iter().map(|v| v.type_name()).collect();
    error(format!(
        "`{name}` expects all Ints or all Floats, got {}",
        types.join(", ")
    ))
}
