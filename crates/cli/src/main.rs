use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use ludo_core::interpreter::Interpreter;
use ludo_core::parser::parse;
use ludo_core::scanner::scan_tokens;

const EX_USAGE: u8 = 64;
const EX_DATAERR: u8 = 65;
const EX_SOFTWARE: u8 = 70;

enum Failure {
    Static,
    Runtime,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.len() {
        0 => {
            run_repl();
            ExitCode::SUCCESS
        }
        1 => run_file(&args[0]),
        _ => {
            eprintln!("usage: ludo [program.ludo]");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn run_file(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("could not read `{path}`: {err}");
            return ExitCode::from(EX_USAGE);
        }
    };

    match run(&source, &mut Interpreter::new(), true) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Static) => ExitCode::from(EX_DATAERR),
        Err(Failure::Runtime) => ExitCode::from(EX_SOFTWARE),
    }
}

fn run_repl() {
    let stdin = io::stdin();
    let mut interpreter = Interpreter::new();

    loop {
        print!("> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!("read error: {err}");
                break;
            }
        }

        let _ = run(&line, &mut interpreter, false);
    }
}

fn run(source: &str, interpreter: &mut Interpreter, show_lines: bool) -> Result<(), Failure> {
    let at = |line: usize| {
        if show_lines {
            format!("[line {line}] ")
        } else {
            String::new()
        }
    };

    let tokens = match scan_tokens(source) {
        Ok(tokens) => tokens,
        Err(errors) => {
            for error in &errors {
                eprintln!("{}Scanning Error: {}", at(error.line), error.message);
            }
            return Err(Failure::Static);
        }
    };

    let stmts = match parse(tokens) {
        Ok(stmts) => stmts,
        Err(errors) => {
            for error in &errors {
                eprintln!("{}Parsing Error: {}", at(error.line), error.message);
            }
            return Err(Failure::Static);
        }
    };

    for stmt in &stmts {
        match interpreter.execute(stmt) {
            Ok(Some(value)) => println!("{value}"),
            Ok(None) => {}
            Err(error) => {
                let line = error.line.map(at).unwrap_or_default();
                eprintln!("{line}Runtime Error: {}", error.message);
                return Err(Failure::Runtime);
            }
        }
    }
    Ok(())
}
