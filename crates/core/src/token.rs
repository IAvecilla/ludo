#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64),
    Float(f64),
    Str(String),
    Symbol(String),
    Ident(String),
    Underscore,

    Let,
    Fn,
    Struct,
    Enum,
    On,
    If,
    Else,
    Case,
    When,
    For,
    In,
    Return,
    And,
    Or,
    Not,
    True,
    False,

    Sequence,
    Over,
    Wait,
    Start,
    As,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,

    Eq,
    EqEq,
    BangEq,
    Less,
    LessEq,
    Greater,
    GreaterEq,

    PipeGt,
    Bar,
    Arrow,

    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,

    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub line: usize,
}

impl Token {
    pub fn new(kind: TokenKind, lexeme: impl Into<String>, line: usize) -> Self {
        Self {
            kind,
            lexeme: lexeme.into(),
            line,
        }
    }
}
