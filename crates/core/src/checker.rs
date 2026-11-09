//! Checks a program before it runs, in one walk over its tree that neither
//! backend depends on. It checks the names it uses, as jlox's resolver does
//! in chapter 11 of the book: a name that is not declared, a call with the
//! wrong number of arguments, a struct built with a field it does not have
//! or without one it needs. And it checks the type of every value.
//!
//! The top level runs from top to bottom, so there a name has to be declared
//! before it is used. The body of a function or a sequence runs later, when it
//! is called or started, so it can use any function, struct, enum or sequence
//! of the program, but a top-level `let` only if it comes before. Types are
//! known everywhere, since nothing has to run to declare them.
//!
//! Types are checked both ways. Where the type a value must have is known, a
//! parameter or a field or what a function returns, the value is checked
//! against it, so `[Line {...}, Circle {...}]` fits a `List[Shape]`. Elsewhere
//! its type is found from what it is made of.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ast::{
    Arm, BinaryOp, Expr, FnDecl, Literal, Param, Pattern, Step, StepKind, Stmt, StmtKind, TypeExpr,
    UnaryOp, Variant,
};
use crate::interpreter::{check_arity, enum_hint, sequence_hint, struct_hint};
use crate::prelude::{constants, NATIVES};
use crate::types::{Type, Vars};

/// Whether an error is about a name or about a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Name,
    Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub kind: Kind,
    pub message: String,
    pub line: usize,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Name => "Name",
            Kind::Type => "Type",
        })
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[line {}] {} Error: {}",
            self.line, self.kind, self.message
        )
    }
}

/// What the top level declares as it runs, kept apart by kind as the
/// backends do.
#[derive(Debug, Clone, Default)]
struct Declared {
    /// The type of each function and value.
    values: HashMap<String, Type>,
    structs: HashSet<String>,
    /// The first symbol of each enum, for the hint when it is used as a value.
    enums: HashMap<String, String>,
    /// The types of the parameters of each sequence.
    sequences: HashMap<String, Vec<Type>>,
}

/// The types the program declares.
#[derive(Debug, Clone, Default)]
struct Types {
    structs: HashMap<String, Vec<(String, Type)>>,
    enums: HashMap<String, Vec<String>>,
    unions: HashMap<String, Vec<Type>>,
}

/// Checks the statements of a program as they come, keeping what they
/// declare for the ones after: a whole file at once, or the REPL one input at
/// a time.
#[derive(Debug, Clone)]
pub struct Checker {
    declared: Declared,
    types: Types,
    vars: Vars,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    pub fn new() -> Self {
        let mut declared = Declared::default();
        for native in NATIVES {
            declared
                .values
                .insert(native.name.to_string(), native_type(native.name));
        }
        for (name, _) in constants() {
            declared
                .values
                .insert(name.to_string(), constant_type(name));
        }
        Checker {
            declared,
            types: Types::default(),
            vars: Vars::default(),
        }
    }

    /// Checks statements that run one after another, after those checked
    /// before. If they have errors, what they declare is forgotten, since
    /// they will not run.
    pub fn check(&mut self, statements: &[Stmt]) -> Result<(), Vec<CheckError>> {
        let mut pass = Pass {
            declared: self.declared.clone(),
            ahead: Declared::default(),
            types: self.types.clone(),
            vars: self.vars.clone(),
            lets: HashMap::new(),
            scopes: Vec::new(),
            body: None,
            state: None,
            line: 0,
            errors: Vec::new(),
        };
        pass.declare_types(statements);
        pass.look_ahead(statements);
        for stmt in statements {
            pass.top_level(stmt);
        }
        if pass.errors.is_empty() {
            self.declared = pass.declared;
            self.types = pass.types;
            self.vars = pass.vars;
            self.vars.commit();
            Ok(())
        } else {
            Err(pass.errors)
        }
    }
}

/// The type of each native. A generic one has its types found at each call:
/// `map` gives a list of what its function gives.
fn native_type(name: &str) -> Type {
    let a = || Type::Generic(0, false);
    let b = || Type::Generic(1, false);
    let number = || Type::Generic(0, true);
    let fun = Type::function;
    match name {
        "Some" => fun(vec![a()], Type::option(a())),
        "Ok" => fun(vec![a()], Type::result(a(), b())),
        "Error" => fun(vec![b()], Type::result(a(), b())),
        "print" => fun(vec![a()], a()),
        "map" => fun(vec![Type::list(a()), fun(vec![a()], b())], Type::list(b())),
        "filter" => fun(
            vec![Type::list(a()), fun(vec![a()], Type::Bool)],
            Type::list(a()),
        ),
        "find" => fun(
            vec![Type::list(a()), fun(vec![a()], Type::Bool)],
            Type::option(a()),
        ),
        "fold" => fun(vec![Type::list(a()), b(), fun(vec![b(), a()], b())], b()),
        "range" => fun(vec![Type::Int, Type::Int], Type::list(Type::Int)),
        "abs" => fun(vec![number()], number()),
        "min" => fun(vec![number(), number()], number()),
        "clamp" => fun(vec![number(), number(), number()], number()),
        "to_float" => fun(vec![Type::Int], Type::Float),
        "to_int" => fun(vec![Type::Float], Type::Int),
        "to_string" => fun(vec![a()], Type::Str),
        "parse_int" => fun(vec![Type::Str], Type::result(Type::Int, Type::Str)),
        "concat" => fun(vec![Type::list(a()), Type::list(a())], Type::list(a())),
        "contains" => fun(vec![Type::list(a()), a()], Type::Bool),
        other => unreachable!("every native has a type, `{other}` too"),
    }
}

fn constant_type(name: &str) -> Type {
    match name {
        "None" => Type::option(Type::Generic(0, false)),
        other => unreachable!("every constant has a type, `{other}` too"),
    }
}

fn literal_type(literal: &Literal) -> Type {
    match literal {
        Literal::Int(_) => Type::Int,
        Literal::Float(_) => Type::Float,
        Literal::Str(_) => Type::Str,
        Literal::Symbol(name) => Type::symbol(name),
        Literal::Bool(_) => Type::Bool,
    }
}

/// Where a value has to fit and how to name that place in an error, as in
/// "argument 2 of `add`".
type Expected<'a> = Option<(&'a Type, &'a str)>;

/// One walk of the checker over a group of statements.
struct Pass {
    /// What the top level has declared before the statement being checked.
    declared: Declared,
    /// Every function, struct, enum and sequence the statements declare,
    /// wherever they are, which a body can use.
    ahead: Declared,
    types: Types,
    vars: Vars,
    /// The line where each top-level `let` of the statements is first
    /// declared, to tell a `let` that comes later from a missing one.
    lets: HashMap<String, usize>,
    /// The locals in scope with their types, innermost last.
    scopes: Vec<Vec<(String, Type)>>,
    /// Inside the body of a function or a sequence, which of the two it is.
    body: Option<&'static str>,
    /// Inside a sequence, its name and the name and type of its state.
    state: Option<(String, String, Type)>,
    line: usize,
    errors: Vec<CheckError>,
}

impl Pass {
    /// Learns the types the statements declare, first their names and then
    /// what they are made of, so they can name each other in any order.
    fn declare_types(&mut self, statements: &[Stmt]) {
        for stmt in statements {
            match &stmt.kind {
                StmtKind::Struct(decl) => {
                    self.types.structs.insert(decl.name.clone(), Vec::new());
                }
                StmtKind::Enum(decl) => {
                    self.types
                        .enums
                        .insert(decl.name.clone(), decl.symbols.clone());
                }
                StmtKind::Union(decl) => {
                    self.types.unions.insert(decl.name.clone(), Vec::new());
                }
                _ => {}
            }
        }
        for stmt in statements {
            self.line = stmt.line;
            match &stmt.kind {
                StmtKind::Struct(decl) => {
                    let fields = decl
                        .fields
                        .iter()
                        .map(|field| (field.name.clone(), self.resolve(&field.ty)))
                        .collect();
                    self.types.structs.insert(decl.name.clone(), fields);
                }
                StmtKind::Union(decl) => {
                    let members = decl.members.iter().map(|m| self.resolve(m)).collect();
                    self.types.unions.insert(decl.name.clone(), members);
                }
                _ => {}
            }
        }
        // All of them first: emptying one would hide the others in its loop.
        let looped: Vec<(usize, String)> = statements
            .iter()
            .filter_map(|stmt| match &stmt.kind {
                StmtKind::Union(decl)
                    if self.made_of(&decl.name, &decl.name, &mut HashSet::new()) =>
                {
                    Some((stmt.line, decl.name.clone()))
                }
                _ => None,
            })
            .collect();
        for (line, name) in looped {
            self.line = line;
            self.type_error(format!("`{name}` is made of itself"));
            self.types.unions.insert(name, Vec::new());
        }
    }

    /// Whether the union `name` has `target` among its members, or among
    /// those of a union in it.
    fn made_of(&self, name: &str, target: &str, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(name.to_string()) {
            return false;
        }
        let members = self.types.unions.get(name).cloned().unwrap_or_default();
        members.iter().any(|member| match member {
            Type::Union(inner) => inner == target || self.made_of(inner, target, seen),
            _ => false,
        })
    }

    /// Learns what the statements declare that a body can use before it is
    /// declared: functions, structs, enums and sequences.
    fn look_ahead(&mut self, statements: &[Stmt]) {
        for stmt in statements {
            self.line = stmt.line;
            match &stmt.kind {
                StmtKind::Let(name, _) => {
                    self.lets.entry(name.clone()).or_insert(stmt.line);
                }
                StmtKind::Fn(decl) => {
                    let signature = self.signature(decl);
                    self.ahead.values.insert(decl.name.clone(), signature);
                }
                StmtKind::Struct(decl) => {
                    self.ahead.structs.insert(decl.name.clone());
                }
                StmtKind::Enum(decl) => {
                    self.ahead
                        .enums
                        .insert(decl.name.clone(), decl.symbols[0].clone());
                }
                StmtKind::Sequence(decl) => {
                    let params = decl.params.iter().map(|p| self.resolve(&p.ty)).collect();
                    self.ahead.sequences.insert(decl.name.clone(), params);
                }
                StmtKind::Union(_) | StmtKind::Start(..) | StmtKind::Expr(_) => {}
            }
        }
    }

    fn signature(&mut self, decl: &FnDecl) -> Type {
        let params = decl.params.iter().map(|p| self.resolve(&p.ty)).collect();
        Type::function(params, self.resolve(&decl.ret))
    }

    /// The type a written type stands for.
    fn resolve(&mut self, ty: &TypeExpr) -> Type {
        let (name, args) = match ty {
            TypeExpr::Function { params, ret } => {
                let params = params.iter().map(|p| self.resolve(p)).collect();
                return Type::function(params, self.resolve(ret));
            }
            TypeExpr::Named { name, args } => (name.as_str(), args),
        };
        let takes = match name {
            "List" | "Option" => 1,
            "Result" => 2,
            _ => 0,
        };
        if args.len() != takes {
            let types = |n: usize| match n {
                0 => "no types".to_string(),
                1 => "1 type".to_string(),
                n => format!("{n} types"),
            };
            self.type_error(format!(
                "`{name}` takes {}, got {}",
                types(takes),
                args.len()
            ));
            return Type::Unknown;
        }
        let mut arg = |i: usize| self.resolve(&args[i]);
        match name {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "Bool" => Type::Bool,
            "String" => Type::Str,
            "Symbol" => Type::Symbol,
            "List" => Type::list(arg(0)),
            "Option" => Type::option(arg(0)),
            "Result" => {
                let ok = arg(0);
                Type::result(ok, arg(1))
            }
            _ if self.types.structs.contains_key(name) => Type::Struct(name.to_string()),
            _ if self.types.enums.contains_key(name) => Type::Enum(name.to_string()),
            _ if self.types.unions.contains_key(name) => Type::Union(name.to_string()),
            _ => {
                self.type_error(format!("undefined type `{name}`"));
                Type::Unknown
            }
        }
    }

    fn top_level(&mut self, stmt: &Stmt) {
        self.line = stmt.line;
        match &stmt.kind {
            StmtKind::Let(name, value) => {
                // Declared once its value is checked, which cannot use it.
                let ty = self.synth(value);
                self.declared.values.insert(name.clone(), ty);
            }
            StmtKind::Fn(decl) => {
                let signature = self.ahead.values[&decl.name].clone();
                self.declared
                    .values
                    .insert(decl.name.clone(), signature.clone());
                let Type::Function(params, ret) = signature else {
                    unreachable!("a function has a function type")
                };
                let what = format!("the value of `{}`", decl.name);
                self.body(&decl.params, params, "function", |pass| {
                    pass.check(&decl.body, &ret, &what)
                });
            }
            StmtKind::Sequence(decl) => {
                let params = self.ahead.sequences[&decl.name].clone();
                self.declared
                    .sequences
                    .insert(decl.name.clone(), params.clone());
                let state = (
                    decl.name.clone(),
                    decl.params[0].name.clone(),
                    params[0].clone(),
                );
                self.state = Some(state);
                self.body(&decl.params, params, "sequence", |pass| {
                    for step in &decl.body {
                        pass.step(step);
                    }
                });
                self.state = None;
            }
            StmtKind::Struct(decl) => {
                self.declared.structs.insert(decl.name.clone());
            }
            StmtKind::Enum(decl) => {
                self.declared
                    .enums
                    .insert(decl.name.clone(), decl.symbols[0].clone());
            }
            StmtKind::Union(_) => {}
            StmtKind::Start(..) | StmtKind::Expr(_) => self.statement(stmt),
        }
    }

    fn body(
        &mut self,
        params: &[Param],
        types: Vec<Type>,
        kind: &'static str,
        check: impl FnOnce(&mut Self),
    ) {
        self.body = Some(kind);
        let locals = params.iter().map(|p| p.name.clone()).zip(types).collect();
        self.scopes.push(locals);
        check(self);
        self.scopes.pop();
        self.body = None;
    }

    fn statement(&mut self, stmt: &Stmt) {
        let outer = std::mem::replace(&mut self.line, stmt.line);
        match &stmt.kind {
            StmtKind::Let(name, value) => {
                let ty = self.synth(value);
                self.declare(name, ty);
            }
            StmtKind::Start(name, args) => self.start(name, args),
            StmtKind::Expr(expr) => {
                self.synth(expr);
            }
            // The parser keeps declarations at the top level.
            StmtKind::Fn(_)
            | StmtKind::Struct(_)
            | StmtKind::Enum(_)
            | StmtKind::Union(_)
            | StmtKind::Sequence(_) => {}
        }
        self.line = outer;
    }

    fn start(&mut self, name: &str, args: &[Expr]) {
        let Some(params) = self.sequence(name) else {
            for arg in args {
                self.synth(arg);
            }
            return;
        };
        self.arity(name, params.len(), args.len());
        for (i, arg) in args.iter().enumerate() {
            match params.get(i) {
                Some(param) => {
                    let what = format!("argument {} of `{name}`", i + 1);
                    self.check(arg, param, &what)
                }
                None => {
                    self.synth(arg);
                }
            }
        }
    }

    fn step(&mut self, step: &Step) {
        let outer = std::mem::replace(&mut self.line, step.line);
        let state = self.state.clone().expect("steps are in a sequence");
        match &step.kind {
            // A `let` of the state's name replaces the state, which has to
            // stay of its type.
            StepKind::Stmt(Stmt {
                kind: StmtKind::Let(name, value),
                ..
            }) if *name == state.1 => {
                let what = format!("the state of `{}`", state.0);
                self.check(value, &state.2, &what);
            }
            StepKind::Stmt(stmt) => self.statement(stmt),
            StepKind::Set(target, field, value) => self.set(target, field, value, &state.2),
            StepKind::Over {
                duration,
                var,
                body,
            } => {
                let ty = self.synth(duration);
                if !self.fits(&ty, &Type::Float) {
                    let got = self.show(&ty);
                    self.type_error(format!("a duration must be a Float in seconds, got {got}"));
                }
                self.scopes
                    .push(var.iter().map(|v| (v.clone(), Type::Float)).collect());
                for step in body {
                    self.step(step);
                }
                self.scopes.pop();
            }
        }
        self.line = outer;
    }

    /// Checks `target.field = value`, where the target is the state.
    fn set(&mut self, target: &str, field: &str, value: &Expr, state: &Type) {
        let state = self.vars.shallow(state);
        let field_type = match &state {
            Type::Struct(name) => self.field_type(name, field),
            Type::Unknown => Some(Type::Unknown),
            other => {
                let got = self.show(other);
                self.type_error(format!(
                    "`{target}.{field} = ...` needs `{target}` to be a struct, got {got}"
                ));
                Some(Type::Unknown)
            }
        };
        match field_type {
            Some(ty) => {
                let what = format!("field `{field}` of `{state}`");
                self.check(value, &ty, &what);
            }
            None => {
                self.synth(value);
                self.type_error(format!("`{state}` has no field `{field}`"));
            }
        }
    }

    fn field_type(&self, name: &str, field: &str) -> Option<Type> {
        self.types.structs[name]
            .iter()
            .find(|(f, _)| f == field)
            .map(|(_, ty)| ty.clone())
    }

    /// The type of an expression, found from what it is made of.
    fn synth(&mut self, expr: &Expr) -> Type {
        self.expression(expr, None)
    }

    /// Checks that an expression gives a value that fits where `expected`
    /// is. `what` names that place for the error.
    fn check(&mut self, expr: &Expr, expected: &Type, what: &str) {
        self.expression(expr, Some((expected, what)));
    }

    /// The type of an expression. With an expected type it checks the
    /// expression against it, handing it down to where the value comes
    /// from: the end of a block, the branches of an `if` or a `case`, the
    /// items of a list, or the result of a generic call.
    fn expression(&mut self, expr: &Expr, expected: Expected) -> Type {
        let ty = match expr {
            Expr::If(cond, then, otherwise) => {
                return self.if_expression(cond, then, otherwise, expected)
            }
            Expr::Block(statements, tail, line) => {
                return self.block(statements, tail, *line, expected)
            }
            Expr::Case(value, arms) => return self.case(value, arms, expected),
            Expr::List(items) => match expected.map(|(t, _)| self.vars.shallow(t)) {
                Some(Type::List(item)) => {
                    for value in items {
                        self.check(value, &item, "an item of the list");
                    }
                    return Type::List(item);
                }
                _ => self.list(items),
            },
            Expr::Call(callee, args) => self.call(callee, args, expected.map(|(t, _)| t)),
            Expr::Int(_) => Type::Int,
            Expr::Float(_) => Type::Float,
            Expr::Str(_) => Type::Str,
            Expr::Bool(_) => Type::Bool,
            Expr::Symbol(name) => Type::symbol(name),
            Expr::Ident(name) => self.value(name),
            Expr::Unary(op, operand) => self.unary(*op, operand),
            Expr::Binary(left, op, right) => self.binary(left, *op, right),
            Expr::Field(target, name) => self.field(target, name),
            Expr::Struct { name, base, fields } => self.structure(name, base.as_deref(), fields),
        };
        if let Some((want, what)) = expected {
            self.expect(&ty, want, what);
        }
        ty
    }

    fn expect(&mut self, actual: &Type, expected: &Type, what: &str) {
        if !self.fits(actual, expected) {
            let (expected, actual) = (self.show(expected), self.show(actual));
            self.type_error(format!("{what} must be {expected}, got {actual}"));
        }
    }

    fn if_expression(
        &mut self,
        cond: &Expr,
        then: &Expr,
        otherwise: &Expr,
        expected: Expected,
    ) -> Type {
        self.condition(cond, "if");
        match expected {
            Some((want, _)) => {
                self.expression(then, expected);
                self.expression(otherwise, expected);
                want.clone()
            }
            None => {
                let then = self.synth(then);
                let otherwise = self.synth(otherwise);
                self.join_or_error(&then, &otherwise, "the branches of `if`")
            }
        }
    }

    fn condition(&mut self, cond: &Expr, keyword: &str) {
        let ty = self.synth(cond);
        if !self.fits(&ty, &Type::Bool) {
            let got = self.show(&ty);
            self.type_error(format!(
                "the condition of `{keyword}` must be a Bool, got {got}"
            ));
        }
    }

    fn block(&mut self, statements: &[Stmt], tail: &Expr, line: usize, expected: Expected) -> Type {
        self.scopes.push(Vec::new());
        for stmt in statements {
            self.statement(stmt);
        }
        let outer = std::mem::replace(&mut self.line, line);
        let ty = self.expression(tail, expected);
        self.line = outer;
        self.scopes.pop();
        ty
    }

    fn case(&mut self, value: &Expr, arms: &[Arm], expected: Expected) -> Type {
        let scrutinee = self.synth(value);
        let mut result: Option<Type> = None;
        for arm in arms {
            let outer = std::mem::replace(&mut self.line, arm.line);
            let mut bound = Vec::new();
            self.pattern(&arm.pattern, &scrutinee, &mut bound);
            self.scopes.push(bound);
            if let Some(guard) = &arm.guard {
                self.condition(guard, "when");
            }
            let ty = self.expression(&arm.body, expected);
            self.scopes.pop();
            if expected.is_none() {
                result = Some(match result {
                    None => ty,
                    Some(before) => self.join_or_error(&before, &ty, "the branches of `case`"),
                });
            }
            self.line = outer;
        }
        match expected {
            Some((want, _)) => want.clone(),
            None => result.unwrap_or(Type::Unknown),
        }
    }

    /// Checks that a pattern can match a value of type `ty`, adding what it
    /// binds to `bound` with its type.
    fn pattern(&mut self, pattern: &Pattern, ty: &Type, bound: &mut Vec<(String, Type)>) {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Bind(name) => bound.push((name.clone(), self.vars.resolve(ty))),
            Pattern::Literal(literal) => {
                if !self.fits(&literal_type(literal), ty) {
                    let ty = self.show(ty);
                    self.type_error(format!(
                        "the pattern `{literal}` can never match a value of type {ty}"
                    ));
                }
            }
            Pattern::Variant(variant, inner) => {
                let held = self.vars.fresh(false);
                let other = self.vars.fresh(false);
                let (whole, type_name) = match variant {
                    Variant::Some | Variant::None => (Type::option(held.clone()), "an Option"),
                    Variant::Ok => (Type::result(held.clone(), other), "a Result"),
                    Variant::Error => (Type::result(other, held.clone()), "a Result"),
                };
                if !self.fits(ty, &whole) {
                    let got = self.show(ty);
                    self.type_error(format!(
                        "a `{}` pattern needs {type_name}, got {got}",
                        variant.name()
                    ));
                    if let Some(inner) = inner {
                        self.pattern(inner, &Type::Unknown, bound);
                    }
                } else if let Some(inner) = inner {
                    self.pattern(inner, &held, bound);
                }
            }
        }
    }

    fn list(&mut self, items: &[Expr]) -> Type {
        let mut item = None;
        for value in items {
            let ty = self.synth(value);
            item = Some(match item {
                None => ty,
                Some(before) => self.join_or_error(&before, &ty, "the items of a list"),
            });
        }
        // The items of an empty list get their type from where it is used.
        Type::list(item.unwrap_or_else(|| self.vars.fresh(false)))
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], expected: Option<&Type>) -> Type {
        let name = match callee {
            Expr::Ident(name) => Some(name.as_str()),
            _ => None,
        };
        let callee = self.synth(callee);
        let (params, ret) = match self.vars.shallow(&callee) {
            Type::Function(params, ret) => (params, *ret),
            Type::Unknown | Type::Var(_) => {
                for arg in args {
                    self.synth(arg);
                }
                return Type::Unknown;
            }
            other => {
                let got = self.show(&other);
                self.type_error(format!("can only call functions, got {got}"));
                for arg in args {
                    self.synth(arg);
                }
                return Type::Unknown;
            }
        };
        if params.len() != args.len() {
            self.arity(name.unwrap_or("the function"), params.len(), args.len());
            for arg in args {
                self.synth(arg);
            }
            return self.vars.resolve(&ret);
        }
        // What the call must give tells a generic native its types before
        // the arguments do: in `concat(net(), [...])`, where a `List[Shape]`
        // is expected, the list holds Shapes and not only Rects.
        if let Some(expected) = expected {
            self.trial(|pass| pass.fits(&ret, expected));
        }
        for (i, (arg, param)) in args.iter().zip(&params).enumerate() {
            let param = self.vars.resolve(param);
            let what = match name {
                Some(name) => format!("argument {} of `{name}`", i + 1),
                None => format!("argument {} of the call", i + 1),
            };
            self.check(arg, &param, &what);
        }
        self.vars.resolve(&ret)
    }

    fn unary(&mut self, op: UnaryOp, operand: &Expr) -> Type {
        let ty = self.synth(operand);
        let resolved = self.vars.shallow(&ty);
        match (op, &resolved) {
            (_, Type::Unknown) => Type::Unknown,
            (UnaryOp::Neg, Type::Int | Type::Float) => resolved,
            (UnaryOp::Neg, Type::Var(_)) => {
                let number = self.vars.fresh(true);
                self.fits(&resolved, &number);
                resolved
            }
            (UnaryOp::Not, _) if self.fits(&resolved, &Type::Bool) => Type::Bool,
            (UnaryOp::Neg, other) => {
                let got = self.show(other);
                self.type_error(format!(
                    "operand of `-` must be an Int or a Float, got {got}"
                ));
                Type::Unknown
            }
            (UnaryOp::Not, other) => {
                let got = self.show(other);
                self.type_error(format!("operand of `!` must be a Bool, got {got}"));
                Type::Bool
            }
        }
    }

    fn binary(&mut self, left: &Expr, op: BinaryOp, right: &Expr) -> Type {
        use BinaryOp::*;
        let a = self.synth(left);
        let b = self.synth(right);
        if let And | Or = op {
            for operand in [&a, &b] {
                if !self.fits(operand, &Type::Bool) {
                    let got = self.show(operand);
                    self.type_error(format!("operands of `{op}` must be Bool, got {got}"));
                }
            }
            return Type::Bool;
        }

        let result = match op {
            Eq | Ne | Lt | Le | Gt | Ge => Type::Bool,
            _ => Type::Unknown,
        };
        let (a, b) = (self.vars.shallow(&a), self.vars.shallow(&b));
        if a == Type::Unknown || b == Type::Unknown {
            return result;
        }
        if let Eq | Ne = op {
            if matches!(a, Type::Function(..)) && matches!(b, Type::Function(..)) {
                self.type_error("functions cannot be compared".to_string());
            } else if !(self.trial(|p| p.fits(&a, &b))
                || self.trial(|p| p.fits(&b, &a))
                || (a.is_symbolic() && b.is_symbolic()))
            {
                self.operands(op, &a, &b, "two values of the same type");
            }
            return result;
        }

        let expected = match op {
            Add => "two Ints, two Floats or two Strings",
            _ => "two Ints or two Floats",
        };
        if !(self.trial(|p| p.fits(&a, &b)) || self.trial(|p| p.fits(&b, &a))) {
            self.operands(op, &a, &b, expected);
            return result;
        }
        let ty = self.vars.shallow(&a);
        let fits = match &ty {
            Type::Int | Type::Float => true,
            Type::Str => op == Add,
            Type::Var(var) if op == Add || self.vars.is_numeric(*var) => true,
            Type::Var(_) => {
                let number = self.vars.fresh(true);
                self.fits(&ty, &number)
            }
            _ => false,
        };
        if !fits {
            self.operands(op, &a, &b, expected);
            return result;
        }
        match result {
            Type::Bool => Type::Bool,
            _ => self.vars.resolve(&ty),
        }
    }

    fn operands(&mut self, op: BinaryOp, a: &Type, b: &Type, expected: &str) {
        let (a, b) = (self.show(a), self.show(b));
        self.type_error(format!(
            "operands of `{op}` must be {expected}, got {a} and {b}"
        ));
    }

    fn field(&mut self, target: &Expr, field: &str) -> Type {
        let ty = self.synth(target);
        match self.vars.shallow(&ty) {
            Type::Struct(name) => match self.field_type(&name, field) {
                Some(ty) => ty,
                None => {
                    self.type_error(format!("`{name}` has no field `{field}`"));
                    Type::Unknown
                }
            },
            // A field of a union is one all its members have, of one type.
            Type::Union(name) => {
                let members = self.types.unions[&name].clone();
                let types: Vec<Option<Type>> = members
                    .iter()
                    .map(|member| match member {
                        Type::Struct(member) => self.field_type(member, field),
                        _ => None,
                    })
                    .collect();
                match types.first() {
                    Some(Some(first)) if types.iter().all(|t| t.as_ref() == Some(first)) => {
                        first.clone()
                    }
                    _ => {
                        self.type_error(format!(
                            "not every member of `{name}` has a field `{field}` of the same type"
                        ));
                        Type::Unknown
                    }
                }
            }
            Type::Unknown | Type::Var(_) => Type::Unknown,
            other => {
                let got = self.show(&other);
                self.type_error(format!("{got} has no fields"));
                Type::Unknown
            }
        }
    }

    fn structure(&mut self, name: &str, base: Option<&Expr>, fields: &[(String, Expr)]) -> Type {
        let declared = self.struct_fields(name);
        if let Some(base) = base {
            let ty = self.synth(base);
            if declared.is_some() && !self.fits(&ty, &Type::Struct(name.to_string())) {
                let got = self.show(&ty);
                self.type_error(format!(
                    "`{name} {{ x | ... }}` needs a {name} as `x`, got {got}"
                ));
            }
        }
        let Some(declared) = declared else {
            for (_, value) in fields {
                self.synth(value);
            }
            return Type::Unknown;
        };
        for (field, value) in fields {
            match declared.iter().find(|(f, _)| f == field) {
                Some((_, ty)) => {
                    let what = format!("field `{field}` of `{name}`");
                    self.check(value, ty, &what);
                }
                None => {
                    self.synth(value);
                    self.name_error(format!("`{name}` has no field `{field}`"));
                }
            }
        }
        if base.is_none() {
            for (field, _) in &declared {
                if !fields.iter().any(|(given, _)| given == field) {
                    self.name_error(format!("missing field `{field}` in `{name}`"));
                }
            }
        }
        Type::Struct(name.to_string())
    }

    /// The fields of the struct a literal builds, if it is declared where
    /// the literal can see it.
    fn struct_fields(&mut self, name: &str) -> Option<Vec<(String, Type)>> {
        let visible = self.declared.structs.contains(name)
            || (self.body.is_some() && self.ahead.structs.contains(name));
        if !visible {
            let message = match self.ahead.structs.contains(name) {
                true => used_before(name),
                false => format!("undefined struct `{name}`"),
            };
            self.name_error(message);
            return None;
        }
        self.types.structs.get(name).cloned()
    }

    /// The types of the parameters of the sequence a `start` starts, if it
    /// is declared where the `start` can see it.
    fn sequence(&mut self, name: &str) -> Option<Vec<Type>> {
        let found = (self.declared.sequences.get(name))
            .or(match self.body {
                Some(_) => self.ahead.sequences.get(name),
                None => None,
            })
            .cloned();
        if found.is_none() {
            let message = match self.ahead.sequences.contains_key(name) {
                true => used_before(name),
                false => format!("undefined sequence `{name}`"),
            };
            self.name_error(message);
        }
        found
    }

    fn declare(&mut self, name: &str, ty: Type) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_string(), ty));
        }
    }

    /// The type of a name used as a value: the innermost local with it, or
    /// what the top level declares.
    fn value(&mut self, name: &str) -> Type {
        let local = self.scopes.iter().flatten().rev().find(|(n, _)| n == name);
        if let Some((_, ty)) = local {
            return ty.clone();
        }
        let global = self.declared.values.get(name).or(match self.body {
            Some(_) => self.ahead.values.get(name),
            None => None,
        });
        if let Some(ty) = global.cloned() {
            return self.vars.instantiate(&ty);
        }

        let later = self.ahead.values.contains_key(name) || self.lets.contains_key(name);
        let message = if let (Some(kind), Some(line)) = (self.body, self.lets.get(name)) {
            format!(
                "`{name}` is declared after this {kind}, on line {line}: a {kind} can only use a top level `let` declared before it"
            )
        } else if later && self.body.is_none() {
            used_before(name)
        } else if self.declared.structs.contains(name) || self.ahead.structs.contains(name) {
            struct_hint(name)
        } else if self.declared.sequences.contains_key(name)
            || self.ahead.sequences.contains_key(name)
        {
            sequence_hint(name)
        } else if let Some(symbol) = self.declared.enums.get(name).or(self.ahead.enums.get(name)) {
            enum_hint(name, symbol)
        } else {
            format!("undefined name `{name}`")
        };
        self.name_error(message);
        Type::Unknown
    }

    fn arity(&mut self, name: &str, expected: usize, got: usize) {
        if let Err(error) = check_arity(name, expected, got) {
            self.name_error(error.message);
        }
    }

    /// Whether a value of type `actual` fits where `expected` is, finding
    /// what variables stand for to make it fit.
    fn fits(&mut self, actual: &Type, expected: &Type) -> bool {
        let actual = self.vars.shallow(actual);
        let expected = self.vars.shallow(expected);
        match (&actual, &expected) {
            (Type::Unknown, _) | (_, Type::Unknown) => true,
            (Type::Var(a), Type::Var(b)) if a == b => true,
            (Type::Var(var), other) | (other, Type::Var(var)) => {
                self.vars.bind(*var, other.clone())
            }
            (Type::Symbols(have), Type::Symbols(want)) => have.is_subset(want),
            (Type::Symbols(_) | Type::Enum(_), Type::Symbol) => true,
            (Type::Symbols(have), Type::Enum(name)) => {
                let symbols = &self.types.enums[name];
                have.iter().all(|symbol| symbols.contains(symbol))
            }
            // Values do not change, so a list of Rects is a list of Shapes.
            (Type::List(a), Type::List(b)) | (Type::Option(a), Type::Option(b)) => self.fits(a, b),
            (Type::Result(a, b), Type::Result(c, d)) => self.fits(a, c) && self.fits(b, d),
            (Type::Function(have, a), Type::Function(want, b)) => {
                have.len() == want.len()
                    && want.iter().zip(have).all(|(w, h)| self.fits(w, h))
                    && self.fits(a, b)
            }
            (Type::Union(a), Type::Union(b)) if a == b => true,
            (Type::Union(name), _) => {
                let members = self.types.unions[name].clone();
                members.iter().all(|member| self.fits(member, &expected))
            }
            (_, Type::Union(name)) => {
                let members = self.types.unions[name].clone();
                members
                    .iter()
                    .any(|member| self.trial(|pass| pass.fits(&actual, member)))
            }
            (a, b) => a == b,
        }
    }

    /// Runs a test of whether types fit, keeping what it finds about the
    /// variables only if they do.
    fn trial(&mut self, test: impl FnOnce(&mut Self) -> bool) -> bool {
        let snapshot = self.vars.snapshot();
        let fits = test(self);
        if !fits {
            self.vars.rollback(snapshot);
        }
        fits
    }

    /// The type of something that is one value or the other, as the branches
    /// of an `if`: the more general of the two, if one fits in the other.
    fn join(&mut self, a: &Type, b: &Type) -> Option<Type> {
        let (a, b) = (self.vars.resolve(a), self.vars.resolve(b));
        if let (Type::Symbols(x), Type::Symbols(y)) = (&a, &b) {
            return Some(Type::Symbols(x.union(y).cloned().collect()));
        }
        if self.trial(|pass| pass.fits(&a, &b)) {
            Some(self.vars.resolve(&b))
        } else if self.trial(|pass| pass.fits(&b, &a)) {
            Some(self.vars.resolve(&a))
        } else if a.is_symbolic() && b.is_symbolic() {
            Some(Type::Symbol)
        } else {
            None
        }
    }

    fn join_or_error(&mut self, a: &Type, b: &Type, what: &str) -> Type {
        match self.join(a, b) {
            Some(ty) => ty,
            None => {
                let (a, b) = (self.show(a), self.show(b));
                self.type_error(format!("{what} have different types, {a} and {b}"));
                Type::Unknown
            }
        }
    }

    fn show(&self, ty: &Type) -> String {
        match self.vars.resolve(ty) {
            Type::Var(var) if self.vars.is_numeric(var) => "Int or Float".to_string(),
            ty => ty.to_string(),
        }
    }

    fn name_error(&mut self, message: String) {
        self.error(Kind::Name, message);
    }

    fn type_error(&mut self, message: String) {
        self.error(Kind::Type, message);
    }

    fn error(&mut self, kind: Kind, message: String) {
        self.errors.push(CheckError {
            kind,
            message,
            line: self.line,
        });
    }
}

fn used_before(name: &str) -> String {
    format!("`{name}` is used before it is declared: the top level runs from top to bottom")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    fn statements(source: &str) -> Vec<Stmt> {
        parse(scan_tokens(source).expect("source should scan")).expect("source should parse")
    }

    /// The errors of checking `source` as a whole file, one per line.
    fn errors(source: &str) -> String {
        match Checker::new().check(&statements(source)) {
            Ok(()) => String::new(),
            Err(errors) => {
                let lines: Vec<String> = errors
                    .iter()
                    .map(|e| format!("[line {}] {}", e.line, e.message))
                    .collect();
                lines.join("\n")
            }
        }
    }

    fn passes(sources: &[&str]) {
        for source in sources {
            assert_eq!(errors(source), "", "{source:?}");
        }
    }

    fn rejects(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(errors(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn a_body_can_use_what_the_program_declares_anywhere() {
        passes(&[
            "f :: () -> Int { g() }\ng :: () -> Int { 1 }",
            "is_even :: (n: Int) -> Bool { n == 0 or is_odd(n - 1) }\nis_odd :: (n: Int) -> Bool { n != 0 and is_even(n - 1) }",
            "origin :: () -> P { P { x: 0, y: 0 } }\nP :: struct { x: Int, y: Int }",
            "kick :: (b: Int) -> Int {\n  start push(b)\n  b\n}\npush :: sequence (b: Int) {}",
            // Parameters, locals, `let`s of a block and names bound by `case`.
            "let x = 1\nf :: (a: Int) -> Int {\n  let b = a + x\n  case b {\n    n when n > 0 -> n\n    _ -> {\n      let c = 0\n      c\n    }\n  }\n}",
            "s :: sequence (b: Ball, to: Float) {\n  let from = b.x\n  over 1.0s as t {\n    b.x = from + to * t\n  }\n}\nBall :: struct { x: Float }",
            "range(0, 3) |> map(Some)\nprint(None)",
            "twice :: (f: (Int) -> Int, x: Int) -> Int { f(f(x)) }",
        ]);
    }

    #[test]
    fn reports_names_that_are_not_declared() {
        rejects(&[
            ("x + 1", "[line 1] undefined name `x`"),
            ("f :: () -> Int {\n  let a = 1\n  b\n}", "[line 3] undefined name `b`"),
            ("case 1 {\n  n -> m\n}", "[line 2] undefined name `m`"),
            ("{\n  let a = 1\n  a\n}\na", "[line 5] undefined name `a`"),
            (
                "Ball :: struct { x: Float }\nBall\nS :: enum { :a }\nS\ns :: sequence (n: Int) {}\ns(1)",
                "[line 2] `Ball` is a struct, not a value: build one with `Ball { ... }`\n\
                 [line 4] `S` is an enum, not a value: its values are its symbols, like `:a`\n\
                 [line 6] `s` is a sequence, not a function: it runs over time and cannot be called",
            ),
        ]);
    }

    #[test]
    fn the_top_level_runs_in_order() {
        rejects(&[
            (
                "let a = f()\nf :: () -> Int { 1 }",
                "[line 1] `f` is used before it is declared: the top level runs from top to bottom",
            ),
            (
                "x\nlet x = 1",
                "[line 1] `x` is used before it is declared: the top level runs from top to bottom",
            ),
            (
                "start s(1)\ns :: sequence (n: Int) {}\nB { x: 1 }\nB :: struct { x: Int }",
                "[line 1] `s` is used before it is declared: the top level runs from top to bottom\n\
                 [line 3] `B` is used before it is declared: the top level runs from top to bottom",
            ),
            (
                "scaled :: (x: Int) -> Int { x * SCALE }\nlet SCALE = 10",
                "[line 1] `SCALE` is declared after this function, on line 2: a function can only use a top level `let` declared before it",
            ),
        ]);
    }

    #[test]
    fn reports_calls_and_structs_that_do_not_fit() {
        rejects(&[
            (
                "add :: (a: Int, b: Int) -> Int { a + b }\nadd(1)\n2 |> add(3, 4)\nabs(1, 2)",
                "[line 2] `add` expects 2 arguments, got 1\n\
                 [line 3] `add` expects 2 arguments, got 3\n\
                 [line 4] `abs` expects 1 argument, got 2",
            ),
            (
                "s :: sequence (b: Int) {}\nstart s()\nstart t(1)",
                "[line 2] `s` expects 1 argument, got 0\n[line 3] undefined sequence `t`",
            ),
            (
                "B :: struct { x: Float, y: Float }\nB { x: 1.0 }\nB { x: 1.0, y: 2.0, z: 3.0 }\nlet b = B { x: 1.0, y: 1.0 }\nB { b | y: 2.0 }\nW { a: 1 }",
                "[line 2] missing field `y` in `B`\n\
                 [line 3] `B` has no field `z`\n\
                 [line 6] undefined struct `W`",
            ),
        ]);
    }

    #[test]
    fn the_repl_checks_each_input_after_the_ones_before() {
        let mut checker = Checker::new();
        let mut input = |source: &str| {
            checker
                .check(&statements(source))
                .map_err(|errors| errors[0].message.clone())
        };
        assert_eq!(input("let x = 1"), Ok(()));
        assert_eq!(input("x + 1"), Ok(()));
        // A function needs what it uses declared before, or in its own input.
        assert_eq!(
            input("f :: () -> Int { g() }"),
            Err("undefined name `g`".into())
        );
        // An input with errors does not run, so it declares nothing.
        assert_eq!(input("f()"), Err("undefined name `f`".into()));
        assert_eq!(input("g :: () -> Int { 2 }"), Ok(()));
        assert_eq!(input("f :: () -> Int { g() }"), Ok(()));
        assert_eq!(input("f()"), Ok(()));
    }

    #[test]
    fn the_examples_pass_after_the_host_prelude() {
        for example in [
            include_str!("../../../examples/square.ludo"),
            include_str!("../../../examples/pong.ludo"),
        ] {
            let mut checker = Checker::new();
            for source in [include_str!("../../play/src/host.ludo"), example] {
                checker.check(&statements(source)).unwrap();
            }
        }
    }

    #[test]
    fn values_of_the_right_types_pass() {
        passes(&[
            // A generic native finds its types at each call.
            "double :: (x: Int) -> Int { x * 2 }\nadd :: (a: Int, b: Int) -> Int { a + b }\nrange(0, 5) |> map(double) |> fold(0, add)",
            "twice :: (f: (Int) -> Int, x: Int) -> Int { f(f(x)) }\ndouble :: (x: Int) -> Int { x * 2 }\ntwice(double, 3)",
            "abs(-1)\nabs(-1.5)\nmin(2, 3) + 1\nclamp(0.5, 0.0, 1.0) * 2.0",
            // A symbol fits an enum that has it.
            "State :: enum { :idle, :serving }\nnext :: (s: State) -> State { if s == :idle { :serving } else { :idle } }\nnext(:idle)",
            // A list of one member fits a list of the union.
            "A :: struct { x: Float }\nB :: struct { x: Float, y: Float }\nAB :: A | B\n\
             all :: () -> List[AB] { concat([A { x: 1.0 }], [B { x: 1.0, y: 2.0 }]) }\n\
             first_x :: (v: AB) -> Float { v.x }",
            "half :: (n: Int) -> Result[Int, String] { if n % 2 == 0 { Ok(n / 2) } else { Error(\"odd\") } }\n\
             case half(4) { Ok(n) -> n, Error(_) -> 0 }\nis_two :: (n: Int) -> Bool { n == 2 }\nfind([1, 2], is_two) == Some(2)",
            // An empty list takes its type from where it is used.
            "let xs = []\nconcat(xs, [1])\nsum :: (xs: List[Int]) -> Int { 0 }\nsum([])",
            "Ball :: struct { x: Float }\ns :: sequence (b: Ball, to: Float) {\n  let from = b.x\n  over 0.5s as t {\n    b.x = from + to * t\n  }\n}",
        ]);
    }

    #[test]
    fn reports_values_of_the_wrong_type() {
        rejects(&[
            (
                "1 + 2.0",
                "[line 1] operands of `+` must be two Ints, two Floats or two Strings, got Int and Float",
            ),
            ("f :: (x: Int) -> String { x * 2 }", "[line 1] the value of `f` must be String, got Int"),
            (
                "add :: (a: Int, b: Int) -> Int { a + b }\nadd(1, \"2\")\nabs(\"x\")\nmin(1, 2.0)",
                "[line 2] argument 2 of `add` must be Int, got String\n\
                 [line 3] argument 1 of `abs` must be Int or Float, got String\n\
                 [line 4] argument 2 of `min` must be Int, got Float",
            ),
            (
                "if 1 { 2 } else { 3 }\nif true { 1 } else { \"a\" }\n[1, \"a\"]",
                "[line 1] the condition of `if` must be a Bool, got Int\n\
                 [line 2] the branches of `if` have different types, Int and String\n\
                 [line 3] the items of a list have different types, Int and String",
            ),
            (
                "B :: struct { x: Float }\nB { x: 1 }\nlet b = B { x: 1.0 }\nb.y\nlet n = 1\nn.x\nn(2)",
                "[line 2] field `x` of `B` must be Float, got Int\n\
                 [line 4] `B` has no field `y`\n\
                 [line 6] Int has no fields\n\
                 [line 7] can only call functions, got Int",
            ),
            (
                "State :: enum { :idle }\nf :: (s: State) -> Int { 1 }\nf(:busy)\nprint(1) == print(\"a\")",
                "[line 3] argument 1 of `f` must be State, got :busy\n\
                 [line 4] operands of `==` must be two values of the same type, got Int and String",
            ),
            (
                "case 1 {\n  :a -> 1\n  Some(x) -> x\n  _ -> 2\n}",
                "[line 2] the pattern `:a` can never match a value of type Int\n\
                 [line 3] a `Some` pattern needs an Option, got Int",
            ),
            (
                "A :: struct { x: Float }\nB :: struct { y: Float }\nAB :: A | B\nf :: (v: AB) -> Float { v.x }",
                "[line 4] not every member of `AB` has a field `x` of the same type",
            ),
            (
                "s :: sequence (b: Int) {\n  wait 1\n}\nBall :: struct { x: Float }\nt :: sequence (b: Ball) {\n  b.x = 1\n}",
                "[line 2] a duration must be a Float in seconds, got Int\n\
                 [line 6] field `x` of `Ball` must be Float, got Int",
            ),
        ]);
    }

    #[test]
    fn reports_types_that_do_not_exist() {
        rejects(&[
            (
                "f :: (x: Foo) -> List { 1 }",
                "[line 1] undefined type `Foo`\n[line 1] `List` takes 1 type, got 0",
            ),
            (
                "A :: B | Int\nB :: A | Int",
                "[line 1] `A` is made of itself\n[line 2] `B` is made of itself",
            ),
        ]);
    }

    #[test]
    fn a_color_that_play_does_not_know_is_reported_before_it_runs() {
        let mut checker = Checker::new();
        checker
            .check(&statements(include_str!("../../play/src/host.ludo")))
            .unwrap();
        let errors = checker
            .check(&statements(
                "[Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0, color: :yelow }]",
            ))
            .unwrap_err();
        assert_eq!(
            errors[0].to_string(),
            "[line 1] Type Error: field `color` of `Rect` must be Paint, got :yelow"
        );
    }
}
