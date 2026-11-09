//! The types the checker gives to values, and the variables it solves to
//! find the types of a generic native at each call.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    Bool,
    Str,
    /// Any symbol.
    Symbol,
    /// One of these symbols: the type of a symbol literal, or of something
    /// that gives one of several. It fits an enum that has all of them.
    Symbols(BTreeSet<String>),
    Enum(String),
    Struct(String),
    Union(String),
    List(Box<Type>),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Function(Vec<Type>, Box<Type>),
    /// A type in the signature of a generic native, which each use of it
    /// replaces with a fresh variable. A numeric one stands for an Int or a
    /// Float.
    Generic(usize, bool),
    /// A type the checker is still finding out.
    Var(usize),
    /// The type of what already has an error, which fits anywhere so the
    /// error is reported once.
    Unknown,
}

impl Type {
    pub fn list(item: Type) -> Type {
        Type::List(Box::new(item))
    }

    pub fn option(item: Type) -> Type {
        Type::Option(Box::new(item))
    }

    pub fn result(ok: Type, error: Type) -> Type {
        Type::Result(Box::new(ok), Box::new(error))
    }

    pub fn function(params: Vec<Type>, ret: Type) -> Type {
        Type::Function(params, Box::new(ret))
    }

    pub fn symbol(name: &str) -> Type {
        Type::Symbols(BTreeSet::from([name.to_string()]))
    }

    /// Whether its values are symbols.
    pub fn is_symbolic(&self) -> bool {
        matches!(self, Type::Symbol | Type::Symbols(_) | Type::Enum(_))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |types: &[Type]| {
            let shown: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            shown.join(", ")
        };
        match self {
            Type::Int => f.write_str("Int"),
            Type::Float => f.write_str("Float"),
            Type::Bool => f.write_str("Bool"),
            Type::Str => f.write_str("String"),
            Type::Symbol => f.write_str("Symbol"),
            Type::Symbols(symbols) => {
                let shown: Vec<String> = symbols.iter().map(|s| format!(":{s}")).collect();
                f.write_str(&shown.join(" | "))
            }
            Type::Enum(name) | Type::Struct(name) | Type::Union(name) => f.write_str(name),
            Type::List(item) => write!(f, "List[{item}]"),
            Type::Option(item) => write!(f, "Option[{item}]"),
            Type::Result(ok, error) => write!(f, "Result[{ok}, {error}]"),
            Type::Function(params, ret) => write!(f, "({}) -> {ret}", list(params)),
            Type::Generic(..) | Type::Var(_) | Type::Unknown => f.write_str("_"),
        }
    }
}

/// The variables the checker is solving, and what it found for each.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    slots: Vec<Slot>,
    /// What each change replaced, newest last, so a test that fails can undo
    /// its changes without copying every variable beforehand.
    trail: Vec<(usize, Slot)>,
}

/// Where the variables were, to go back to it.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    slots: usize,
    trail: usize,
}

#[derive(Debug, Clone)]
struct Slot {
    bound: Option<Type>,
    /// Whether it can only be an Int or a Float.
    numeric: bool,
}

impl Vars {
    pub fn fresh(&mut self, numeric: bool) -> Type {
        self.slots.push(Slot {
            bound: None,
            numeric,
        });
        Type::Var(self.slots.len() - 1)
    }

    pub fn is_numeric(&self, var: usize) -> bool {
        self.slots[var].numeric
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            slots: self.slots.len(),
            trail: self.trail.len(),
        }
    }

    /// Undoes every change since the snapshot, and drops the variables made
    /// after it.
    pub fn rollback(&mut self, snapshot: Snapshot) {
        while self.trail.len() > snapshot.trail {
            let (var, slot) = self.trail.pop().expect("the trail is longer");
            self.slots[var] = slot;
        }
        self.slots.truncate(snapshot.slots);
    }

    /// Forgets how to undo the changes so far, once nothing will.
    pub fn commit(&mut self) {
        self.trail.clear();
    }

    fn set(&mut self, var: usize, slot: Slot) {
        let before = std::mem::replace(&mut self.slots[var], slot);
        self.trail.push((var, before));
    }

    /// A type with the variables found so far put in, all the way down.
    pub fn resolve(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(var) => match &self.slots[*var].bound {
                Some(bound) => self.resolve(bound),
                None => ty.clone(),
            },
            Type::List(item) => Type::list(self.resolve(item)),
            Type::Option(item) => Type::option(self.resolve(item)),
            Type::Result(ok, error) => Type::result(self.resolve(ok), self.resolve(error)),
            Type::Function(params, ret) => Type::function(
                params.iter().map(|p| self.resolve(p)).collect(),
                self.resolve(ret),
            ),
            other => other.clone(),
        }
    }

    /// A type with its outermost variable put in, if it is found.
    pub fn shallow(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(var) => match &self.slots[*var].bound {
                Some(bound) => self.shallow(bound),
                None => ty.clone(),
            },
            other => other.clone(),
        }
    }

    /// Takes `ty` as what a variable stands for, if it can: a numeric one
    /// only takes an Int, a Float or another variable, and none takes a type
    /// that has itself inside.
    pub fn bind(&mut self, var: usize, ty: Type) -> bool {
        match ty {
            Type::Var(other) if other == var => true,
            Type::Var(other) => {
                if self.slots[var].numeric && !self.slots[other].numeric {
                    self.set(
                        other,
                        Slot {
                            bound: None,
                            numeric: true,
                        },
                    );
                }
                self.bind_slot(var, Type::Var(other));
                true
            }
            Type::Int | Type::Float | Type::Unknown => {
                self.bind_slot(var, ty);
                true
            }
            _ if self.slots[var].numeric => false,
            _ if self.occurs(var, &ty) => false,
            _ => {
                self.bind_slot(var, ty);
                true
            }
        }
    }

    fn bind_slot(&mut self, var: usize, ty: Type) {
        let numeric = self.slots[var].numeric;
        self.set(
            var,
            Slot {
                bound: Some(ty),
                numeric,
            },
        );
    }

    fn occurs(&self, var: usize, ty: &Type) -> bool {
        match self.shallow(ty) {
            Type::Var(other) => other == var,
            Type::List(item) | Type::Option(item) => self.occurs(var, &item),
            Type::Result(ok, error) => self.occurs(var, &ok) || self.occurs(var, &error),
            Type::Function(params, ret) => {
                params.iter().any(|p| self.occurs(var, p)) || self.occurs(var, &ret)
            }
            _ => false,
        }
    }

    /// The type of a use of a generic native: each of its generics replaced
    /// by a fresh variable, the same one wherever it repeats.
    pub fn instantiate(&mut self, ty: &Type) -> Type {
        self.instantiate_with(ty, &mut HashMap::new())
    }

    fn instantiate_with(&mut self, ty: &Type, fresh: &mut HashMap<usize, Type>) -> Type {
        match ty {
            Type::Generic(id, numeric) => fresh
                .entry(*id)
                .or_insert_with(|| self.fresh(*numeric))
                .clone(),
            Type::List(item) => Type::list(self.instantiate_with(item, fresh)),
            Type::Option(item) => Type::option(self.instantiate_with(item, fresh)),
            Type::Result(ok, error) => {
                let ok = self.instantiate_with(ok, fresh);
                Type::result(ok, self.instantiate_with(error, fresh))
            }
            Type::Function(params, ret) => {
                let params = params
                    .iter()
                    .map(|p| self.instantiate_with(p, fresh))
                    .collect();
                Type::function(params, self.instantiate_with(ret, fresh))
            }
            other => other.clone(),
        }
    }
}
