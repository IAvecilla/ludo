use crate::token::{Token, TokenKind};

const DURATION_SUFFIX: &str = "s";

#[derive(Debug, Clone, PartialEq)]
pub struct ScanError {
    pub message: String,
    pub line: usize,
}

pub fn scan_tokens(source: &str) -> Result<Vec<Token>, Vec<ScanError>> {
    Scanner::new(source).run()
}

struct Scanner {
    chars: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
    open: Vec<char>,
    tokens: Vec<Token>,
    errors: Vec<ScanError>,
}

impl Scanner {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            start: 0,
            current: 0,
            line: 1,
            open: Vec::new(),
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, Vec<ScanError>> {
        while !self.is_at_end() {
            self.start = self.current;
            self.scan_token();
        }

        self.start = self.current;
        self.add_token(TokenKind::Eof);

        if self.errors.is_empty() {
            Ok(self.tokens)
        } else {
            Err(self.errors)
        }
    }

    fn scan_token(&mut self) {
        let c = self.advance();
        let kind = match c {
            ' ' | '\r' | '\t' => return,
            '\n' => {
                // A newline ends a statement, except inside `( )` or `[ ]` and when the
                // next line starts with `|>` or `.`, which continue the current one.
                let in_group = matches!(self.open.last(), Some('(' | '['));
                if in_group || self.next_line_continues() {
                    self.line += 1;
                    return;
                }
                TokenKind::Newline
            }

            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,

            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '%' => TokenKind::Percent,

            '-' if self.matches('>') => TokenKind::Arrow,
            '-' => TokenKind::Minus,
            '=' if self.matches('=') => TokenKind::EqEq,
            '=' => TokenKind::Eq,
            '<' if self.matches('=') => TokenKind::LessEq,
            '<' => TokenKind::Less,
            '>' if self.matches('=') => TokenKind::GreaterEq,
            '>' => TokenKind::Greater,
            '|' if self.matches('>') => TokenKind::PipeGt,
            '|' => TokenKind::Bar,
            '!' if self.matches('=') => TokenKind::BangEq,
            '!' => TokenKind::Bang,

            '/' if self.matches('/') => {
                while !self.is_at_end() && self.peek() != '\n' {
                    self.advance();
                }
                return;
            }
            '/' => TokenKind::Slash,

            ':' if self.matches(':') => TokenKind::ColonColon,
            ':' if is_symbol_start(self.peek()) => self.symbol(),
            ':' => TokenKind::Colon,

            '"' => match self.string() {
                Ok(kind) => kind,
                Err(message) => return self.error(message),
            },
            c if c.is_ascii_digit() => match self.number() {
                Ok(kind) => kind,
                Err(message) => return self.error(message),
            },
            c if is_ident_start(c) => self.identifier(),

            other => return self.error(format!("unexpected character `{other}`")),
        };

        match kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => self.open.push(c),
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                self.open.pop();
            }
            _ => {}
        }
        self.add_token(kind);
        if c == '\n' {
            self.line += 1;
        }
    }

    fn next_line_continues(&self) -> bool {
        let mut i = self.current;
        while matches!(self.chars.get(i), Some(' ' | '\t' | '\r' | '\n')) {
            i += 1;
        }
        matches!(
            (self.chars.get(i), self.chars.get(i + 1)),
            (Some('.'), _) | (Some('|'), Some('>'))
        )
    }

    fn string(&mut self) -> Result<TokenKind, String> {
        while !self.is_at_end() && self.peek() != '"' && self.peek() != '\n' {
            self.advance();
        }

        if self.is_at_end() || self.peek() == '\n' {
            return Err(format!("unterminated string: `{}`", self.lexeme()));
        }

        self.advance();
        let value = self.chars[self.start + 1..self.current - 1]
            .iter()
            .collect();
        Ok(TokenKind::Str(value))
    }

    fn symbol(&mut self) -> TokenKind {
        while is_ident_continue(self.peek()) {
            self.advance();
        }
        TokenKind::Symbol(self.chars[self.start + 1..self.current].iter().collect())
    }

    fn number(&mut self) -> Result<TokenKind, String> {
        while self.peek().is_ascii_digit() {
            self.advance();
        }

        let mut is_float = self.peek() == '.' && self.peek_next().is_ascii_digit();
        if is_float {
            self.advance();
            while self.peek().is_ascii_digit() {
                self.advance();
            }
        }

        let text = self.lexeme();

        if is_ident_start(self.peek()) {
            let suffix_start = self.current;
            while is_ident_continue(self.peek()) {
                self.advance();
            }
            let suffix: String = self.chars[suffix_start..self.current].iter().collect();
            if suffix != DURATION_SUFFIX {
                return Err(format!(
                    "unknown numeric suffix `{suffix}` in `{}`",
                    self.lexeme()
                ));
            }
            is_float = true;
        }

        if is_float {
            text.parse()
                .map(TokenKind::Float)
                .map_err(|_| format!("invalid float literal `{text}`"))
        } else {
            text.parse()
                .map(TokenKind::Int)
                .map_err(|_| format!("integer literal `{text}` does not fit in 64 bits"))
        }
    }

    fn identifier(&mut self) -> TokenKind {
        while is_ident_continue(self.peek()) {
            self.advance();
        }

        let text = self.lexeme();
        match text.as_str() {
            "let" => TokenKind::Let,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "case" => TokenKind::Case,
            "when" => TokenKind::When,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "true" => TokenKind::True,
            "false" => TokenKind::False,

            "sequence" => TokenKind::Sequence,
            "over" => TokenKind::Over,
            "wait" => TokenKind::Wait,
            "start" => TokenKind::Start,
            "as" => TokenKind::As,

            _ => TokenKind::Ident(text),
        }
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.chars.len()
    }

    fn advance(&mut self) -> char {
        let c = self.chars[self.current];
        self.current += 1;
        c
    }

    fn matches(&mut self, expected: char) -> bool {
        if self.is_at_end() || self.chars[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn peek(&self) -> char {
        self.chars.get(self.current).copied().unwrap_or('\0')
    }

    fn peek_next(&self) -> char {
        self.chars.get(self.current + 1).copied().unwrap_or('\0')
    }

    fn lexeme(&self) -> String {
        self.chars[self.start..self.current].iter().collect()
    }

    fn add_token(&mut self, kind: TokenKind) {
        let lexeme = self.lexeme();
        self.tokens.push(Token::new(kind, lexeme, self.line));
    }

    fn error(&mut self, message: String) {
        self.errors.push(ScanError {
            message,
            line: self.line,
        });
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_symbol_start(c: char) -> bool {
    c.is_ascii_lowercase() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> String {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        assert_eq!(tokens.last().map(|t| &t.kind), Some(&TokenKind::Eof));
        let kinds: Vec<String> = tokens[..tokens.len() - 1]
            .iter()
            .map(|t| format!("{:?}", t.kind))
            .collect();
        kinds.join(" ")
    }

    fn scans(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(kinds(source), *expected, "{source:?}");
        }
    }

    #[test]
    fn scans_operators_and_numbers() {
        scans(&[
            (
                "1 + 2 * 3 % 4 / 2.0",
                "Int(1) Plus Int(2) Star Int(3) Percent Int(4) Slash Float(2.0)",
            ),
            (
                "== != <= >= -> |> | = < > !",
                "EqEq BangEq LessEq GreaterEq Arrow PipeGt Bar Eq Less Greater Bang",
            ),
        ]);
    }

    #[test]
    fn scans_keywords_and_names() {
        scans(&[
            (
                "sequence sequencer over overflow on only",
                r#"Sequence Ident("sequencer") Over Ident("overflow") Ident("on") Ident("only")"#,
            ),
            ("_ _unused", r#"Ident("_") Ident("_unused")"#),
        ]);
    }

    #[test]
    fn tells_symbols_from_annotations() {
        scans(&[
            (":jump", r#"Symbol("jump")"#),
            ("f :: :jump", r#"Ident("f") ColonColon Symbol("jump")"#),
            ("pos: Vec2", r#"Ident("pos") Colon Ident("Vec2")"#),
            ("pos:Vec2", r#"Ident("pos") Colon Ident("Vec2")"#),
            (
                "Player { p | hp: 1 }",
                r#"Ident("Player") LBrace Ident("p") Bar Ident("hp") Colon Int(1) RBrace"#,
            ),
        ]);
    }

    #[test]
    fn scans_strings_and_comments() {
        scans(&[
            (r#""hello""#, r#"Str("hello")"#),
            (r#""""#, r#"Str("")"#),
            (
                r#""let x |> 1 // not a comment""#,
                r#"Str("let x |> 1 // not a comment")"#,
            ),
            (r#""a\nb""#, r#"Str("a\\nb")"#),
            ("1 // 2 + 3", "Int(1)"),
            ("1 // 2\n3", "Int(1) Newline Int(3)"),
        ]);
    }

    #[test]
    fn a_newline_ends_a_statement_unless_it_is_grouped_or_continued() {
        scans(&[
            (
                "let x = 1\nx",
                r#"Let Ident("x") Eq Int(1) Newline Ident("x")"#,
            ),
            (
                "f(\n1,\n2\n)",
                r#"Ident("f") LParen Int(1) Comma Int(2) RParen"#,
            ),
            ("[1,\n2]", "LBracket Int(1) Comma Int(2) RBracket"),
            ("{\n1\n}", "LBrace Newline Int(1) Newline RBrace"),
            (
                "f({\n1\n})",
                r#"Ident("f") LParen LBrace Newline Int(1) Newline RBrace RParen"#,
            ),
            (
                "p\n  |> f\n  |> g",
                r#"Ident("p") PipeGt Ident("f") PipeGt Ident("g")"#,
            ),
            ("p\n  .f()", r#"Ident("p") Dot Ident("f") LParen RParen"#),
            ("a\n| b", r#"Ident("a") Newline Bar Ident("b")"#),
        ]);
    }

    #[test]
    fn tokens_keep_their_lexeme_and_line() {
        let tokens = scan_tokens("let dt = 0.2s").unwrap();
        let lexemes: Vec<&str> = tokens.iter().map(|t| t.lexeme.as_str()).collect();
        assert_eq!(lexemes, ["let", "dt", "=", "0.2s", ""]);

        let lines = |source: &str| -> Vec<usize> {
            scan_tokens(source)
                .unwrap()
                .iter()
                .map(|t| t.line)
                .collect()
        };
        assert_eq!(lines("let x\n\nx"), [1, 1, 1, 2, 3, 3]);
        assert_eq!(lines("f(\n1\n)"), [1, 1, 2, 3, 3]);
        assert_eq!(lines("p\n|> f"), [1, 2, 2, 2]);
    }
}
