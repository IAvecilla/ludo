use std::rc::Rc;

use crate::ast::{
    BinaryOp, Expr, FnDecl, Param, SeqDecl, Step, StepKind, Stmt, StmtKind, StructDecl, TypeExpr,
    UnaryOp,
};
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub at_end: bool,
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
    blocks: usize,
    struct_ok: bool,
    state: Option<String>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            current: 0,
            blocks: 0,
            struct_ok: true,
            state: None,
        }
    }

    fn statement_line(&mut self) -> Result<Stmt, ParseError> {
        let stmt = self.statement()?;
        if !self.is_at_end() && !self.matches(&[TokenKind::Newline]) {
            return Err(self.error("expected a new line after the statement"));
        }
        Ok(stmt)
    }

    fn synchronize(&mut self) {
        let mut depth = self.blocks;
        while !self.is_at_end() {
            match self.peek() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth = depth.saturating_sub(1),
                TokenKind::Newline if depth == 0 => break,
                _ => {}
            }
            self.advance();
        }
        self.blocks = 0;
    }

    fn skip_newlines(&mut self) {
        while self.matches(&[TokenKind::Newline]) {}
    }

    fn statement(&mut self) -> Result<Stmt, ParseError> {
        let line = self.tokens[self.current].line;
        let declaration = match self.peek() {
            TokenKind::Fn => Some("functions"),
            TokenKind::Struct => Some("structs"),
            TokenKind::Sequence => Some("sequences"),
            _ => None,
        };
        if let Some(what) = declaration {
            if self.blocks > 0 {
                return Err(self.error(format!("{what} can only be declared at the top level")));
            }
        }

        let kind = match self.peek() {
            TokenKind::Let => {
                self.advance();
                self.let_declaration()?
            }
            TokenKind::Fn => {
                self.advance();
                self.fn_declaration()?
            }
            TokenKind::Struct => {
                self.advance();
                self.struct_declaration()?
            }
            TokenKind::Sequence => {
                self.advance();
                self.sequence_declaration(line)?
            }
            _ => {
                let expr = self.expression()?;
                if self.peek() == &TokenKind::Eq {
                    return Err(self.error(
                        "there is no assignment: bind a new value with `let`, or change a field of the state of a sequence",
                    ));
                }
                StmtKind::Expr(expr)
            }
        };
        Ok(Stmt { kind, line })
    }

    fn fn_declaration(&mut self) -> Result<StmtKind, ParseError> {
        let name = self.expect_ident("a function name after `fn`")?;
        self.expect(TokenKind::LParen, "`(` after the function name")?;
        let params = self.parameters()?;
        self.expect(TokenKind::Arrow, "`->` and a return type")?;
        let ret = self.type_expr()?;
        if self.peek() != &TokenKind::LBrace {
            return Err(self.error("expected `{` to start the function body"));
        }
        let body = self.block()?;
        Ok(StmtKind::Fn(Rc::new(FnDecl {
            name,
            params,
            ret,
            body,
        })))
    }

    fn parameters(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();
        if self.peek() != &TokenKind::RParen {
            loop {
                let name = self.expect_ident("a parameter name")?;
                self.expect(TokenKind::Colon, "`:` and a type after the parameter name")?;
                let ty = self.type_expr()?;
                params.push(Param { name, ty });
                if !self.matches(&[TokenKind::Comma]) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen, "`)` to close the parameter list")?;
        Ok(params)
    }

    fn struct_declaration(&mut self) -> Result<StmtKind, ParseError> {
        if let TokenKind::Ident(name) = self.peek() {
            if !starts_uppercase(name) {
                return Err(self.error("a struct name starts with an uppercase letter"));
            }
        }
        let name = self.expect_ident("a struct name after `struct`")?;
        self.expect(TokenKind::LBrace, "`{` after the struct name")?;

        let mut fields: Vec<Param> = Vec::new();
        loop {
            self.skip_newlines();
            if self.matches(&[TokenKind::RBrace]) {
                break;
            }
            if let TokenKind::Ident(field) = self.peek() {
                if fields.iter().any(|f| &f.name == field) {
                    return Err(self.error("this field is already declared"));
                }
            }
            let field = self.expect_ident("a field name")?;
            self.expect(TokenKind::Colon, "`:` and a type after the field name")?;
            let ty = self.type_expr()?;
            fields.push(Param { name: field, ty });
            self.field_separator()?;
        }
        Ok(StmtKind::Struct(Rc::new(StructDecl { name, fields })))
    }

    fn field_separator(&mut self) -> Result<(), ParseError> {
        match self.peek() {
            TokenKind::Comma | TokenKind::Newline => {
                self.advance();
                Ok(())
            }
            TokenKind::RBrace => Ok(()),
            _ => Err(self.error("expected `,`, a new line or `}` after the field")),
        }
    }

    fn sequence_declaration(&mut self, line: usize) -> Result<StmtKind, ParseError> {
        let name = self.expect_ident("a sequence name after `sequence`")?;
        self.expect(TokenKind::LParen, "`(` after the sequence name")?;
        let params = self.parameters()?;
        let Some(state) = params.first().map(|p| p.name.clone()) else {
            return Err(ParseError {
                message: format!(
                    "`{name}` needs a parameter: the first one is the state the sequence runs over"
                ),
                line,
                at_end: false,
            });
        };
        if self.peek() != &TokenKind::LBrace {
            return Err(self.error("expected `{` to start the sequence body"));
        }
        self.state = Some(state);
        let body = self.steps(false);
        self.state = None;
        let body = body?;
        Ok(StmtKind::Sequence(Rc::new(SeqDecl { name, params, body })))
    }

    fn steps(&mut self, in_over: bool) -> Result<Vec<Step>, ParseError> {
        let closing = if in_over {
            "expected `}` to close the body of `over`"
        } else {
            "expected `}` to close the sequence"
        };
        self.advance();
        self.blocks += 1;
        let mut steps = Vec::new();

        loop {
            self.skip_newlines();
            if self.matches(&[TokenKind::RBrace]) {
                break;
            }
            if self.is_at_end() {
                return Err(self.error(closing));
            }
            steps.push(self.step(in_over)?);
            if self.is_at_end() {
                return Err(self.error(closing));
            }
            if !self.matches(&[TokenKind::Newline]) && self.peek() != &TokenKind::RBrace {
                return Err(self.error("expected a new line or `}` after the statement"));
            }
        }

        self.blocks -= 1;
        Ok(steps)
    }

    fn step(&mut self, in_over: bool) -> Result<Step, ParseError> {
        let line = self.tokens[self.current].line;
        let kind = match self.peek() {
            TokenKind::Wait | TokenKind::Over if in_over => {
                return Err(self.error(
                    "`wait` and `over` cannot go inside `over`: a sequence is a flat list of steps",
                ));
            }
            TokenKind::Wait => {
                self.advance();
                StepKind::Over {
                    duration: self.condition()?,
                    var: None,
                    body: Vec::new(),
                }
            }
            TokenKind::Over => {
                self.advance();
                let duration = self.condition()?;
                let var = if self.matches(&[TokenKind::As]) {
                    if self.matches(&[TokenKind::Underscore]) {
                        None
                    } else {
                        Some(self.expect_ident("a name or `_` after `as`")?)
                    }
                } else {
                    None
                };
                if self.peek() != &TokenKind::LBrace {
                    return Err(self.error("expected `{` to start the body of `over`"));
                }
                StepKind::Over {
                    duration,
                    var,
                    body: self.steps(true)?,
                }
            }
            TokenKind::Ident(name) if self.is_field_assignment() => {
                let target = name.clone();
                if self.state.as_ref() != Some(&target) {
                    let state = self.state.clone().unwrap_or_default();
                    return Err(self.error(format!(
                        "only `{state}`, the state of the sequence, can have its fields changed"
                    )));
                }
                self.advance();
                self.advance();
                let field = self.expect_ident("a field name")?;
                self.advance();
                StepKind::Set(target, field, self.expression()?)
            }
            _ => StepKind::Stmt(self.statement()?.kind),
        };
        Ok(Step { kind, line })
    }

    fn is_field_assignment(&self) -> bool {
        let kind = |offset: usize| self.tokens.get(self.current + offset).map(|t| &t.kind);
        kind(1) == Some(&TokenKind::Dot)
            && matches!(kind(2), Some(TokenKind::Ident(_)))
            && kind(3) == Some(&TokenKind::Eq)
    }

    fn condition(&mut self) -> Result<Expr, ParseError> {
        let outer = std::mem::replace(&mut self.struct_ok, false);
        let expr = self.expression();
        self.struct_ok = outer;
        expr
    }

    fn type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let name = self.expect_ident("a type")?;
        let mut args = Vec::new();
        if self.matches(&[TokenKind::LBracket]) {
            loop {
                args.push(self.type_expr()?);
                if !self.matches(&[TokenKind::Comma]) {
                    break;
                }
            }
            self.expect(TokenKind::RBracket, "`]` to close the type arguments")?;
        }
        Ok(TypeExpr { name, args })
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

    fn block(&mut self) -> Result<Expr, ParseError> {
        let outer = std::mem::replace(&mut self.struct_ok, true);
        let block = self.block_body();
        self.struct_ok = outer;
        block
    }

    fn block_body(&mut self) -> Result<Expr, ParseError> {
        let open_line = self.advance().line;
        self.blocks += 1;
        let mut stmts = Vec::new();

        loop {
            self.skip_newlines();
            if self.matches(&[TokenKind::RBrace]) {
                break;
            }
            if self.is_at_end() {
                return Err(self.error("expected `}` to close the block"));
            }
            stmts.push(self.statement()?);
            if self.is_at_end() {
                return Err(self.error("expected `}` to close the block"));
            }
            if !self.matches(&[TokenKind::Newline]) && self.peek() != &TokenKind::RBrace {
                return Err(self.error("expected a new line or `}` after the statement"));
            }
        }

        self.blocks -= 1;
        match stmts.pop() {
            Some(Stmt {
                kind: StmtKind::Expr(tail),
                ..
            }) => Ok(Expr::Block(stmts, Box::new(tail))),
            Some(stmt) => Err(ParseError {
                message: "a block must end with an expression".to_string(),
                line: stmt.line,
                at_end: false,
            }),
            None => Err(ParseError {
                message: "an empty block has no value".to_string(),
                line: open_line,
                at_end: false,
            }),
        }
    }

    fn if_expression(&mut self) -> Result<Expr, ParseError> {
        self.advance();
        let cond = self.condition()?;
        let then = self.branch("`{` after the condition of `if`")?;

        if matches!(self.next_significant(), TokenKind::Else | TokenKind::Eof) {
            self.skip_newlines();
        }
        if !self.matches(&[TokenKind::Else]) {
            return Err(self.error("expected `else`: an `if` without one has no value"));
        }

        let otherwise = if self.peek() == &TokenKind::If {
            self.if_expression()?
        } else {
            self.branch("`{` or `if` after `else`")?
        };
        Ok(Expr::If(
            Box::new(cond),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    fn branch(&mut self, what: &str) -> Result<Expr, ParseError> {
        if self.peek() != &TokenKind::LBrace {
            return Err(self.error(format!("expected {what}")));
        }
        self.block()
    }

    fn next_significant(&self) -> &TokenKind {
        let mut i = self.current;
        while self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        &self.tokens[i].kind
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
        let expr =
            match self.peek().clone() {
                TokenKind::Int(v) => Expr::Int(v),
                TokenKind::Float(v) => Expr::Float(v),
                TokenKind::Str(v) => Expr::Str(v),
                TokenKind::Symbol(v) => Expr::Symbol(v),
                TokenKind::True => Expr::Bool(true),
                TokenKind::False => Expr::Bool(false),
                TokenKind::Ident(v)
                    if self.struct_ok && starts_uppercase(&v) && self.next_is_brace() =>
                {
                    return self.struct_literal(v);
                }
                TokenKind::Ident(v) => Expr::Ident(v),
                TokenKind::LParen => {
                    self.advance();
                    let outer = std::mem::replace(&mut self.struct_ok, true);
                    let inner = self.expression();
                    self.struct_ok = outer;
                    let inner = inner?;
                    self.expect(TokenKind::RParen, "`)` to close the group")?;
                    return Ok(inner);
                }
                TokenKind::LBrace => return self.block(),
                TokenKind::If => return self.if_expression(),
                TokenKind::Return => return Err(self.error(
                    "ludo has no `return`: a function's value is the last expression of its body",
                )),
                TokenKind::Wait | TokenKind::Over => {
                    return Err(
                        self.error("`wait` and `over` only go directly in the body of a sequence")
                    )
                }
                _ => return Err(self.error("expected an expression")),
            };
        self.advance();
        Ok(expr)
    }

    fn next_is_brace(&self) -> bool {
        self.tokens.get(self.current + 1).map(|t| &t.kind) == Some(&TokenKind::LBrace)
    }

    fn struct_literal(&mut self, name: String) -> Result<Expr, ParseError> {
        self.advance();
        self.advance();
        let outer = std::mem::replace(&mut self.struct_ok, true);
        let literal = self.struct_fields(name);
        self.struct_ok = outer;
        literal
    }

    fn struct_fields(&mut self, name: String) -> Result<Expr, ParseError> {
        self.skip_newlines();
        let starts_with_field = matches!(self.peek(), TokenKind::Ident(_))
            && self.tokens.get(self.current + 1).map(|t| &t.kind) == Some(&TokenKind::Colon);
        let base = if starts_with_field || self.peek() == &TokenKind::RBrace {
            None
        } else {
            let base = self.expression()?;
            self.skip_newlines();
            self.expect(TokenKind::Bar, "`|` after the struct being updated")?;
            Some(Box::new(base))
        };

        let mut fields: Vec<(String, Expr)> = Vec::new();
        loop {
            self.skip_newlines();
            if self.matches(&[TokenKind::RBrace]) {
                break;
            }
            if let TokenKind::Ident(field) = self.peek() {
                if fields.iter().any(|(f, _)| f == field) {
                    return Err(self.error("this field is already given"));
                }
            }
            let field = self.expect_ident("a field name")?;
            self.expect(TokenKind::Colon, "`:` after the field name")?;
            fields.push((field, self.expression()?));
            self.field_separator()?;
        }
        Ok(Expr::Struct { name, base, fields })
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
            at_end: token.kind == TokenKind::Eof,
        }
    }
}

fn starts_uppercase(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
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
            at_end: false,
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
        assert_eq!(error("let"), "expected an expression, found `let`");
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

    #[test]
    fn parses_a_block_on_one_line() {
        assert_eq!(stmt("{ 1 + 2 }"), "(block (+ 1 2))");
    }

    #[test]
    fn parses_a_block_over_several_lines() {
        assert_eq!(
            stmt("let area = {\n  let w = 4\n  let h = 5\n  w * h\n}"),
            "(let area (block (let w 4) (let h 5) (* w h)))"
        );
    }

    #[test]
    fn blocks_nest() {
        assert_eq!(
            stmt("{\n  let a = { 1 }\n  { a + 1 }\n}"),
            "(block (let a (block 1)) (block (+ a 1)))"
        );
    }

    #[test]
    fn a_block_is_an_expression() {
        assert_eq!(stmt("{ 1 } + { 2 }"), "(+ (block 1) (block 2))");
        assert_eq!(stmt("f({\n  1\n})"), "(call f (block 1))");
    }

    #[test]
    fn a_block_must_end_with_an_expression() {
        assert_eq!(
            stmt_error("{\n  let a = 1\n}"),
            "a block must end with an expression"
        );
        assert_eq!(stmt_error("{ }"), "an empty block has no value");
    }

    #[test]
    fn reports_an_unclosed_block() {
        assert_eq!(
            stmt_error("{\n  1"),
            "expected `}` to close the block, at end of input"
        );
    }

    #[test]
    fn an_error_inside_a_block_skips_the_rest_of_the_block() {
        assert_eq!(
            program_errors("let a = {\n  let 1 = 2\n  3\n}\nlet 2 = b"),
            vec![
                "expected a name after `let`, found `1`",
                "expected a name after `let`, found `2`",
            ]
        );
    }

    #[test]
    fn a_block_error_reports_the_line_inside_the_block() {
        let tokens = scan_tokens("let a = {\n  let 1 = 2\n  3\n}").unwrap();
        let errors = super::parse(tokens).unwrap_err();
        assert_eq!(errors[0].line, 2);
    }

    #[test]
    fn parses_a_function() {
        assert_eq!(
            stmt("fn add(a: Int, b: Int) -> Int {\n  a + b\n}"),
            "(fn add (a: Int, b: Int) -> Int (block (+ a b)))"
        );
    }

    #[test]
    fn parses_a_function_on_one_line() {
        assert_eq!(
            stmt("fn double(x: Int) -> Int { x * 2 }"),
            "(fn double (x: Int) -> Int (block (* x 2)))"
        );
    }

    #[test]
    fn parses_a_function_without_parameters() {
        assert_eq!(
            stmt("fn one() -> Int { 1 }"),
            "(fn one () -> Int (block 1))"
        );
    }

    #[test]
    fn parses_generic_type_arguments() {
        assert_eq!(
            stmt("fn f(xs: List[Int], m: Map[String, List[Int]]) -> Int { 1 }"),
            "(fn f (xs: List[Int], m: Map[String, List[Int]]) -> Int (block 1))"
        );
    }

    #[test]
    fn a_parameter_needs_a_type() {
        assert_eq!(
            stmt_error("fn f(x) -> Int { x }"),
            "expected `:` and a type after the parameter name, found `)`"
        );
    }

    #[test]
    fn a_function_needs_a_return_type() {
        assert_eq!(
            stmt_error("fn f(x: Int) { x }"),
            "expected `->` and a return type, found `{`"
        );
    }

    #[test]
    fn a_function_body_is_a_block() {
        assert_eq!(
            stmt_error("fn f(x: Int) -> Int x"),
            "expected `{` to start the function body, found `x`"
        );
    }

    #[test]
    fn functions_are_only_declared_at_the_top_level() {
        assert_eq!(
            stmt_error("{\n  fn f() -> Int { 1 }\n  1\n}"),
            "functions can only be declared at the top level, found `fn`"
        );
    }

    #[test]
    fn parses_an_if() {
        assert_eq!(stmt("if a { 1 } else { 2 }"), "(if a (block 1) (block 2))");
    }

    #[test]
    fn an_if_is_an_expression() {
        assert_eq!(
            stmt("let s = if fast { 200.0 } else { 120.0 }"),
            "(let s (if fast (block 200.0) (block 120.0)))"
        );
    }

    #[test]
    fn else_if_chains() {
        assert_eq!(
            stmt("if a { 1 } else if b { 2 } else { 3 }"),
            "(if a (block 1) (if b (block 2) (block 3)))"
        );
    }

    #[test]
    fn an_if_can_span_lines() {
        assert_eq!(
            stmt("if a {\n  1\n} else {\n  2\n}"),
            "(if a (block 1) (block 2))"
        );
    }

    #[test]
    fn else_may_start_the_next_line() {
        assert_eq!(
            stmt("if a {\n  1\n}\nelse {\n  2\n}"),
            "(if a (block 1) (block 2))"
        );
    }

    #[test]
    fn an_if_needs_an_else() {
        assert_eq!(
            stmt_error("if a { 1 }"),
            "expected `else`: an `if` without one has no value, at end of input"
        );
        assert_eq!(
            stmt_error("if a { 1 }\nlet x = 1"),
            "expected `else`: an `if` without one has no value, at end of line"
        );
    }

    #[test]
    fn if_branches_are_blocks() {
        assert_eq!(
            stmt_error("if a 1 else 2"),
            "expected `{` after the condition of `if`, found `1`"
        );
        assert_eq!(
            stmt_error("if a { 1 } else 2"),
            "expected `{` or `if` after `else`, found `2`"
        );
    }

    #[test]
    fn return_is_reserved_but_not_part_of_the_language() {
        assert_eq!(
            program_errors("fn f(x: Int) -> Int {\n  return x\n}"),
            vec!["ludo has no `return`: a function's value is the last expression of its body, found `return`"]
        );
    }

    #[test]
    fn errors_know_whether_the_input_ended_too_soon() {
        let at_end = |source: &str| {
            super::parse(scan_tokens(source).unwrap())
                .unwrap_err()
                .last()
                .unwrap()
                .at_end
        };
        assert!(at_end("fn f() -> Int {\n"));
        assert!(!at_end("let x =\n"));
        assert!(at_end("if a { 1 }\n"));
        assert!(at_end("(1 +\n"));
        assert!(!at_end("1 + )\n"));
        assert!(!at_end("{ let x = 1 }\n"));
        assert!(!at_end("fn f() -> Int { 1 }\nfn g( -> Int { 2 }\n"));
    }

    #[test]
    fn parses_a_struct_declaration() {
        assert_eq!(
            stmt("struct Ball { x: Float, y: Float }"),
            "(struct Ball (x: Float, y: Float))"
        );
        assert_eq!(
            stmt("struct Ball {\n  x: Float\n  y: Float,\n}"),
            "(struct Ball (x: Float, y: Float))"
        );
        assert_eq!(stmt("struct Empty {}"), "(struct Empty ())");
    }

    #[test]
    fn struct_declaration_errors() {
        assert_eq!(
            stmt_error("struct ball { x: Float }"),
            "a struct name starts with an uppercase letter, found `ball`"
        );
        assert_eq!(
            stmt_error("struct Ball { x: Float, x: Int }"),
            "this field is already declared, found `x`"
        );
        assert_eq!(
            stmt_error("struct Ball { x }"),
            "expected `:` and a type after the field name, found `}`"
        );
        assert_eq!(
            stmt_error("fn f() -> Int {\n  struct A {}\n  1\n}"),
            "structs can only be declared at the top level, found `struct`"
        );
    }

    #[test]
    fn parses_a_struct_literal() {
        assert_eq!(parse("Ball { x: 1.0, y: 2.0 }"), "(Ball x: 1.0 y: 2.0)");
        assert_eq!(parse("Empty {}"), "(Empty)");
        assert_eq!(
            stmt("let b = Ball {\n  x: 1.0,\n  y: 2.0\n}"),
            "(let b (Ball x: 1.0 y: 2.0))"
        );
    }

    #[test]
    fn parses_a_struct_update() {
        assert_eq!(parse("Ball { b | x: 3.0 }"), "(Ball b | x: 3.0)");
        assert_eq!(
            parse("Ball { move(b) | x: b.x + 1.0 }"),
            "(Ball (call move b) | x: (+ (. b x) 1.0))"
        );
    }

    #[test]
    fn struct_literal_errors() {
        assert_eq!(
            error("Ball { x: 1.0, x: 2.0 }"),
            "this field is already given, found `x`"
        );
        assert_eq!(
            error("Ball { b x: 1.0 }"),
            "expected `|` after the struct being updated, found `x`"
        );
    }

    #[test]
    fn a_lowercase_name_before_a_brace_is_not_a_struct() {
        assert_eq!(
            stmt("if ready { 1 } else { 2 }"),
            "(if ready (block 1) (block 2))"
        );
    }

    #[test]
    fn the_condition_of_an_if_is_not_a_struct_literal() {
        assert_eq!(
            stmt("if x > MAX { 1 } else { 2 }"),
            "(if (> x MAX) (block 1) (block 2))"
        );
        assert_eq!(
            stmt("if (Ball { x: 1.0 }) == b { 1 } else { 2 }"),
            "(if (== (Ball x: 1.0) b) (block 1) (block 2))"
        );
        assert_eq!(
            stmt("if ok { Ball { x: 1.0 } } else { b }"),
            "(if ok (block (Ball x: 1.0)) (block b))"
        );
    }

    #[test]
    fn parses_a_sequence() {
        assert_eq!(
            stmt("sequence serve(b: Ball, speed: Float) {\n  wait 1.0s\n  b.vx = speed\n}"),
            "(sequence serve (b: Ball, speed: Float) (over 1.0 _) (set b.vx speed))"
        );
    }

    #[test]
    fn parses_over() {
        assert_eq!(
            stmt("sequence slide(b: Ball) {\n  let from = b.x\n  over 0.5s as t {\n    b.x = from + t\n  }\n}"),
            "(sequence slide (b: Ball) (let from (. b x)) (over 0.5 t (set b.x (+ from t))))"
        );
        assert_eq!(
            stmt("sequence s(b: Ball) {\n  over 0.5s as _ {}\n  over d {}\n}"),
            "(sequence s (b: Ball) (over 0.5 _) (over d _))"
        );
    }

    #[test]
    fn a_sequence_runs_over_its_first_parameter() {
        assert_eq!(
            stmt_error("sequence serve() {}"),
            "`serve` needs a parameter: the first one is the state the sequence runs over"
        );
        assert_eq!(
            stmt_error("sequence serve(b: Ball, other: Ball) {\n  other.x = 1.0\n}"),
            "only `b`, the state of the sequence, can have its fields changed, found `other`"
        );
    }

    #[test]
    fn a_sequence_is_flat() {
        assert_eq!(
            stmt_error("sequence s(b: Ball) {\n  over 1.0s {\n    wait 1.0s\n  }\n}"),
            "`wait` and `over` cannot go inside `over`: a sequence is a flat list of steps, found `wait`"
        );
        assert_eq!(
            stmt_error("sequence s(b: Ball) {\n  if ok { wait 1.0s } else { 1 }\n}"),
            "`wait` and `over` only go directly in the body of a sequence, found `wait`"
        );
        assert_eq!(
            stmt_error("fn f() -> Int {\n  wait 1.0s\n  1\n}"),
            "`wait` and `over` only go directly in the body of a sequence, found `wait`"
        );
    }

    #[test]
    fn there_is_no_assignment() {
        assert_eq!(
            stmt_error("x = 1"),
            "there is no assignment: bind a new value with `let`, or change a field of the state of a sequence, found `=`"
        );
        assert_eq!(
            stmt_error("b.x = 1"),
            "there is no assignment: bind a new value with `let`, or change a field of the state of a sequence, found `=`"
        );
    }

    #[test]
    fn a_non_expression_token_is_reported_by_its_text() {
        assert_eq!(error("1 + )"), "expected an expression, found `)`");
    }
}
