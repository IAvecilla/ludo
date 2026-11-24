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

    /// The same type with `f` applied to each type it is made of.
    fn map(&self, mut f: impl FnMut(&Type) -> Type) -> Type {
        match self {
            Type::List(item) => Type::list(f(item)),
            Type::Option(item) => Type::option(f(item)),
            Type::Result(ok, error) => {
                let ok = f(ok);
                Type::result(ok, f(error))
            }
            Type::Function(params, ret) => {
                let params = params.iter().map(&mut f).collect();
                Type::function(params, f(ret))
            }
            other => other.clone(),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            Type::Function(params, ret) => {
                let params: Vec<String> = params.iter().map(Type::to_string).collect();
                write!(f, "({}) -> {ret}", params.join(", "))
            }
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
        for (var, slot) in self.trail.drain(snapshot.trail..).rev() {
            self.slots[var] = slot;
        }
        self.slots.truncate(snapshot.slots);
    }

    /// Forgets how to undo the changes so far, once nothing will.
    pub fn commit(&mut self) {
        self.trail.clear();
    }

    /// Changes a variable, keeping on the trail what it was.
    fn change(&mut self, var: usize, change: impl FnOnce(&mut Slot)) {
        self.trail.push((var, self.slots[var].clone()));
        change(&mut self.slots[var]);
    }

    /// A type with the variables found so far put in, all the way down.
    pub fn resolve(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(var) => match &self.slots[*var].bound {
                Some(bound) => self.resolve(bound),
                None => ty.clone(),
            },
            _ => ty.map(|part| self.resolve(part)),
        }
    }

    /// A type with its outermost variable put in, if it is found.
    pub fn shallow(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(var) => match &self.slots[*var].bound {
                Some(bound) => self.shallow(bound),
                None => ty.clone(),
            },
            _ => ty.clone(),
        }
    }

    /// Takes `ty` as what a variable stands for, if it can: a numeric one
    /// only takes an Int, a Float or another variable, and none takes a type
    /// that has itself inside.
    pub fn bind(&mut self, var: usize, ty: Type) -> bool {
        let numeric = self.slots[var].numeric;
        match ty {
            Type::Var(other) if other == var => return true,
            Type::Var(other) if numeric && !self.slots[other].numeric => {
                self.change(other, |slot| slot.numeric = true);
            }
            Type::Var(_) | Type::Int | Type::Float | Type::Unknown => {}
            _ if numeric || self.occurs(var, &ty) => return false,
            _ => {}
        }
        self.change(var, |slot| slot.bound = Some(ty));
        true
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
            _ => ty.map(|part| self.instantiate_with(part, fresh)),
        }
    }
}
