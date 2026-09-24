use std::fmt;
use std::rc::Rc;

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
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Struct {
        name: String,
        base: Option<Box<Expr>>,
        fields: Vec<(String, Expr)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub name: String,
    pub args: Vec<TypeExpr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: TypeExpr,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeqDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    Stmt(StmtKind),
    Set(String, String, Expr),
    Over {
        duration: Expr,
        var: Option<String>,
        body: Vec<Step>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let(String, Expr),
    Fn(Rc<FnDecl>),
    Struct(Rc<StructDecl>),
    Sequence(Rc<SeqDecl>),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub line: usize,
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl fmt::Display for StmtKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StmtKind::Let(name, value) => write!(f, "(let {name} {value})"),
            StmtKind::Fn(decl) => write!(f, "{decl}"),
            StmtKind::Struct(decl) => write!(f, "{decl}"),
            StmtKind::Sequence(decl) => write!(f, "{decl}"),
            StmtKind::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

impl fmt::Display for StructDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct {} ({}))", self.name, params(&self.fields))
    }
}

impl fmt::Display for SeqDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(sequence {} ({})", self.name, params(&self.params))?;
        for step in &self.body {
            write!(f, " {step}")?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StepKind::Stmt(kind) => write!(f, "{kind}"),
            StepKind::Set(target, field, value) => write!(f, "(set {target}.{field} {value})"),
            StepKind::Over {
                duration,
                var,
                body,
            } => {
                write!(f, "(over {duration} {}", var.as_deref().unwrap_or("_"))?;
                for step in body {
                    write!(f, " {step}")?;
                }
                f.write_str(")")
            }
        }
    }
}

fn params(params: &[Param]) -> String {
    let params: Vec<String> = params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.ty))
        .collect();
    params.join(", ")
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if !self.args.is_empty() {
            let args: Vec<String> = self.args.iter().map(|a| a.to_string()).collect();
            write!(f, "[{}]", args.join(", "))?;
        }
        Ok(())
    }
}

impl fmt::Display for FnDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(fn {} ({}) -> {} {})",
            self.name,
            params(&self.params),
            self.ret,
            self.body
        )
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
            Expr::If(cond, then, otherwise) => write!(f, "(if {cond} {then} {otherwise})"),
            Expr::Block(stmts, tail) => {
                f.write_str("(block")?;
                for stmt in stmts {
                    write!(f, " {stmt}")?;
                }
                write!(f, " {tail})")
            }
            Expr::Struct { name, base, fields } => {
                write!(f, "({name}")?;
                if let Some(base) = base {
                    write!(f, " {base} |")?;
                }
                for (field, value) in fields {
                    write!(f, " {field}: {value}")?;
                }
                f.write_str(")")
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
