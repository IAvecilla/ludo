use crate::value::{Native, Value};

pub const NATIVES: &[Native] = &[
    Native {
        name: "abs",
        arity: 1,
        fun: abs,
    },
    Native {
        name: "min",
        arity: 2,
        fun: min,
    },
    Native {
        name: "max",
        arity: 2,
        fun: max,
    },
    Native {
        name: "clamp",
        arity: 3,
        fun: clamp,
    },
    Native {
        name: "sqrt",
        arity: 1,
        fun: sqrt,
    },
    Native {
        name: "sin",
        arity: 1,
        fun: sin,
    },
    Native {
        name: "cos",
        arity: 1,
        fun: cos,
    },
    Native {
        name: "to_float",
        arity: 1,
        fun: to_float,
    },
    Native {
        name: "to_int",
        arity: 1,
        fun: to_int,
    },
];

fn abs(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Int(v) => v
            .checked_abs()
            .map(Value::Int)
            .ok_or_else(|| "integer overflow in `abs`".to_string()),
        Value::Float(v) => Ok(Value::Float(v.abs())),
        v => Err(format!(
            "`abs` expects an Int or a Float, got {}",
            v.type_name()
        )),
    }
}

fn min(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.min(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.min(*b))),
        (a, b) => Err(numbers("min", &[a, b])),
    }
}

fn max(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(*a.max(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.max(*b))),
        (a, b) => Err(numbers("max", &[a, b])),
    }
}

fn clamp(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1], &args[2]) {
        (Value::Int(x), Value::Int(lo), Value::Int(hi)) if lo <= hi => {
            Ok(Value::Int(*x.clamp(lo, hi)))
        }
        (Value::Float(x), Value::Float(lo), Value::Float(hi)) if lo <= hi => {
            Ok(Value::Float(x.clamp(*lo, *hi)))
        }
        (Value::Int(_), Value::Int(_), Value::Int(_))
        | (Value::Float(_), Value::Float(_), Value::Float(_)) => {
            Err("`clamp` expects its lower bound to be at most its upper bound".to_string())
        }
        (x, lo, hi) => Err(numbers("clamp", &[x, lo, hi])),
    }
}

fn sqrt(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Float(v) if *v < 0.0 => Err("`sqrt` of a negative number".to_string()),
        Value::Float(v) => Ok(Value::Float(v.sqrt())),
        v => Err(float("sqrt", v)),
    }
}

fn sin(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Float(v) => Ok(Value::Float(v.sin())),
        v => Err(float("sin", v)),
    }
}

fn cos(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Float(v) => Ok(Value::Float(v.cos())),
        v => Err(float("cos", v)),
    }
}

fn to_float(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Int(v) => Ok(Value::Float(*v as f64)),
        v => Err(format!("`to_float` expects an Int, got {}", v.type_name())),
    }
}

fn to_int(args: &[Value]) -> Result<Value, String> {
    match &args[0] {
        Value::Float(v) if v.is_finite() && v.abs() < i64::MAX as f64 => {
            Ok(Value::Int(v.trunc() as i64))
        }
        Value::Float(v) => Err(format!("`to_int` cannot represent {v:?} as an Int")),
        v => Err(float("to_int", v)),
    }
}

fn float(name: &str, v: &Value) -> String {
    format!("`{name}` expects a Float, got {}", v.type_name())
}

fn numbers(name: &str, args: &[&Value]) -> String {
    let types: Vec<&str> = args.iter().map(|v| v.type_name()).collect();
    format!(
        "`{name}` expects all Ints or all Floats, got {}",
        types.join(", ")
    )
}
