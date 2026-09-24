use crate::ast::{BinaryOp, Expr, Stmt, StmtKind, UnaryOp};
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>, Vec<ParseError>> {
    let mut parser = Parser::new(tokens);
    let mut stmts = Vec::new();
    let mut errors = Vec::new();

    loop {
        parser.skip_newlines();
        if parser.is_at_end() {
            break;
        }
        match parser.statement_line() {
            Ok(stmt) => stmts.push(stmt),
            Err(error) => {
                errors.push(error);
                parser.synchronize();
            }
        }
    }

    if errors.is_empty() {
        Ok(stmts)
    } else {
        Err(errors)
    }
}

pub fn parse_expr(tokens: Vec<Token>) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(tokens);
    let expr = parser.expression()?;
    parser.expect_end()?;
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    fn statement_line(&mut self) -> Result<Stmt, ParseError> {
        let stmt = self.statement()?;
        if !self.is_at_end() && !self.matches(&[TokenKind::Newline]) {
            return Err(self.error("expected a new line after the statement"));
        }
        Ok(stmt)
    }

    fn synchronize(&mut self) {
        while !self.is_at_end() && self.peek() != &TokenKind::Newline {
            self.advance();
        }
    }

    fn skip_newlines(&mut self) {
        while self.matches(&[TokenKind::Newline]) {}
    }

    fn statement(&mut self) -> Result<Stmt, ParseError> {
        let line = self.tokens[self.current].line;
        let kind = if self.matches(&[TokenKind::Let]) {
            self.let_declaration()?
        } else {
            StmtKind::Expr(self.expression()?)
        };
        Ok(Stmt { kind, line })
    }

    fn let_declaration(&mut self) -> Result<StmtKind, ParseError> {
        let name = self.expect_ident("a name after `let`")?;
        self.expect(TokenKind::Eq, "`=` after the name")?;
        let value = self.expression()?;
        Ok(StmtKind::Let(name, value))
    }

    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and()?;
        while self.matches(&[TokenKind::Or]) {
            let right = self.and()?;
            left = Expr::Binary(Box::new(left), BinaryOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.equality()?;
        while self.matches(&[TokenKind::And]) {
            let right = self.equality()?;
            left = Expr::Binary(Box::new(left), BinaryOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.comparison()?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinaryOp::Eq,
                TokenKind::BangEq => BinaryOp::Ne,
                _ => break,
            };
            self.advance();
            let right = self.comparison()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.pipe()?;
        loop {
            let op = match self.peek() {
                TokenKind::Less => BinaryOp::Lt,
                TokenKind::LessEq => BinaryOp::Le,
                TokenKind::Greater => BinaryOp::Gt,
                TokenKind::GreaterEq => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.pipe()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn pipe(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.term()?;
        while self.matches(&[TokenKind::PipeGt]) {
            let line = self.tokens[self.current - 1].line;
            let right = self.term()?;
            left = pipe_into(left, right, line)?;
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.factor()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.factor()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.unary()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Not => UnaryOp::Not,
            _ => return self.postfix(),
        };
        self.advance();
        let right = self.unary()?;
        Ok(Expr::Unary(op, Box::new(right)))
    }

    fn postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            if self.matches(&[TokenKind::LParen]) {
                let args = self.arguments()?;
                expr = Expr::Call(Box::new(expr), args);
            } else if self.matches(&[TokenKind::Dot]) {
                let name = self.expect_ident("a field or method name after `.`")?;
                if self.matches(&[TokenKind::LParen]) {
                    let mut args = vec![expr];
                    args.extend(self.arguments()?);
                    expr = Expr::Call(Box::new(Expr::Ident(name)), args);
                } else {
                    expr = Expr::Field(Box::new(expr), name);
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.peek() != &TokenKind::RParen {
            loop {
                args.push(self.expression()?);
                if !self.matches(&[TokenKind::Comma]) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "`)` to close the argument list")?;
        Ok(args)
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let expr = match self.peek().clone() {
            TokenKind::Int(v) => Expr::Int(v),
            TokenKind::Float(v) => Expr::Float(v),
            TokenKind::Str(v) => Expr::Str(v),
            TokenKind::Symbol(v) => Expr::Symbol(v),
            TokenKind::True => Expr::Bool(true),
            TokenKind::False => Expr::Bool(false),
            TokenKind::Ident(v) => Expr::Ident(v),
            TokenKind::LParen => {
                self.advance();
                let inner = self.expression()?;
                self.expect(TokenKind::RParen, "`)` to close the group")?;
                return Ok(inner);
            }
            TokenKind::Eof | TokenKind::Newline => return Err(self.error("expected an expression")),
            other => return Err(self.error(format!("`{other:?}` is not an expression"))),
        };
        self.advance();
        Ok(expr)
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.current].kind
    }

    fn is_at_end(&self) -> bool {
        self.peek() == &TokenKind::Eof
    }

    fn advance(&mut self) -> &Token {
        if self.tokens[self.current].kind != TokenKind::Eof {
            self.current += 1;
        }
        &self.tokens[self.current - 1]
    }

    fn matches(&mut self, kinds: &[TokenKind]) -> bool {
        if kinds.contains(self.peek()) {
            self.advance();
            return true;
        }
        false
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<(), ParseError> {
        if self.peek() == &kind {
            self.advance();
            return Ok(());
        }
        Err(self.error(format!("expected {what}")))
    }

    fn expect_ident(&mut self, what: &str) -> Result<String, ParseError> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            self.advance();
            return Ok(name);
        }
        Err(self.error(format!("expected {what}")))
    }

    fn expect_end(&mut self) -> Result<(), ParseError> {
        if self.peek() == &TokenKind::Eof {
            return Ok(());
        }
        Err(self.error("unexpected trailing input"))
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        let token = &self.tokens[self.current];
        let message = message.into();
        let message = match token.kind {
            TokenKind::Eof => format!("{message}, at end of input"),
            TokenKind::Newline => format!("{message}, at end of line"),
            _ => format!("{message}, found `{}`", token.lexeme),
        };
        ParseError {
            message,
            line: token.line,
        }
    }
}

fn pipe_into(left: Expr, right: Expr, line: usize) -> Result<Expr, ParseError> {
    match right {
        Expr::Call(callee, args) => {
            let mut piped = vec![left];
            piped.extend(args);
            Ok(Expr::Call(callee, piped))
        }
        callee @ (Expr::Ident(_) | Expr::Field(_, _)) => {
            Ok(Expr::Call(Box::new(callee), vec![left]))
        }
        _ => Err(ParseError {
            message: "the right side of `|>` must be a function or a call".to_string(),
            line,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::scan_tokens;

    fn parse(source: &str) -> String {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        parse_expr(tokens)
            .unwrap_or_else(|e| panic!("source should parse cleanly: {}", e.message))
            .to_string()
    }

    fn error(source: &str) -> String {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        parse_expr(tokens).unwrap_err().message
    }

    #[test]
    fn parses_literals() {
        assert_eq!(parse("1"), "1");
        assert_eq!(parse("1.5"), "1.5");
        assert_eq!(parse("0.2s"), "0.2");
        assert_eq!(parse(r#""hi""#), r#""hi""#);
        assert_eq!(parse(":jump"), ":jump");
        assert_eq!(parse("true"), "true");
        assert_eq!(parse("x"), "x");
    }

    #[test]
    fn factor_binds_tighter_than_term() {
        assert_eq!(parse("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(parse("1 * 2 + 3"), "(+ (* 1 2) 3)");
    }

    #[test]
    fn binary_operators_are_left_associative() {
        assert_eq!(parse("1 - 2 - 3"), "(- (- 1 2) 3)");
        assert_eq!(parse("1 / 2 / 3"), "(/ (/ 1 2) 3)");
    }

    #[test]
    fn parentheses_leave_no_node_behind() {
        assert_eq!(parse("(1 + 2) * 3"), "(* (+ 1 2) 3)");
        assert_eq!(parse("(((1)))"), "1");
    }

    #[test]
    fn unary_binds_tighter_than_factor() {
        assert_eq!(parse("-1 * 2"), "(* (- 1) 2)");
        assert_eq!(parse("not a and b"), "(and (not a) b)");
        assert_eq!(parse("--1"), "(- (- 1))");
    }

    #[test]
    fn full_precedence_ladder() {
        assert_eq!(
            parse("a or b and c == d < e + f * g"),
            "(or a (and b (== c (< d (+ e (* f g))))))"
        );
    }

    #[test]
    fn parses_calls() {
        assert_eq!(parse("f()"), "(call f)");
        assert_eq!(parse("f(1, 2)"), "(call f 1 2)");
        assert_eq!(parse("f(1)(2)"), "(call (call f 1) 2)");
        assert_eq!(parse("f(g(1))"), "(call f (call g 1))");
    }

    #[test]
    fn a_dot_without_parens_is_field_access() {
        assert_eq!(parse("p.pos"), "(. p pos)");
        assert_eq!(parse("p.pos.x"), "(. (. p pos) x)");
    }

    #[test]
    fn a_dot_with_parens_puts_the_receiver_first() {
        assert_eq!(parse("p.physics(dt)"), "(call physics p dt)");
        assert_eq!(parse("p.pos.length()"), "(call length (. p pos))");
    }

    #[test]
    fn call_dot_and_pipe_build_the_same_tree() {
        let expected = "(call physics p dt)";
        assert_eq!(parse("physics(p, dt)"), expected);
        assert_eq!(parse("p.physics(dt)"), expected);
        assert_eq!(parse("p |> physics(dt)"), expected);
    }

    #[test]
    fn a_pipe_into_a_bare_name_calls_it_with_one_argument() {
        assert_eq!(parse("p |> normalize"), "(call normalize p)");
    }

    #[test]
    fn pipes_chain_left_to_right() {
        assert_eq!(
            parse("p |> handle(input) |> physics(dt)"),
            "(call physics (call handle p input) dt)"
        );
    }

    #[test]
    fn pipe_binds_looser_than_term() {
        assert_eq!(parse("a + b |> f"), "(call f (+ a b))");
    }

    #[test]
    fn pipe_binds_tighter_than_and() {
        assert_eq!(parse("x |> valid() and y"), "(and (call valid x) y)");
    }

    #[test]
    fn pipe_binds_tighter_than_comparison() {
        assert_eq!(parse("a |> f < b"), "(< (call f a) b)");
    }

    #[test]
    fn rejects_a_pipe_into_a_non_callable() {
        assert_eq!(
            error("a |> 1"),
            "the right side of `|>` must be a function or a call"
        );
    }

    #[test]
    fn reports_an_unclosed_paren() {
        assert_eq!(
            error("(1 + 2"),
            "expected `)` to close the group, at end of input"
        );
    }

    #[test]
    fn reports_a_missing_operand() {
        assert_eq!(error("1 +"), "expected an expression, at end of input");
    }

    #[test]
    fn reports_a_token_that_cannot_start_an_expression() {
        assert_eq!(error("let"), "`Let` is not an expression, found `let`");
    }

    #[test]
    fn reports_trailing_input() {
        assert_eq!(error("1 2"), "unexpected trailing input, found `2`");
    }

    fn program(source: &str) -> Vec<String> {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        super::parse(tokens)
            .unwrap_or_else(|e| panic!("source should parse cleanly: {}", e[0].message))
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn stmt(source: &str) -> String {
        let mut stmts = program(source);
        assert_eq!(stmts.len(), 1, "expected a single statement");
        stmts.remove(0)
    }

    fn program_errors(source: &str) -> Vec<String> {
        let tokens = scan_tokens(source).expect("source should scan cleanly");
        super::parse(tokens)
            .unwrap_err()
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    fn stmt_error(source: &str) -> String {
        let mut errors = program_errors(source);
        assert_eq!(errors.len(), 1, "expected a single error");
        errors.remove(0)
    }

    #[test]
    fn parses_a_let() {
        assert_eq!(stmt("let x = 1 + 2"), "(let x (+ 1 2))");
    }

    #[test]
    fn an_expression_is_a_statement() {
        assert_eq!(stmt("x |> f"), "(call f x)");
    }

    #[test]
    fn let_needs_a_name() {
        assert_eq!(
            stmt_error("let 1 = 2"),
            "expected a name after `let`, found `1`"
        );
        assert_eq!(
            stmt_error("let"),
            "expected a name after `let`, at end of input"
        );
    }

    #[test]
    fn let_needs_an_initializer() {
        assert_eq!(
            stmt_error("let x"),
            "expected `=` after the name, at end of input"
        );
        assert_eq!(
            stmt_error("let x 1"),
            "expected `=` after the name, found `1`"
        );
    }

    #[test]
    fn let_rejects_trailing_input() {
        assert_eq!(
            stmt_error("let x = 1 2"),
            "expected a new line after the statement, found `2`"
        );
    }

    #[test]
    fn parses_one_statement_per_line() {
        assert_eq!(
            program("let x = 1\nlet y = x + 1\ny"),
            vec!["(let x 1)", "(let y (+ x 1))", "y"]
        );
    }

    #[test]
    fn blank_lines_are_skipped() {
        assert_eq!(program("\n\nlet x = 1\n\n\nx\n"), vec!["(let x 1)", "x"]);
        assert_eq!(program(""), Vec::<String>::new());
    }

    #[test]
    fn a_call_can_span_lines() {
        assert_eq!(program("f(\n  1,\n  2\n)"), vec!["(call f 1 2)"]);
    }

    #[test]
    fn a_pipe_chain_can_span_lines() {
        assert_eq!(
            program("let p = q\n  |> handle(input)\n  |> physics(dt)"),
            vec!["(let p (call physics (call handle q input) dt))"]
        );
    }

    #[test]
    fn an_expression_cannot_continue_on_the_next_line_without_a_pipe() {
        assert_eq!(
            stmt_error("let x = 1 +\n2"),
            "expected an expression, at end of line"
        );
    }

    #[test]
    fn recovers_and_reports_every_bad_line() {
        assert_eq!(
            program_errors("let 1 = 2\nlet ok = 1\nlet x\n1 +"),
            vec![
                "expected a name after `let`, found `1`",
                "expected `=` after the name, at end of line",
                "expected an expression, at end of input",
            ]
        );
    }

    #[test]
    fn statements_record_their_line() {
        let tokens = scan_tokens("let x = 1\n\nx").unwrap();
        let lines: Vec<usize> = super::parse(tokens)
            .unwrap()
            .iter()
            .map(|s| s.line)
            .collect();
        assert_eq!(lines, vec![1, 3]);
    }

    #[test]
    fn errors_record_their_line() {
        let tokens = scan_tokens("let ok = 1\nlet 1 = 2\n\nlet x").unwrap();
        let lines: Vec<usize> = super::parse(tokens)
            .unwrap_err()
            .iter()
            .map(|e| e.line)
            .collect();
        assert_eq!(lines, vec![2, 4]);
    }
}
