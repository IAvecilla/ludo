use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Symbol(String),
    Bool(bool),
    Ident(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(Box<Expr>, BinaryOp, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Field(Box<Expr>, String),
    Block(Vec<Stmt>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let(String, Expr),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StmtKind::Let(name, value) => write!(f, "(let {name} {value})"),
            StmtKind::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "not",
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
        })
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Int(v) => write!(f, "{v}"),
            Expr::Float(v) => write!(f, "{v:?}"),
            Expr::Str(v) => write!(f, "{v:?}"),
            Expr::Symbol(v) => write!(f, ":{v}"),
            Expr::Bool(v) => write!(f, "{v}"),
            Expr::Ident(v) => write!(f, "{v}"),
            Expr::Unary(op, e) => write!(f, "({op} {e})"),
            Expr::Binary(l, op, r) => write!(f, "({op} {l} {r})"),
            Expr::Field(e, name) => write!(f, "(. {e} {name})"),
            Expr::Block(stmts, tail) => {
                f.write_str("(block")?;
                for stmt in stmts {
                    write!(f, " {stmt}")?;
                }
                write!(f, " {tail})")
            }
            Expr::Call(callee, args) => {
                write!(f, "(call {callee}")?;
                for arg in args {
                    write!(f, " {arg}")?;
                }
                f.write_str(")")
            }
        }
    }
}
