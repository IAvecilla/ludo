//! Checks the names a program uses before it runs, as jlox's resolver does in
//! chapter 11 of the book: a name that is not declared, a call with the wrong
//! number of arguments, a struct built with a field it does not have or
//! without one it needs. It walks the tree once, before either backend runs
//! it.
//!
//! The top level runs from top to bottom, so there a name has to be declared
//! before it is used. The body of a function or a sequence runs later, when it
//! is called or started, so it can use any function, struct, enum or sequence
//! of the program, but a top-level `let` only if it comes before.

use std::collections::HashMap;
use std::fmt;

use crate::ast::{Arm, Expr, Param, Pattern, Step, StepKind, Stmt, StmtKind};
use crate::interpreter::{check_arity, enum_hint, sequence_hint, struct_hint};
use crate::prelude::{constants, NATIVES};

#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    pub line: usize,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[line {}] Name Error: {}", self.line, self.message)
    }
}

/// What the top level declares, kept apart by kind as the backends do.
#[derive(Debug, Clone, Default)]
struct Declared {
    /// The functions, with how many arguments they take, and the values,
    /// with `None`.
    values: HashMap<String, Option<usize>>,
    /// The fields of each struct.
    structs: HashMap<String, Vec<String>>,
    /// The first symbol of each enum, for the hint when it is used as a value.
    enums: HashMap<String, String>,
    /// How many arguments each sequence takes.
    sequences: HashMap<String, usize>,
}

impl Declared {
    fn declare(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(name, _) => {
                self.values.insert(name.clone(), None);
            }
            StmtKind::Fn(decl) => {
                self.values
                    .insert(decl.name.clone(), Some(decl.params.len()));
            }
            StmtKind::Struct(decl) => {
                let fields = decl.fields.iter().map(|f| f.name.clone()).collect();
                self.structs.insert(decl.name.clone(), fields);
            }
            StmtKind::Enum(decl) => {
                self.enums
                    .insert(decl.name.clone(), decl.symbols[0].clone());
            }
            StmtKind::Sequence(decl) => {
                self.sequences.insert(decl.name.clone(), decl.params.len());
            }
            StmtKind::Start(..) | StmtKind::Expr(_) => {}
        }
    }
}

/// Checks the statements of a program as they come, keeping what they
/// declare for the ones after: a whole file at once, or the REPL one input at
/// a time.
#[derive(Debug, Clone)]
pub struct Checker {
    declared: Declared,
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
                .insert(native.name.to_string(), Some(native.arity));
        }
        for (name, _) in constants() {
            declared.values.insert(name.to_string(), None);
        }
        Checker { declared }
    }

    /// Checks statements that run one after another, after those checked
    /// before. If they have errors, what they declare is forgotten, since
    /// they will not run.
    pub fn check(&mut self, statements: &[Stmt]) -> Result<(), Vec<CheckError>> {
        let mut ahead = Declared::default();
        let mut lets = HashMap::new();
        for stmt in statements {
            match &stmt.kind {
                StmtKind::Let(name, _) => {
                    lets.entry(name.clone()).or_insert(stmt.line);
                }
                _ => ahead.declare(stmt),
            }
        }

        let mut pass = Pass {
            declared: self.declared.clone(),
            ahead,
            lets,
            scopes: Vec::new(),
            body: None,
            line: 0,
            errors: Vec::new(),
        };
        for stmt in statements {
            pass.top_level(stmt);
        }
        if pass.errors.is_empty() {
            self.declared = pass.declared;
            Ok(())
        } else {
            Err(pass.errors)
        }
    }
}

/// One walk of the checker over a group of statements.
struct Pass {
    /// What the top level has declared before the statement being checked.
    declared: Declared,
    /// Every function, struct, enum and sequence the statements declare,
    /// wherever they are, which a body can use.
    ahead: Declared,
    /// The line where each top-level `let` of the statements is first
    /// declared, to tell a `let` that comes later from a missing one.
    lets: HashMap<String, usize>,
    /// The locals in scope, innermost last.
    scopes: Vec<Vec<String>>,
    /// Inside the body of a function or a sequence, which of the two it is.
    body: Option<&'static str>,
    line: usize,
    errors: Vec<CheckError>,
}

impl Pass {
    fn top_level(&mut self, stmt: &Stmt) {
        self.line = stmt.line;
        match &stmt.kind {
            StmtKind::Let(_, value) => self.expression(value),
            StmtKind::Fn(decl) => {
                self.declared.declare(stmt);
                self.body(&decl.params, "function", |pass| pass.expression(&decl.body));
            }
            StmtKind::Sequence(decl) => {
                self.declared.declare(stmt);
                self.body(&decl.params, "sequence", |pass| {
                    for step in &decl.body {
                        pass.step(step);
                    }
                });
            }
            StmtKind::Struct(_) | StmtKind::Enum(_) => {}
            StmtKind::Start(..) | StmtKind::Expr(_) => self.statement(stmt),
        }
        // A `let` is declared once its value is checked, which cannot use it.
        self.declared.declare(stmt);
    }

    fn body(&mut self, params: &[Param], kind: &'static str, check: impl FnOnce(&mut Self)) {
        self.body = Some(kind);
        self.scopes
            .push(params.iter().map(|p| p.name.clone()).collect());
        check(self);
        self.scopes.pop();
        self.body = None;
    }

    fn statement(&mut self, stmt: &Stmt) {
        let outer = std::mem::replace(&mut self.line, stmt.line);
        match &stmt.kind {
            StmtKind::Let(name, value) => {
                self.expression(value);
                self.declare(name);
            }
            StmtKind::Start(name, args) => {
                for arg in args {
                    self.expression(arg);
                }
                if let Some(arity) = self.sequence(name) {
                    self.arity(name, arity, args.len());
                }
            }
            StmtKind::Expr(expr) => self.expression(expr),
            // The parser keeps declarations at the top level.
            StmtKind::Fn(_) | StmtKind::Struct(_) | StmtKind::Enum(_) | StmtKind::Sequence(_) => {}
        }
        self.line = outer;
    }

    fn step(&mut self, step: &Step) {
        let outer = std::mem::replace(&mut self.line, step.line);
        match &step.kind {
            StepKind::Stmt(stmt) => self.statement(stmt),
            StepKind::Set(_, _, value) => self.expression(value),
            StepKind::Over {
                duration,
                var,
                body,
            } => {
                self.expression(duration);
                self.scopes.push(var.iter().cloned().collect());
                for step in body {
                    self.step(step);
                }
                self.scopes.pop();
            }
        }
        self.line = outer;
    }

    fn expression(&mut self, expr: &Expr) {
        match expr {
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Symbol(_) | Expr::Bool(_) => {}
            Expr::Ident(name) => {
                self.value(name);
            }
            Expr::Unary(_, operand) => self.expression(operand),
            Expr::Binary(left, _, right) => {
                self.expression(left);
                self.expression(right);
            }
            Expr::Call(callee, args) => {
                let arity = match callee.as_ref() {
                    Expr::Ident(name) => self.value(name).map(|arity| (name, arity)),
                    other => {
                        self.expression(other);
                        None
                    }
                };
                for arg in args {
                    self.expression(arg);
                }
                if let Some((name, arity)) = arity {
                    self.arity(name, arity, args.len());
                }
            }
            Expr::Field(target, _) => self.expression(target),
            Expr::Block(statements, tail, line) => {
                self.scopes.push(Vec::new());
                for stmt in statements {
                    self.statement(stmt);
                }
                let outer = std::mem::replace(&mut self.line, *line);
                self.expression(tail);
                self.line = outer;
                self.scopes.pop();
            }
            Expr::If(cond, then, otherwise) => {
                self.expression(cond);
                self.expression(then);
                self.expression(otherwise);
            }
            Expr::List(items) => {
                for item in items {
                    self.expression(item);
                }
            }
            Expr::Struct { name, base, fields } => {
                if let Some(base) = base {
                    self.expression(base);
                }
                for (_, value) in fields {
                    self.expression(value);
                }
                if let Some(declared) = self.structure(name) {
                    for (field, _) in fields {
                        if !declared.contains(field) {
                            self.error(format!("`{name}` has no field `{field}`"));
                        }
                    }
                    if base.is_none() {
                        for field in &declared {
                            if !fields.iter().any(|(given, _)| given == field) {
                                self.error(format!("missing field `{field}` in `{name}`"));
                            }
                        }
                    }
                }
            }
            Expr::Case(value, arms) => {
                self.expression(value);
                for arm in arms {
                    self.arm(arm);
                }
            }
        }
    }

    fn arm(&mut self, arm: &Arm) {
        let outer = std::mem::replace(&mut self.line, arm.line);
        let mut bound = Vec::new();
        bindings(&arm.pattern, &mut bound);
        self.scopes.push(bound);
        if let Some(guard) = &arm.guard {
            self.expression(guard);
        }
        self.expression(&arm.body);
        self.scopes.pop();
        self.line = outer;
    }

    fn declare(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.to_string());
        }
    }

    /// Looks up a name used as a value. Gives how many arguments it takes if
    /// it is a function of the top level, for a call to check them.
    fn value(&mut self, name: &str) -> Option<usize> {
        if self.scopes.iter().flatten().any(|local| local == name) {
            return None;
        }
        if let Some(arity) = self.declared.values.get(name) {
            return *arity;
        }
        if self.body.is_some() {
            if let Some(arity) = self.ahead.values.get(name) {
                return *arity;
            }
        }

        let later = self.ahead.values.contains_key(name) || self.lets.contains_key(name);
        let message = if let (Some(kind), Some(line)) = (self.body, self.lets.get(name)) {
            format!(
                "`{name}` is declared after this {kind}, on line {line}: a {kind} can only use a top level `let` declared before it"
            )
        } else if later && self.body.is_none() {
            used_before(name)
        } else if self.known(|d| d.structs.contains_key(name)) {
            struct_hint(name)
        } else if self.known(|d| d.sequences.contains_key(name)) {
            sequence_hint(name)
        } else if let Some(symbol) = self.declared.enums.get(name).or(self.ahead.enums.get(name)) {
            enum_hint(name, symbol)
        } else {
            format!("undefined name `{name}`")
        };
        self.error(message);
        None
    }

    /// The fields of the struct a literal builds, if it is declared where
    /// the literal can see it.
    fn structure(&mut self, name: &str) -> Option<Vec<String>> {
        let found = (self.declared.structs.get(name))
            .or(match self.body {
                Some(_) => self.ahead.structs.get(name),
                None => None,
            })
            .cloned();
        if found.is_none() {
            let message = match self.ahead.structs.contains_key(name) {
                true => used_before(name),
                false => format!("undefined struct `{name}`"),
            };
            self.error(message);
        }
        found
    }

    /// How many arguments the sequence a `start` starts takes, if it is
    /// declared where the `start` can see it.
    fn sequence(&mut self, name: &str) -> Option<usize> {
        let found = (self.declared.sequences.get(name))
            .or(match self.body {
                Some(_) => self.ahead.sequences.get(name),
                None => None,
            })
            .copied();
        if found.is_none() {
            let message = match self.ahead.sequences.contains_key(name) {
                true => used_before(name),
                false => format!("undefined sequence `{name}`"),
            };
            self.error(message);
        }
        found
    }

    /// Whether a name is declared before or anywhere in the statements.
    fn known(&self, has: impl Fn(&Declared) -> bool) -> bool {
        has(&self.declared) || has(&self.ahead)
    }

    fn arity(&mut self, name: &str, expected: usize, got: usize) {
        if let Err(error) = check_arity(name, expected, got) {
            self.error(error.message);
        }
    }

    fn error(&mut self, message: String) {
        self.errors.push(CheckError {
            message,
            line: self.line,
        });
    }
}

fn used_before(name: &str) -> String {
    format!("`{name}` is used before it is declared: the top level runs from top to bottom")
}

/// The names a pattern binds.
fn bindings(pattern: &Pattern, bound: &mut Vec<String>) {
    match pattern {
        Pattern::Bind(name) => bound.push(name.clone()),
        Pattern::Variant(_, Some(inner)) => bindings(inner, bound),
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Variant(_, None) => {}
    }
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
            // A local can be called with any arguments: its type tells how many.
            "twice :: (f: Fn, x: Int) -> Int { f(f(x), 1) }",
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
}
