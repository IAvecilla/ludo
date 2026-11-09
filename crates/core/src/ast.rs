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
    Block(Vec<Stmt>, Box<Expr>, usize),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
    Struct {
        name: String,
        base: Option<Box<Expr>>,
        fields: Vec<(String, Expr)>,
    },
    /// `case value { pattern [when guard] -> body ... }`: the value of the
    /// first branch whose pattern matches and whose guard holds.
    Case(Box<Expr>, Vec<Arm>),
}

/// A branch of a `case`, with the line it starts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `_`, which matches anything.
    Wildcard,
    /// A name, which matches anything and binds it in the branch.
    Bind(String),
    /// A literal, which matches a value of its type equal to it.
    Literal(Literal),
    /// `Some(p)`, `None`, `Ok(p)` or `Error(p)`, which match that variant
    /// when what it holds, if anything, matches `p`.
    Variant(Variant, Option<Box<Pattern>>),
}

/// The variants of the built-in `Option` and `Result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Some,
    None,
    Ok,
    Error,
}

impl Variant {
    pub fn name(self) -> &'static str {
        match self {
            Variant::Some => "Some",
            Variant::None => "None",
            Variant::Ok => "Ok",
            Variant::Error => "Error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Str(String),
    Symbol(String),
    Bool(bool),
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

/// `Name :: enum { :a, :b }`: a closed set of symbols.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeqDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    Stmt(Stmt),
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
    Enum(Rc<EnumDecl>),
    Sequence(Rc<SeqDecl>),
    Start(String, Vec<Expr>),
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
            StmtKind::Enum(decl) => write!(f, "{decl}"),
            StmtKind::Sequence(decl) => write!(f, "{decl}"),
            StmtKind::Start(name, args) => write!(f, "(start {name}{})", spaced(args)),
            StmtKind::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

impl fmt::Display for StructDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct {} ({}))", self.name, params(&self.fields))
    }
}

impl fmt::Display for EnumDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(enum {}", self.name)?;
        for symbol in &self.symbols {
            write!(f, " :{symbol}")?;
        }
        f.write_str(")")
    }
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}", self.pattern)?;
        if let Some(guard) = &self.guard {
            write!(f, " when {guard}")?;
        }
        write!(f, " -> {})", self.body)
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard => f.write_str("_"),
            Pattern::Bind(name) => f.write_str(name),
            Pattern::Literal(literal) => write!(f, "{literal}"),
            Pattern::Variant(variant, Some(inner)) => write!(f, "{}({inner})", variant.name()),
            Pattern::Variant(variant, None) => f.write_str(variant.name()),
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int(v) => write!(f, "{v}"),
            Literal::Float(v) => write!(f, "{v:?}"),
            Literal::Str(v) => write!(f, "{v:?}"),
            Literal::Symbol(v) => write!(f, ":{v}"),
            Literal::Bool(v) => write!(f, "{v}"),
        }
    }
}

impl fmt::Display for SeqDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(sequence {} ({}){})",
            self.name,
            params(&self.params),
            spaced(&self.body)
        )
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            StepKind::Stmt(stmt) => write!(f, "{stmt}"),
            StepKind::Set(target, field, value) => write!(f, "(set {target}.{field} {value})"),
            StepKind::Over {
                duration,
                var,
                body,
            } => write!(
                f,
                "(over {duration} {}{})",
                var.as_deref().unwrap_or("_"),
                spaced(body)
            ),
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

fn spaced(items: &[impl fmt::Display]) -> String {
    items.iter().map(|item| format!(" {item}")).collect()
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
            UnaryOp::Not => "!",
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
            Expr::Block(statements, tail, _) => write!(f, "(block{} {tail})", spaced(statements)),
            Expr::List(items) => write!(f, "(list{})", spaced(items)),
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
            Expr::Call(callee, args) => write!(f, "(call {callee}{})", spaced(args)),
            Expr::Case(value, arms) => write!(f, "(case {value}{})", spaced(arms)),
        }
    }
}
