use crate::token::{Token, TokenKind};

const DURATION_SUFFIX: &str = "s";

#[derive(Debug, Clone, PartialEq)]
pub struct ScanError {
    pub message: String,
}

pub fn scan_tokens(source: &str) -> Result<Vec<Token>, Vec<ScanError>> {
    Scanner::new(source).run()
}

struct Scanner {
    chars: Vec<char>,
    start: usize,
    current: usize,
    depth: usize,
    tokens: Vec<Token>,
    errors: Vec<ScanError>,
}

impl Scanner {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            start: 0,
            current: 0,
            depth: 0,
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
        match c {
            ' ' | '\r' | '\t' => {}
            '\n' => self.newline(),

            '(' => self.open(TokenKind::LParen),
            ')' => self.close(TokenKind::RParen),
            '[' => self.open(TokenKind::LBracket),
            ']' => self.close(TokenKind::RBracket),
            '{' => self.add_token(TokenKind::LBrace),
            '}' => self.add_token(TokenKind::RBrace),
            ',' => self.add_token(TokenKind::Comma),
            '.' => self.add_token(TokenKind::Dot),

            '+' => self.add_token(TokenKind::Plus),
            '*' => self.add_token(TokenKind::Star),
            '%' => self.add_token(TokenKind::Percent),

            '-' => {
                let kind = if self.matches('>') {
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                };
                self.add_token(kind);
            }
            '=' => {
                let kind = if self.matches('=') {
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                };
                self.add_token(kind);
            }
            '<' => {
                let kind = if self.matches('=') {
                    TokenKind::LessEq
                } else {
                    TokenKind::Less
                };
                self.add_token(kind);
            }
            '>' => {
                let kind = if self.matches('=') {
                    TokenKind::GreaterEq
                } else {
                    TokenKind::Greater
                };
                self.add_token(kind);
            }
            '|' => {
                let kind = if self.matches('>') {
                    TokenKind::PipeGt
                } else {
                    TokenKind::Bar
                };
                self.add_token(kind);
            }
            '!' => {
                if self.matches('=') {
                    self.add_token(TokenKind::BangEq);
                } else {
                    self.error("unexpected character `!`, did you mean `not`?".to_string());
                }
            }

            '/' => {
                if self.matches('/') {
                    while !self.is_at_end() && self.peek() != '\n' {
                        self.advance();
                    }
                } else {
                    self.add_token(TokenKind::Slash);
                }
            }

            ':' => {
                if is_symbol_start(self.peek()) {
                    self.symbol();
                } else {
                    self.add_token(TokenKind::Colon);
                }
            }

            '"' => self.string(),

            c if c.is_ascii_digit() => self.number(),
            c if is_ident_start(c) => self.identifier(),

            other => self.error(format!("unexpected character `{other}`")),
        }
    }

    fn newline(&mut self) {
        if self.depth > 0 || self.next_line_continues() {
            return;
        }
        self.add_token(TokenKind::Newline);
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

    fn open(&mut self, kind: TokenKind) {
        self.depth += 1;
        self.add_token(kind);
    }

    fn close(&mut self, kind: TokenKind) {
        self.depth = self.depth.saturating_sub(1);
        self.add_token(kind);
    }

    fn string(&mut self) {
        while !self.is_at_end() && self.peek() != '"' && self.peek() != '\n' {
            self.advance();
        }

        if self.is_at_end() || self.peek() == '\n' {
            self.error(format!("unterminated string: `{}`", self.lexeme()));
            return;
        }

        self.advance();

        let value: String = self.chars[self.start + 1..self.current - 1]
            .iter()
            .collect();
        self.add_token(TokenKind::Str(value));
    }

    fn symbol(&mut self) {
        while is_ident_continue(self.peek()) {
            self.advance();
        }

        let name: String = self.chars[self.start + 1..self.current].iter().collect();
        self.add_token(TokenKind::Symbol(name));
    }

    fn number(&mut self) {
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
                self.error(format!(
                    "unknown numeric suffix `{suffix}` in `{}`",
                    self.lexeme()
                ));
                return;
            }
            is_float = true;
        }

        if is_float {
            match text.parse::<f64>() {
                Ok(value) => self.add_token(TokenKind::Float(value)),
                Err(_) => self.error(format!("invalid float literal `{text}`")),
            }
        } else {
            match text.parse::<i64>() {
                Ok(value) => self.add_token(TokenKind::Int(value)),
                Err(_) => self.error(format!("integer literal `{text}` does not fit in 64 bits")),
            }
        }
    }

    fn identifier(&mut self) {
        while is_ident_continue(self.peek()) {
            self.advance();
        }

        let text = self.lexeme();
        let kind = match text.as_str() {
            "_" => TokenKind::Underscore,

            "let" => TokenKind::Let,
            "fn" => TokenKind::Fn,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "on" => TokenKind::On,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "case" => TokenKind::Case,
            "when" => TokenKind::When,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "true" => TokenKind::True,
            "false" => TokenKind::False,

            "sequence" => TokenKind::Sequence,
            "over" => TokenKind::Over,
            "wait" => TokenKind::Wait,
            "start" => TokenKind::Start,
            "as" => TokenKind::As,

            _ => TokenKind::Ident(text),
        };
        self.add_token(kind);
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
        self.tokens.push(Token::new(kind, lexeme));
    }

    fn error(&mut self, message: String) {
        self.errors.push(ScanError { message });
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

    fn kinds(source: &str) -> Vec<TokenKind> {
        let mut tokens = scan_tokens(source).expect("source should scan cleanly");
        assert_eq!(tokens.pop().map(|t| t.kind), Some(TokenKind::Eof));
        tokens.into_iter().map(|t| t.kind).collect()
    }

    fn errors(source: &str) -> Vec<String> {
        scan_tokens(source)
            .unwrap_err()
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    fn ident(name: &str) -> TokenKind {
        TokenKind::Ident(name.into())
    }

    #[test]
    fn scans_arithmetic() {
        assert_eq!(
            kinds("1 + 2 * 3 % 4"),
            vec![
                TokenKind::Int(1),
                TokenKind::Plus,
                TokenKind::Int(2),
                TokenKind::Star,
                TokenKind::Int(3),
                TokenKind::Percent,
                TokenKind::Int(4),
            ]
        );
    }

    #[test]
    fn scans_two_character_operators() {
        assert_eq!(
            kinds("== != <= >= -> |> | = < >"),
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LessEq,
                TokenKind::GreaterEq,
                TokenKind::Arrow,
                TokenKind::PipeGt,
                TokenKind::Bar,
                TokenKind::Eq,
                TokenKind::Less,
                TokenKind::Greater,
            ]
        );
    }

    #[test]
    fn keywords_are_not_identifiers() {
        assert_eq!(
            kinds("sequence sequencer over overflow on only"),
            vec![
                TokenKind::Sequence,
                ident("sequencer"),
                TokenKind::Over,
                ident("overflow"),
                TokenKind::On,
                ident("only"),
            ]
        );
    }

    #[test]
    fn underscore_alone_is_the_wildcard() {
        assert_eq!(
            kinds("_ _unused"),
            vec![TokenKind::Underscore, ident("_unused")]
        );
    }

    #[test]
    fn distinguishes_int_from_float() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42)]);
        assert_eq!(kinds("3.5"), vec![TokenKind::Float(3.5)]);
    }

    #[test]
    fn a_duration_suffix_makes_a_float() {
        assert_eq!(kinds("0.2s"), vec![TokenKind::Float(0.2)]);
        assert_eq!(kinds("2s"), vec![TokenKind::Float(2.0)]);
    }

    #[test]
    fn a_suffix_must_touch_the_number() {
        assert_eq!(kinds("2 s"), vec![TokenKind::Int(2), ident("s")]);
    }

    #[test]
    fn rejects_an_unknown_suffix() {
        assert_eq!(
            errors("16px"),
            vec!["unknown numeric suffix `px` in `16px`"]
        );
    }

    #[test]
    fn trailing_dot_is_not_part_of_the_number() {
        assert_eq!(
            kinds("1.x"),
            vec![TokenKind::Int(1), TokenKind::Dot, ident("x")]
        );
    }

    #[test]
    fn symbols_are_told_apart_from_annotations() {
        assert_eq!(kinds(":jump"), vec![TokenKind::Symbol("jump".into())]);
        assert_eq!(
            kinds("pos: Vec2"),
            vec![ident("pos"), TokenKind::Colon, ident("Vec2")]
        );
        assert_eq!(
            kinds("pos:Vec2"),
            vec![ident("pos"), TokenKind::Colon, ident("Vec2")]
        );
    }

    #[test]
    fn scans_a_record_update() {
        assert_eq!(
            kinds("Player { p | hp: 1 }"),
            vec![
                ident("Player"),
                TokenKind::LBrace,
                ident("p"),
                TokenKind::Bar,
                ident("hp"),
                TokenKind::Colon,
                TokenKind::Int(1),
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn scans_strings() {
        assert_eq!(kinds(r#""hello""#), vec![TokenKind::Str("hello".into())]);
        assert_eq!(kinds(r#""""#), vec![TokenKind::Str(String::new())]);
    }

    #[test]
    fn string_contents_are_not_tokenized() {
        assert_eq!(
            kinds(r#""let x |> 1 // not a comment""#),
            vec![TokenKind::Str("let x |> 1 // not a comment".into())]
        );
    }

    #[test]
    fn strings_have_no_escape_sequences() {
        assert_eq!(kinds(r#""a\nb""#), vec![TokenKind::Str(r"a\nb".into())]);
    }

    #[test]
    fn a_comment_runs_to_the_end_of_the_line() {
        assert_eq!(kinds("1 // 2 + 3"), vec![TokenKind::Int(1)]);
        assert_eq!(
            kinds("1 // 2\n3"),
            vec![TokenKind::Int(1), TokenKind::Newline, TokenKind::Int(3)]
        );
    }

    #[test]
    fn a_newline_ends_a_statement() {
        assert_eq!(
            kinds("let x = 1\nx"),
            vec![
                TokenKind::Let,
                ident("x"),
                TokenKind::Eq,
                TokenKind::Int(1),
                TokenKind::Newline,
                ident("x"),
            ]
        );
    }

    #[test]
    fn newlines_inside_parens_and_brackets_are_ignored() {
        assert_eq!(
            kinds("f(\n1,\n2\n)"),
            vec![
                ident("f"),
                TokenKind::LParen,
                TokenKind::Int(1),
                TokenKind::Comma,
                TokenKind::Int(2),
                TokenKind::RParen,
            ]
        );
        assert_eq!(
            kinds("[1,\n2]"),
            vec![
                TokenKind::LBracket,
                TokenKind::Int(1),
                TokenKind::Comma,
                TokenKind::Int(2),
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn newlines_inside_braces_are_kept() {
        assert_eq!(
            kinds("{\n1\n}"),
            vec![
                TokenKind::LBrace,
                TokenKind::Newline,
                TokenKind::Int(1),
                TokenKind::Newline,
                TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn a_line_starting_with_a_pipe_continues_the_previous_one() {
        assert_eq!(
            kinds("p\n  |> f\n  |> g"),
            vec![
                ident("p"),
                TokenKind::PipeGt,
                ident("f"),
                TokenKind::PipeGt,
                ident("g"),
            ]
        );
    }

    #[test]
    fn a_line_starting_with_a_dot_continues_the_previous_one() {
        assert_eq!(
            kinds("p\n  .f()"),
            vec![
                ident("p"),
                TokenKind::Dot,
                ident("f"),
                TokenKind::LParen,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn a_bar_alone_does_not_continue_a_line() {
        assert_eq!(
            kinds("a\n| b"),
            vec![ident("a"), TokenKind::Newline, TokenKind::Bar, ident("b")]
        );
    }

    #[test]
    fn tokens_keep_their_lexeme() {
        let tokens = scan_tokens("let dt = 0.2s").unwrap();
        let lexemes: Vec<&str> = tokens.iter().map(|t| t.lexeme.as_str()).collect();
        assert_eq!(lexemes, vec!["let", "dt", "=", "0.2s", ""]);
    }

    #[test]
    fn unterminated_string_is_an_error() {
        assert_eq!(errors(r#""oops"#), vec!["unterminated string: `\"oops`"]);
    }

    #[test]
    fn a_string_cannot_span_lines() {
        assert_eq!(
            errors("\"oops\n1 # 2"),
            vec!["unterminated string: `\"oops`", "unexpected character `#`"]
        );
    }

    #[test]
    fn a_lone_bang_suggests_not() {
        assert_eq!(
            errors("!x"),
            vec!["unexpected character `!`, did you mean `not`?"]
        );
    }

    #[test]
    fn reports_every_bad_character() {
        assert_eq!(
            errors("1 # 2 $ 3"),
            vec!["unexpected character `#`", "unexpected character `$`"]
        );
    }
}
