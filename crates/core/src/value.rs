use std::fmt;
use std::rc::Rc;

use crate::ast::Literal;
use crate::chunk::Function;
use crate::runtime::RuntimeError;
use crate::vm::Vm;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Symbol(String),
    /// A function compiled to bytecode.
    Function(Rc<Function>),
    /// A function written where it is used, with the values it keeps.
    Closure(Rc<Closure>),
    Native(Native),
    Struct(Rc<Instance>),
    List(Rc<Vec<Value>>),
    /// `Some(x)` or `None`.
    Option(Option<Rc<Value>>),
    /// `Ok(x)` or `Error(e)`.
    Result(Result<Rc<Value>, Rc<Value>>),
}

/// A function written where it is used, and the values of the locals it keeps, in
/// the order its `captured` names them. Values never change, so keeping a
/// copy of them is the same as keeping the variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Closure {
    pub function: Rc<Function>,
    pub captured: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub name: String,
    pub fields: Vec<(String, Value)>,
}

impl Instance {
    pub fn get(&self, field: &str) -> Option<&Value> {
        self.fields.iter().find(|(f, _)| f == field).map(|(_, v)| v)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Native {
    pub name: &'static str,
    pub arity: usize,
    /// Gets the VM to call back into ludo, as `map` does with the function
    /// it is given.
    pub fun: fn(&mut Vm, &[Value]) -> Result<Value, RuntimeError>,
}

impl PartialEq for Native {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Value {
    pub fn type_name(&self) -> &str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::Str(_) => "String",
            Value::Symbol(_) => "Symbol",
            Value::Function(_) | Value::Closure(_) | Value::Native(_) => "Function",
            Value::Struct(instance) => &instance.name,
            Value::List(_) => "List",
            Value::Option(_) => "Option",
            Value::Result(_) => "Result",
        }
    }

    pub fn is_function(&self) -> bool {
        matches!(
            self,
            Value::Function(_) | Value::Closure(_) | Value::Native(_)
        )
    }

    pub fn same_type(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Struct(a), Value::Struct(b)) => a.name == b.name,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

/// The value a literal pattern of a `case` stands for.
impl From<&Literal> for Value {
    fn from(literal: &Literal) -> Self {
        match literal {
            Literal::Int(v) => Value::Int(*v),
            Literal::Float(v) => Value::Float(*v),
            Literal::Str(v) => Value::Str(v.clone()),
            Literal::Symbol(v) => Value::Symbol(v.clone()),
            Literal::Bool(v) => Value::Bool(*v),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v:?}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Str(v) => write!(f, "{v}"),
            Value::Symbol(v) => write!(f, ":{v}"),
            Value::Function(function) => write!(f, "{function}"),
            Value::Closure(_) => f.write_str("<fn>"),
            Value::Native(native) => write!(f, "<fn {}>", native.name),
            Value::Struct(instance) => {
                write!(f, "{} {{", instance.name)?;
                for (i, (field, value)) in instance.fields.iter().enumerate() {
                    let sep = if i == 0 { " " } else { ", " };
                    write!(f, "{sep}{field}: ")?;
                    nested(f, value)?;
                }
                if instance.fields.is_empty() {
                    f.write_str("}")
                } else {
                    f.write_str(" }")
                }
            }
            Value::Option(Some(value)) => wrapped(f, "Some", value),
            Value::Option(None) => f.write_str("None"),
            Value::Result(Ok(value)) => wrapped(f, "Ok", value),
            Value::Result(Err(value)) => wrapped(f, "Error", value),
            Value::List(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    nested(f, item)?;
                }
                f.write_str("]")
            }
        }
    }
}

fn wrapped(f: &mut fmt::Formatter<'_>, variant: &str, value: &Value) -> fmt::Result {
    write!(f, "{variant}(")?;
    nested(f, value)?;
    f.write_str(")")
}

fn nested(f: &mut fmt::Formatter<'_>, value: &Value) -> fmt::Result {
    match value {
        Value::Str(v) => write!(f, "{v:?}"),
        v => write!(f, "{v}"),
    }
}
