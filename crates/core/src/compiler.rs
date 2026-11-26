use std::fmt;
use std::rc::Rc;

use crate::ast::{
    Arm, BinaryOp, Expr, FnDecl, Param, Pattern, Step, StepKind, Stmt, StmtKind, UnaryOp, Variant,
};
use crate::chunk::{Chunk, Function, Op, Shape, Test};
use crate::value::Value;

/// An error found while compiling. Every construct of ludo compiles now, so
/// none is reported yet; the checks planned before running will report here.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub message: String,
    pub line: usize,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[line {}] Compile Error: {}", self.line, self.message)
    }
}

/// Compiles a top-level statement into a function with no name, the script
/// that runs it. An expression returns its value; a `let` or a declaration
/// binds a global and returns nothing. The VM keeps the globals from one
/// script to the next.
pub fn compile(stmt: &Stmt) -> Result<Function, CompileError> {
    let mut compiler = Compiler::new(stmt.line);
    match &stmt.kind {
        StmtKind::Expr(expr) => compiler.expression(expr)?,
        _ => compiler.statement(stmt)?,
    }
    compiler.emit(Op::Return);
    Ok(compiler.finish(None))
}

/// A local variable while its block is being compiled.
struct Local {
    name: String,
    /// How many blocks deep it was declared, to know which block ends it.
    depth: usize,
    /// Where it lives on the stack.
    slot: usize,
}

struct Compiler {
    chunk: Chunk,
    line: usize,
    arity: usize,
    /// The locals in scope, innermost last, so the newest of several with
    /// the same name is found first.
    locals: Vec<Local>,
    scope_depth: usize,
    /// How many values the stack holds at this point of the code. In clox a
    /// block is a statement, so it starts with only locals on the stack and a
    /// local's slot is its index among them. In ludo a block is an
    /// expression and can start above temporaries, like the `1` in
    /// `1 + { let a = 2 ... }`, so a local's slot comes from this height.
    height: usize,
    /// While compiling a sequence, the name of its state, its first
    /// parameter.
    state: Option<String>,
}

impl Compiler {
    fn new(line: usize) -> Self {
        Compiler {
            chunk: Chunk::default(),
            line,
            arity: 0,
            locals: Vec::new(),
            scope_depth: 0,
            height: 0,
            state: None,
        }
    }

    /// A compiler of its own for each function, as in clox. Its parameters
    /// are its first locals, from slot 0 up: the frame starts at the first
    /// argument, as in ploxb, and the function sits right below.
    fn function(line: usize, params: &[Param]) -> Self {
        let mut compiler = Compiler::new(line);
        compiler.arity = params.len();
        compiler.scope_depth = 1;
        for param in params {
            compiler.height += 1;
            compiler.add_local(&param.name);
        }
        compiler
    }

    fn finish(self, name: Option<&str>) -> Function {
        Function {
            name: name.map(str::to_string),
            arity: self.arity,
            captured: Vec::new(),
            chunk: self.chunk,
        }
    }

    /// Declares the value on top of the stack as a local.
    fn add_local(&mut self, name: &str) {
        self.locals.push(Local {
            name: name.to_string(),
            depth: self.scope_depth,
            slot: self.height - 1,
        });
    }

    fn emit(&mut self, op: Op) {
        self.chunk.write(op, self.line);
        self.height = self
            .height
            .checked_add_signed(stack_effect(op, &self.chunk))
            .expect("the compiler never pops an empty stack");
    }

    /// Compiles a function written with `fn` and makes a closure of it. It
    /// keeps the locals in scope here that its body names, which go on the
    /// stack above the function for `Closure` to take. In its own chunk they
    /// are locals right after its parameters, where the VM puts them when it
    /// is called. A local of the function around it is a local here too, so
    /// a closure inside another one finds what it keeps.
    fn lambda(&mut self, decl: &FnDecl) -> Result<(), CompileError> {
        let mut names = Vec::new();
        mentioned(&decl.body, &mut names);
        let (captured, slots): (Vec<String>, Vec<usize>) = names
            .into_iter()
            .filter(|name| decl.params.iter().all(|p| &p.name != name))
            .filter_map(|name| self.resolve_local(&name).map(|slot| (name, slot)))
            .unzip();
        let mut compiler = Compiler::function(self.line, &decl.params);
        for name in &captured {
            compiler.height += 1;
            compiler.add_local(name);
        }
        compiler.expression(&decl.body)?;
        compiler.emit(Op::Return);
        let mut function = compiler.finish(Some(&decl.name));
        function.captured = captured;
        self.constant(Value::Function(Rc::new(function)));
        for slot in &slots {
            self.emit(Op::GetLocal(*slot));
        }
        self.emit(Op::Closure(slots.len()));
        Ok(())
    }

    fn constant(&mut self, value: Value) {
        let index = self.chunk.add_constant(value);
        self.emit(Op::Constant(index));
    }

    /// Stores a global's name as a string constant, for the instructions
    /// that look it up when they run.
    fn identifier_constant(&mut self, name: &str) -> usize {
        self.chunk.add_constant(Value::Str(name.to_string()))
    }

    /// Emits a jump whose target is not known yet, and returns where it is
    /// so `patch_jump` can fill it in.
    fn emit_jump(&mut self, jump: fn(usize) -> Op) -> usize {
        self.emit(jump(usize::MAX));
        self.chunk.code.len() - 1
    }

    /// Points the jump at `at` to the next instruction to be emitted.
    fn patch_jump(&mut self, at: usize) {
        let distance = self.chunk.code.len() - at - 1;
        match &mut self.chunk.code[at] {
            Op::Jump(target) | Op::JumpIfFalse(target) => *target = distance,
            op => unreachable!("only jumps are patched, got {op:?}"),
        }
    }

    fn begin_scope(&mut self) {
        self.scope_depth += 1;
    }

    /// Forgets the locals of the scope that ends, and says how many there
    /// were.
    fn close_scope(&mut self) -> usize {
        self.scope_depth -= 1;
        let inner = self.locals.iter().rev();
        let count = inner.take_while(|l| l.depth > self.scope_depth).count();
        self.locals.truncate(self.locals.len() - count);
        count
    }

    /// Drops the locals of the block that ends, which sit right below its
    /// value on the stack.
    fn end_scope(&mut self) {
        let count = self.close_scope();
        if count > 0 {
            self.emit(Op::PopBelow(count));
        }
    }

    /// Drops the locals of a scope that leaves no value, like the body of an
    /// `over`, with a `Pop` each as clox does.
    fn drop_scope(&mut self) {
        for _ in 0..self.close_scope() {
            self.emit(Op::Pop);
        }
    }

    /// The slot of the innermost local with this name. Without one, the name
    /// is a global.
    fn resolve_local(&self, name: &str) -> Option<usize> {
        let local = self.locals.iter().rev().find(|local| local.name == name);
        local.map(|local| local.slot)
    }

    /// Compiles a statement, which leaves the stack as it found it except
    /// for the new local of a `let` inside a block.
    fn statement(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        let outer = std::mem::replace(&mut self.line, stmt.line);
        match &stmt.kind {
            StmtKind::Let(name, value) if self.scope_depth == 0 => {
                let global = self.identifier_constant(name);
                self.expression(value)?;
                self.emit(Op::DefineGlobal(global));
            }
            // The value stays where it was pushed and becomes the local. It
            // is declared only now, so its own initializer still sees the
            // outer variable with that name.
            StmtKind::Let(name, value) => {
                self.expression(value)?;
                self.add_local(name);
            }
            StmtKind::Expr(expr) => {
                self.expression(expr)?;
                self.emit(Op::Pop);
            }
            // The parser only lets functions be declared at the top level, so
            // they are always globals.
            StmtKind::Fn(decl) => {
                let global = self.identifier_constant(&decl.name);
                let mut compiler = Compiler::function(stmt.line, &decl.params);
                compiler.expression(&decl.body)?;
                compiler.emit(Op::Return);
                let function = compiler.finish(Some(&decl.name));
                self.constant(Value::Function(Rc::new(function)));
                self.emit(Op::DefineGlobal(global));
            }
            // Only the names of the fields matter when the program runs.
            StmtKind::Struct(decl) => {
                let fields = decl.fields.iter().map(|f| f.name.clone()).collect();
                let shape = self.shape(&decl.name, fields);
                self.emit(Op::DefineStruct(shape));
            }
            StmtKind::Enum(decl) => {
                let shape = self.shape(&decl.name, decl.symbols.clone());
                self.emit(Op::DefineEnum(shape));
            }
            // A union only tells the checker what a type can be.
            StmtKind::Union(_) => {}
            // A sequence compiles like a function whose body is its steps.
            // Its state, the first parameter, sits in slot 0, where the VM
            // puts the world before each frame and reads it back after. It
            // ends with `End` instead of `Return`, so the state stays there.
            StmtKind::Sequence(decl) => {
                let name = self.identifier_constant(&decl.name);
                let mut compiler = Compiler::function(stmt.line, &decl.params);
                compiler.state = decl.params.first().map(|param| param.name.clone());
                for step in &decl.body {
                    compiler.step(step)?;
                }
                compiler.emit(Op::End);
                let sequence = compiler.finish(Some(&decl.name));
                self.constant(Value::Function(Rc::new(sequence)));
                self.emit(Op::DefineSequence(name));
            }
            StmtKind::Start(name, args) => {
                for arg in args {
                    self.expression(arg)?;
                }
                let name = self.identifier_constant(name);
                let argc = args.len();
                self.emit(Op::Start { name, argc });
            }
        }
        self.line = outer;
        Ok(())
    }

    fn shape(&mut self, name: &str, fields: Vec<String>) -> usize {
        let name = name.to_string();
        self.chunk.add_shape(Shape { name, fields })
    }

    /// Compiles a step of a sequence, which, like a statement, leaves the
    /// stack as it found it except for a new local.
    fn step(&mut self, step: &Step) -> Result<(), CompileError> {
        let outer = std::mem::replace(&mut self.line, step.line);
        match &step.kind {
            // Binding the state's name again replaces the state: the newest
            // binding is what the game gets.
            StepKind::Stmt(Stmt {
                kind: StmtKind::Let(name, value),
                ..
            }) if self.state.as_ref() == Some(name) => {
                self.expression(value)?;
                self.emit(Op::SetState);
            }
            StepKind::Stmt(stmt) => self.statement(stmt)?,
            StepKind::Set(target, field, value) => {
                self.expression(value)?;
                let state = self.identifier_constant(target);
                let field = self.identifier_constant(field);
                self.emit(Op::SetField { state, field });
            }
            StepKind::Over {
                duration,
                var,
                body,
            } => self.over(duration, var.as_deref(), body)?,
        }
        self.line = outer;
        Ok(())
    }

    /// Compiles an `over`, or a `wait`, which is an `over` with no body, into
    /// a loop of passes, one per frame of the game.
    fn over(
        &mut self,
        duration: &Expr,
        var: Option<&str>,
        body: &[Step],
    ) -> Result<(), CompileError> {
        // If this frame already had a pass, the duration waits for the next
        // one too, since it is evaluated at the first pass.
        self.emit(Op::Yield);
        self.expression(duration)?;
        self.emit(Op::OverStart);
        let counters = self.height - 2;

        let top = self.chunk.code.len();
        self.begin_scope();
        self.emit(Op::OverPass(counters));
        // `t` is a local of the body, with no name when there is no `as`.
        self.add_local(var.unwrap_or_default());
        for step in body {
            self.step(step)?;
        }
        self.drop_scope();
        let distance = self.chunk.code.len() + 1 - top;
        self.emit(Op::OverLoop { counters, distance });

        self.emit(Op::Pop);
        self.emit(Op::Pop);
        Ok(())
    }

    /// Compiles a `case`. Its value stays on the stack as a hidden local while
    /// each branch tests it in turn. A branch that does not apply drops what
    /// it pushed and goes on to the next one; one that does leaves its value
    /// and jumps to the end, where the hidden local goes.
    fn case(&mut self, value: &Expr, arms: &[Arm]) -> Result<(), CompileError> {
        self.expression(value)?;
        let slot = self.height - 1;
        let line = self.line;
        let mut ends = Vec::new();
        for arm in arms {
            self.line = arm.line;
            // Only the branch before can reach here, when it does not apply,
            // with the value alone on the stack.
            self.height = slot + 1;
            self.begin_scope();
            let mut mismatches = Vec::new();
            if arm.pattern != Pattern::Wildcard {
                self.emit(Op::GetLocal(slot));
                self.pattern(&arm.pattern, &mut mismatches);
            }
            let bound = self.height - slot - 1;
            let mut unmet = Vec::new();
            if let Some(guard) = &arm.guard {
                self.expression(guard)?;
                self.emit(Op::CheckBool(Test::When));
                unmet.push(self.emit_jump(Op::JumpIfFalse));
                self.emit(Op::Pop);
            }
            self.expression(&arm.body)?;
            self.end_scope();
            ends.push(self.emit_jump(Op::Jump));

            // A guard that does not hold leaves the bindings and its Bool.
            // With no bindings that is what a pattern that does not match
            // leaves, the Bool alone, and both share the same `Pop`.
            if bound == 0 {
                mismatches.append(&mut unmet);
            }
            let mut skip = None;
            if !unmet.is_empty() {
                for jump in unmet {
                    self.patch_jump(jump);
                }
                self.height = slot + bound + 2;
                for _ in 0..=bound {
                    self.emit(Op::Pop);
                }
                if !mismatches.is_empty() {
                    skip = Some(self.emit_jump(Op::Jump));
                }
            }
            if !mismatches.is_empty() {
                for jump in mismatches {
                    self.patch_jump(jump);
                }
                self.height = slot + 2;
                self.emit(Op::Pop);
            }
            if let Some(skip) = skip {
                self.patch_jump(skip);
            }
        }
        self.line = line;
        self.height = slot + 1;
        self.emit(Op::NoMatch);
        for end in ends {
            self.patch_jump(end);
        }
        self.height = slot + 2;
        self.emit(Op::PopBelow(1));
        Ok(())
    }

    /// Tests the value on top of the stack against a pattern, taking it. If
    /// it matches, what the pattern binds stays on the stack as locals. Every
    /// test that can fail adds its jump to `mismatches`, and leaves the stack
    /// as it was below the value, with a false on top: a pattern binds at
    /// most one name, at its innermost, so nothing is bound when a test fails.
    fn pattern(&mut self, pattern: &Pattern, mismatches: &mut Vec<usize>) {
        match pattern {
            Pattern::Wildcard => self.emit(Op::Pop),
            Pattern::Bind(name) => self.add_local(name),
            Pattern::Literal(literal) => {
                let index = self.chunk.add_constant(Value::from(literal));
                self.emit(Op::MatchConstant(index));
                mismatches.push(self.emit_jump(Op::JumpIfFalse));
                self.emit(Op::Pop);
            }
            Pattern::Variant(variant, inner) => {
                self.emit(Op::MatchVariant(*variant));
                mismatches.push(self.emit_jump(Op::JumpIfFalse));
                self.emit(Op::Pop);
                if let Some(inner) = inner {
                    self.pattern(inner, mismatches);
                }
            }
        }
    }

    fn expression(&mut self, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::Int(v) => self.constant(Value::Int(*v)),
            Expr::Float(v) => self.constant(Value::Float(*v)),
            Expr::Str(v) => self.constant(Value::Str(v.clone())),
            Expr::Symbol(v) => self.constant(Value::Symbol(v.clone())),
            Expr::Bool(true) => self.emit(Op::True),
            Expr::Bool(false) => self.emit(Op::False),
            Expr::Unary(op, right) => {
                self.expression(right)?;
                self.emit(match op {
                    UnaryOp::Neg => Op::Negate,
                    UnaryOp::Not => Op::Not,
                });
            }
            // If the left side is false, so is the whole `and`: it stays as
            // the value and the right side is skipped.
            Expr::Binary(left, BinaryOp::And, right) => {
                self.expression(left)?;
                self.emit(Op::CheckBool(Test::And));
                let end = self.emit_jump(Op::JumpIfFalse);
                self.emit(Op::Pop);
                self.expression(right)?;
                self.emit(Op::CheckBool(Test::And));
                self.patch_jump(end);
            }
            // If the left side is true, so is the whole `or`. clox has no
            // jump on true, so a false one jumps over a jump to the end.
            Expr::Binary(left, BinaryOp::Or, right) => {
                self.expression(left)?;
                self.emit(Op::CheckBool(Test::Or));
                let else_jump = self.emit_jump(Op::JumpIfFalse);
                let end = self.emit_jump(Op::Jump);
                self.patch_jump(else_jump);
                self.emit(Op::Pop);
                self.expression(right)?;
                self.emit(Op::CheckBool(Test::Or));
                self.patch_jump(end);
            }
            Expr::Binary(left, op, right) => {
                self.expression(left)?;
                self.expression(right)?;
                self.emit(Op::Binary(*op));
            }
            Expr::Ident(name) => match self.resolve_local(name) {
                Some(slot) => self.emit(Op::GetLocal(slot)),
                None => {
                    let global = self.identifier_constant(name);
                    self.emit(Op::GetGlobal(global));
                }
            },
            Expr::Block(statements, tail, line) => {
                self.begin_scope();
                for stmt in statements {
                    self.statement(stmt)?;
                }
                let outer = std::mem::replace(&mut self.line, *line);
                self.expression(tail)?;
                self.end_scope();
                self.line = outer;
            }
            // The callee goes first, then its arguments above it, so the frame
            // of the call starts at the first argument.
            Expr::Call(callee, args) => {
                self.expression(callee)?;
                for arg in args {
                    self.expression(arg)?;
                }
                self.emit(Op::Call(args.len()));
            }
            Expr::Field(target, name) => {
                self.expression(target)?;
                let field = self.identifier_constant(name);
                self.emit(Op::GetField(field));
            }
            // Each branch pops the condition and leaves its own value. Both
            // paths reach the end with the stack one value higher, so the
            // height the compiler counts holds after the jumps too.
            Expr::If(cond, then, otherwise) => {
                self.expression(cond)?;
                self.emit(Op::CheckBool(Test::If));
                let then_jump = self.emit_jump(Op::JumpIfFalse);
                self.emit(Op::Pop);
                self.expression(then)?;
                let else_jump = self.emit_jump(Op::Jump);
                self.patch_jump(then_jump);
                self.emit(Op::Pop);
                self.expression(otherwise)?;
                self.patch_jump(else_jump);
            }
            Expr::Case(value, arms) => self.case(value, arms)?,
            Expr::Lambda(decl) => self.lambda(decl)?,
            Expr::List(items) => {
                for item in items {
                    self.expression(item)?;
                }
                self.emit(Op::List(items.len()));
            }
            // The struct being updated goes first and the fields above it, in
            // the order they are written. Checking them against the
            // declaration waits until `Struct` runs.
            Expr::Struct { name, base, fields } => {
                if let Some(base) = base {
                    self.expression(base)?;
                }
                for (_, value) in fields {
                    self.expression(value)?;
                }
                let names = fields.iter().map(|(field, _)| field.clone()).collect();
                let shape = self.shape(name, names);
                let base = base.is_some();
                self.emit(Op::Struct { shape, base });
            }
        }
        Ok(())
    }
}

/// Adds to `names` every name that `expr` uses as a value, once each, also
/// inside the functions written in it. Some may be bound again inside, like
/// `x` in `{ let x = 1 x }`, and keeping a copy of the outer one then only
/// costs that copy.
fn mentioned(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Symbol(_) | Expr::Bool(_) => {}
        Expr::Ident(name) => {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        Expr::Unary(_, operand) | Expr::Field(operand, _) => mentioned(operand, names),
        Expr::Binary(left, _, right) => {
            mentioned(left, names);
            mentioned(right, names);
        }
        Expr::Call(callee, args) => {
            mentioned(callee, names);
            args.iter().for_each(|arg| mentioned(arg, names));
        }
        Expr::Block(statements, tail, _) => {
            for stmt in statements {
                match &stmt.kind {
                    StmtKind::Let(_, value) | StmtKind::Expr(value) => mentioned(value, names),
                    StmtKind::Start(_, args) => args.iter().for_each(|arg| mentioned(arg, names)),
                    // Declarations only go at the top level.
                    _ => {}
                }
            }
            mentioned(tail, names);
        }
        Expr::If(cond, then, otherwise) => {
            mentioned(cond, names);
            mentioned(then, names);
            mentioned(otherwise, names);
        }
        Expr::List(items) => items.iter().for_each(|item| mentioned(item, names)),
        Expr::Struct { base, fields, .. } => {
            if let Some(base) = base {
                mentioned(base, names);
            }
            fields.iter().for_each(|(_, value)| mentioned(value, names));
        }
        Expr::Case(value, arms) => {
            mentioned(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    mentioned(guard, names);
                }
                mentioned(&arm.body, names);
            }
        }
        Expr::Lambda(decl) => mentioned(&decl.body, names),
    }
}

/// How many values an instruction pushes minus how many it pops.
fn stack_effect(op: Op, chunk: &Chunk) -> isize {
    match op {
        Op::Constant(_) | Op::True | Op::False | Op::GetGlobal(_) | Op::GetLocal(_) => 1,
        // `Return` ends the chunk, so nothing after it counts the stack.
        Op::Negate | Op::Not | Op::CheckBool(_) | Op::Jump(_) | Op::JumpIfFalse(_) | Op::Return => {
            0
        }
        Op::DefineGlobal(_) | Op::Pop | Op::Binary(_) => -1,
        Op::PopBelow(count) => -(count as isize),
        // It takes the callee and its arguments, and leaves the result.
        Op::Call(argc) => -(argc as isize),
        Op::List(count) => 1 - count as isize,
        // It takes the function and what it keeps, and leaves the closure.
        Op::Closure(count) => -(count as isize),
        Op::Struct { shape, base } => {
            1 - chunk.shapes[shape].fields.len() as isize - isize::from(base)
        }
        Op::DefineStruct(_) | Op::GetField(_) => 0,
        Op::DefineEnum(_) | Op::MatchConstant(_) | Op::NoMatch => 0,
        // When it matches, a `Some`, `Ok` or `Error` leaves what it holds and
        // the Bool; a `None` leaves only the Bool.
        Op::MatchVariant(Variant::None) => 0,
        Op::MatchVariant(_) => 1,
        Op::Start { argc, .. } => -(argc as isize),
        Op::Yield | Op::OverLoop { .. } | Op::End => 0,
        // It takes the duration and leaves the two counters.
        Op::OverStart => 1,
        Op::OverPass(_) => 1,
        Op::DefineSequence(_) | Op::SetField { .. } | Op::SetState => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::scanner::scan_tokens;

    /// The bytecode of the first statement of `source`, followed by that of
    /// the functions it declares, as `ludo dis` shows it.
    fn listing(source: &str) -> String {
        let statements = parse(scan_tokens(source).unwrap()).unwrap();
        let script = compile(&statements[0]).unwrap();
        let mut listing = script.chunk.to_string();
        for constant in &script.chunk.constants {
            if let Value::Function(function) = constant {
                listing += &format!("== {function} ==\n{}", function.chunk);
            }
        }
        listing
    }

    fn disassembles(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(listing(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn compiles_expressions_and_globals() {
        disassembles(&[
            (
                "\n\"a\" + \"b\" == \"ab\"",
                "0000    2 Constant            0 \"a\"\n\
                 0001    | Constant            1 \"b\"\n\
                 0002    | Add\n\
                 0003    | Constant            2 \"ab\"\n\
                 0004    | Equal\n\
                 0005    | Return\n",
            ),
            (
                "let speed = 2 * dt",
                "0000    1 Constant            1 2\n\
                 0001    | GetGlobal           2 \"dt\"\n\
                 0002    | Mul\n\
                 0003    | DefineGlobal        0 \"speed\"\n\
                 0004    | Return\n",
            ),
            // A block's locals live in stack slots above the temporaries
            // under it, and go below its value when it ends.
            (
                "let r = 1 + {\n  let a = 2\n  a * a\n}",
                "0000    1 Constant            1 1\n\
                 0001    2 Constant            2 2\n\
                 0002    3 GetLocal            1\n\
                 0003    | GetLocal            1\n\
                 0004    | Mul\n\
                 0005    | PopBelow            1\n\
                 0006    1 Add\n\
                 0007    | DefineGlobal        0 \"r\"\n\
                 0008    | Return\n",
            ),
        ]);
    }

    #[test]
    fn jumps_over_what_does_not_run() {
        disassembles(&[
            (
                "if c { 1 } else { 2 }",
                "0000    1 GetGlobal           0 \"c\"\n\
                 0001    | CheckBool          if\n\
                 0002    | JumpIfFalse         3 -> 6\n\
                 0003    | Pop\n\
                 0004    | Constant            1 1\n\
                 0005    | Jump                2 -> 8\n\
                 0006    | Pop\n\
                 0007    | Constant            2 2\n\
                 0008    | Return\n",
            ),
            (
                "a or b",
                "0000    1 GetGlobal           0 \"a\"\n\
                 0001    | CheckBool          or\n\
                 0002    | JumpIfFalse         1 -> 4\n\
                 0003    | Jump                3 -> 7\n\
                 0004    | Pop\n\
                 0005    | GetGlobal           1 \"b\"\n\
                 0006    | CheckBool          or\n\
                 0007    | Return\n",
            ),
        ]);
    }

    /// The functions in the constants of a chunk.
    fn functions(chunk: &Chunk) -> Vec<Rc<Function>> {
        chunk
            .constants
            .iter()
            .filter_map(|constant| match constant {
                Value::Function(function) => Some(function.clone()),
                _ => None,
            })
            .collect()
    }

    fn declared(source: &str) -> Rc<Function> {
        let statements = parse(scan_tokens(source).unwrap()).unwrap();
        functions(&compile(&statements[0]).unwrap().chunk)[0].clone()
    }

    #[test]
    fn a_lambda_keeps_the_locals_it_names() {
        let f = declared("f :: (k: Int, unused: Int) -> Fn {\n  fn(x: Int) -> Int { x * k }\n}");
        assert_eq!(
            f.chunk.code,
            [Op::Constant(0), Op::GetLocal(0), Op::Closure(1), Op::Return]
        );
        let lambda = &functions(&f.chunk)[0];
        assert_eq!(
            (lambda.arity, lambda.captured.clone()),
            (1, vec!["k".to_string()])
        );
        // What it keeps sits right after its parameter.
        assert_eq!(
            lambda.chunk.code,
            [
                Op::GetLocal(0),
                Op::GetLocal(1),
                Op::Binary(BinaryOp::Mul),
                Op::Return
            ]
        );

        // The middle one keeps `a` for the inner one, which keeps it and `b`.
        let g = declared(
            "g :: (a: Int) -> Fn {\n  fn(b: Int) -> Fn { fn(c: Int) -> Int { a + b + c } }\n}",
        );
        let middle = &functions(&g.chunk)[0];
        let inner = &functions(&middle.chunk)[0];
        assert_eq!(middle.captured, ["a"]);
        assert_eq!(inner.captured, ["a", "b"]);

        // A parameter with the name of a local around it hides that local.
        let h = declared("h :: (x: Int) -> Fn {\n  fn(x: Int) -> Int { x }\n}");
        assert!(functions(&h.chunk)[0].captured.is_empty());
    }

    #[test]
    fn functions_and_sequences_have_their_own_chunks() {
        disassembles(&[
            (
                "add :: (a: Int, b: Int) -> Int { a + b }",
                "0000    1 Constant            1 <fn add>\n\
                 0001    | DefineGlobal        0 \"add\"\n\
                 0002    | Return\n\
                 == <fn add> ==\n\
                 0000    1 GetLocal            0\n\
                 0001    | GetLocal            1\n\
                 0002    | Add\n\
                 0003    | Return\n",
            ),
            (
                "add(1, 2)",
                "0000    1 GetGlobal           0 \"add\"\n\
                 0001    | Constant            1 1\n\
                 0002    | Constant            2 2\n\
                 0003    | Call                2\n\
                 0004    | Return\n",
            ),
            (
                "s :: sequence (b: Ball) {\n  over 0.5s as t {\n    b.x = t\n  }\n}",
                "0000    1 Constant            1 <fn s>\n\
                 0001    | DefineSequence      0 \"s\"\n\
                 0002    | Return\n\
                 == <fn s> ==\n\
                 0000    2 Yield\n\
                 0001    | Constant            0 0.5\n\
                 0002    | OverStart\n\
                 0003    | OverPass            1\n\
                 0004    3 GetLocal            3\n\
                 0005    | SetField         b.x\n\
                 0006    2 Pop\n\
                 0007    | OverLoop            1 -> 3\n\
                 0008    | Pop\n\
                 0009    | Pop\n\
                 0010    1 End\n",
            ),
        ]);
    }

    #[test]
    fn a_struct_literal_puts_its_values_on_the_stack_before_building_it() {
        disassembles(&[
            (
                "Ball :: struct { x: Float, y: Float }",
                "0000    1 DefineStruct        0 Ball { x, y }\n\
                 0001    | Return\n",
            ),
            (
                "Ball { b | y: b.y + 1.0 }",
                "0000    1 GetGlobal           0 \"b\"\n\
                 0001    | GetGlobal           1 \"b\"\n\
                 0002    | GetField            2 \"y\"\n\
                 0003    | Constant            3 1.0\n\
                 0004    | Add\n\
                 0005    | Struct              0 Ball { _ | y }\n\
                 0006    | Return\n",
            ),
        ]);
    }

    #[test]
    fn a_case_tests_each_branch_and_drops_what_a_failed_one_pushed() {
        disassembles(&[
            (
                "case s { :a -> 1, n when n > 2 -> n, _ -> 0 }",
                "0000    1 GetGlobal           0 \"s\"\n\
                 0001    | GetLocal            0\n\
                 0002    | MatchConstant       1 :a\n\
                 0003    | JumpIfFalse         3 -> 7\n\
                 0004    | Pop\n\
                 0005    | Constant            2 1\n\
                 0006    | Jump               16 -> 23\n\
                 0007    | Pop\n\
                 0008    | GetLocal            0\n\
                 0009    | GetLocal            1\n\
                 0010    | Constant            3 2\n\
                 0011    | Greater\n\
                 0012    | CheckBool        when\n\
                 0013    | JumpIfFalse         4 -> 18\n\
                 0014    | Pop\n\
                 0015    | GetLocal            1\n\
                 0016    | PopBelow            1\n\
                 0017    | Jump                5 -> 23\n\
                 0018    | Pop\n\
                 0019    | Pop\n\
                 0020    | Constant            4 0\n\
                 0021    | Jump                1 -> 23\n\
                 0022    | NoMatch\n\
                 0023    | PopBelow            1\n\
                 0024    | Return\n",
            ),
            // A nested pattern is a chain of tests.
            (
                "case r { Ok(Some(n)) -> n, _ -> 0 }",
                "0000    1 GetGlobal           0 \"r\"\n\
                 0001    | GetLocal            0\n\
                 0002    | MatchVariant       Ok\n\
                 0003    | JumpIfFalse         7 -> 11\n\
                 0004    | Pop\n\
                 0005    | MatchVariant     Some\n\
                 0006    | JumpIfFalse         4 -> 11\n\
                 0007    | Pop\n\
                 0008    | GetLocal            1\n\
                 0009    | PopBelow            1\n\
                 0010    | Jump                4 -> 15\n\
                 0011    | Pop\n\
                 0012    | Constant            1 0\n\
                 0013    | Jump                1 -> 15\n\
                 0014    | NoMatch\n\
                 0015    | PopBelow            1\n\
                 0016    | Return\n",
            ),
        ]);
    }
}
