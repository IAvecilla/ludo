use std::fmt;
use std::rc::Rc;

use crate::ast::FnDecl;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Symbol(String),
    Function(Rc<FnDecl>),
    Native(Native),
    Struct(Rc<Instance>),
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
    pub fun: fn(&[Value]) -> Result<Value, String>,
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
            Value::Function(_) | Value::Native(_) => "Function",
            Value::Struct(instance) => &instance.name,
        }
    }

    pub fn same_type(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Struct(a), Value::Struct(b)) => a.name == b.name,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
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
            Value::Function(decl) => write!(f, "<fn {}>", decl.name),
            Value::Native(native) => write!(f, "<fn {}>", native.name),
            Value::Struct(instance) => {
                write!(f, "{} {{", instance.name)?;
                for (i, (field, value)) in instance.fields.iter().enumerate() {
                    let sep = if i == 0 { " " } else { ", " };
                    match value {
                        Value::Str(v) => write!(f, "{sep}{field}: {v:?}")?,
                        v => write!(f, "{sep}{field}: {v}")?,
                    }
                }
                if instance.fields.is_empty() {
                    f.write_str("}")
                } else {
                    f.write_str(" }")
                }
            }
        }
    }
}
