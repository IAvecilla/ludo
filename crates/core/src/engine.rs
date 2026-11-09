//! Runs a program on either backend, so the CLI and `play` can choose one:
//! the VM, which compiles each statement to bytecode first, or the
//! interpreter, which walks its tree.

use std::collections::HashMap;

use crate::ast::Stmt;
use crate::compiler::{compile, CompileError};
use crate::interpreter::{Interpreter, RuntimeError};
use crate::value::Value;
use crate::vm::Vm;

/// Which backend runs a program.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Vm,
    Interpreter,
}

/// Why a statement did not run.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Compile(CompileError),
    Runtime(RuntimeError),
}

/// A program running on one of the backends.
pub enum Engine {
    Vm(Vm),
    Interpreter(Interpreter),
}

impl Engine {
    pub fn new(backend: Backend) -> Self {
        match backend {
            Backend::Vm => Engine::Vm(Vm::new()),
            Backend::Interpreter => Engine::Interpreter(Interpreter::new()),
        }
    }

    pub fn backend(&self) -> Backend {
        match self {
            Engine::Vm(_) => Backend::Vm,
            Engine::Interpreter(_) => Backend::Interpreter,
        }
    }

    /// Runs a top-level statement and gives the value it left, if any.
    pub fn execute(&mut self, stmt: &Stmt) -> Result<Option<Value>, Failure> {
        match self {
            Engine::Vm(vm) => {
                let script = compile(stmt).map_err(Failure::Compile)?;
                vm.run(script).map_err(Failure::Runtime)
            }
            Engine::Interpreter(interpreter) => interpreter.execute(stmt).map_err(Failure::Runtime),
        }
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match self {
            Engine::Vm(vm) => vm.call_function(name, args),
            Engine::Interpreter(interpreter) => interpreter.call_function(name, args),
        }
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        match self {
            Engine::Vm(vm) => vm.arity(name),
            Engine::Interpreter(interpreter) => interpreter.arity(name),
        }
    }

    pub fn struct_shapes(&self) -> HashMap<String, Vec<String>> {
        match self {
            Engine::Vm(vm) => vm.struct_shapes(),
            Engine::Interpreter(interpreter) => interpreter.struct_shapes(),
        }
    }

    pub fn run_sequences(&mut self, world: Value, dt: f64) -> Result<Value, RuntimeError> {
        match self {
            Engine::Vm(vm) => vm.run_sequences(world, dt),
            Engine::Interpreter(interpreter) => interpreter.run_sequences(world, dt),
        }
    }

    /// Takes over the sequences running in `from`, which runs on the same
    /// backend: a reload never changes it.
    pub fn adopt_sequences(&mut self, from: &mut Engine) {
        match (self, from) {
            (Engine::Vm(vm), Engine::Vm(from)) => vm.adopt_sequences(from),
            (Engine::Interpreter(interpreter), Engine::Interpreter(from)) => {
                interpreter.adopt_sequences(from)
            }
            _ => unreachable!("a reload keeps the backend"),
        }
    }

    pub fn stop_sequences(&mut self) {
        match self {
            Engine::Vm(vm) => vm.stop_sequences(),
            Engine::Interpreter(interpreter) => interpreter.stop_sequences(),
        }
    }
}
