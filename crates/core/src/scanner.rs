use crate::token::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ScanError {
    pub message: String,
    pub line: usize,
}

pub fn scan_tokens(source: &str) -> Result<Vec<Token>, Vec<ScanError>> {
    let mut scanner = Scanner {
        chars: source.chars().collect(),
        start: 0,
        current: 0,
        line: 1,
        open: Vec::new(),
        tokens: Vec::new(),
    };
    let mut errors = Vec::new();
    while !scanner.is_at_end() {
        scanner.start = scanner.current;
        if let Err(message) = scanner.scan_token() {
            let line = scanner.line;
            errors.push(ScanError { message, line });
        }
    }
    scanner.start = scanner.current;
    scanner.add_token(TokenKind::Eof);

    if errors.is_empty() {
        Ok(scanner.tokens)
    } else {
        Err(errors)
    }
}

struct Scanner {
    chars: Vec<char>,
    start: usize,
    current: usize,
    line: usize,
    /// The brackets opened and not yet closed, innermost last.
    open: Vec<char>,
    tokens: Vec<Token>,
}

impl Scanner {
    fn scan_token(&mut self) -> Result<(), String> {
        let c = self.advance();
        let kind = match c {
            ' ' | '\r' | '\t' => return Ok(()),
            '\n' => {
                // A newline ends a statement, except inside `( )` or `[ ]` and when the
                // next line starts with `|>` or `.`, which continue the current one.
                let in_group = matches!(self.open.last(), Some('(' | '['));
                if !in_group && !self.next_line_continues() {
                    self.add_token(TokenKind::Newline);
                }
                self.line += 1;
                return Ok(());
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
                return Ok(());
            }
            '/' => TokenKind::Slash,

            ':' if self.matches(':') => TokenKind::ColonColon,
            ':' if self.peek().is_ascii_lowercase() || self.peek() == '_' => {
                self.skip_word();
                TokenKind::Symbol(self.lexeme()[1..].to_string())
            }
            ':' => TokenKind::Colon,

            '"' => self.string()?,
            c if c.is_ascii_digit() => self.number()?,
            c if is_word_start(c) => self.identifier(),

            other => return Err(format!("unexpected character `{other}`")),
        };

        match c {
            '(' | '[' | '{' => self.open.push(c),
            ')' | ']' | '}' => {
                self.open.pop();
            }
            _ => {}
        }
        self.add_token(kind);
        Ok(())
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

        let text = self.lexeme();
        Ok(TokenKind::Str(text[1..text.len() - 1].to_string()))
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

        // A duration, like `0.5s`, is a number of seconds.
        if is_word_start(self.peek()) {
            self.skip_word();
            let lexeme = self.lexeme();
            let suffix = &lexeme[text.len()..];
            if suffix != "s" {
                return Err(format!("unknown numeric suffix `{suffix}` in `{lexeme}`"));
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
        self.skip_word();
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

    fn skip_word(&mut self) {
        while self.peek().is_ascii_alphanumeric() || self.peek() == '_' {
            self.advance();
        }
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.chars.len()
    }

    fn advance(&mut self) -> char {
        self.current += 1;
        self.chars[self.current - 1]
    }

    fn matches(&mut self, expected: char) -> bool {
        let found = !self.is_at_end() && self.peek() == expected;
        if found {
            self.current += 1;
        }
        found
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
        let line = self.line;
        self.tokens.push(Token { kind, lexeme, line });
    }
}

fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scans(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            let tokens = scan_tokens(source).expect("source should scan cleanly");
            let kinds: Vec<String> = tokens.iter().map(|t| format!("{:?}", t.kind)).collect();
            assert_eq!(kinds.join(" "), format!("{expected} Eof"), "{source:?}");
        }
    }

    #[test]
    fn scans_operators_numbers_and_names() {
        scans(&[
            (
                "1 + 2 * 3 % 4 / 2.0 - 0.5s",
                "Int(1) Plus Int(2) Star Int(3) Percent Int(4) Slash Float(2.0) Minus Float(0.5)",
            ),
            (
                "== != <= >= -> |> | = < > !",
                "EqEq BangEq LessEq GreaterEq Arrow PipeGt Bar Eq Less Greater Bang",
            ),
            (
                "over overflow _unused",
                r#"Over Ident("overflow") Ident("_unused")"#,
            ),
            // A colon before a lowercase name starts a symbol.
            (
                "f :: :jump pos:Vec2",
                r#"Ident("f") ColonColon Symbol("jump") Ident("pos") Colon Ident("Vec2")"#,
            ),
        ]);
    }

    #[test]
    fn scans_strings_and_comments() {
        scans(&[
            (
                r#""let // not a comment\n""#,
                r#"Str("let // not a comment\\n")"#,
            ),
            ("1 // 2\n3", "Int(1) Newline Int(3)"),
        ]);
    }

    #[test]
    fn a_newline_ends_a_statement_unless_it_is_grouped_or_continued() {
        scans(&[
            (
                "f(\n[1,\n2]\n)",
                r#"Ident("f") LParen LBracket Int(1) Comma Int(2) RBracket RParen"#,
            ),
            (
                "f({\n1\n})",
                r#"Ident("f") LParen LBrace Newline Int(1) Newline RBrace RParen"#,
            ),
            (
                "p\n  |> f\n  .g",
                r#"Ident("p") PipeGt Ident("f") Dot Ident("g")"#,
            ),
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

    #[test]
    fn reports_every_error_with_its_line() {
        let errors = scan_tokens("@\n\"ab\n2x 99999999999999999999").unwrap_err();
        let shown: Vec<String> = errors
            .iter()
            .map(|e| format!("{}: {}", e.line, e.message))
            .collect();
        assert_eq!(
            shown,
            [
                "1: unexpected character `@`",
                "2: unterminated string: `\"ab`",
                "3: unknown numeric suffix `x` in `2x`",
                "3: integer literal `99999999999999999999` does not fit in 64 bits",
            ]
        );
    }
}
