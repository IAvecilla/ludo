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

impl ParseError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        ParseError {
            message: message.into(),
            line,
            at_end: false,
        }
    }
}

pub fn parse(tokens: Vec<Token>) -> Result<Vec<Stmt>, Vec<ParseError>> {
    let mut parser = Parser::new(tokens);
    let mut statements = Vec::new();
    let mut errors = Vec::new();

    loop {
        parser.skip_newlines();
        if parser.is_at_end() {
            break;
        }
        match parser.statement_line() {
            Ok(stmt) => statements.push(stmt),
            Err(error) => {
                errors.push(error);
                parser.synchronize();
            }
        }
    }

    if errors.is_empty() {
        Ok(statements)
    } else {
        Err(errors)
    }
}

pub fn parse_expr(tokens: Vec<Token>) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(tokens);
    let expr = parser.expression()?;
    if !parser.is_at_end() {
        return Err(parser.error("unexpected trailing input"));
    }
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

    fn statement(&mut self) -> Result<Stmt, ParseError> {
        let line = self.line();
        let kind = match self.peek() {
            TokenKind::Let => {
                self.advance();
                let name = self.expect_ident("a name after `let`")?;
                self.expect(TokenKind::Eq, "`=` after the name")?;
                StmtKind::Let(name, self.expression()?)
            }
            TokenKind::Ident(_) if self.kind_at(1) == Some(&TokenKind::ColonColon) => {
                self.declaration(line)?
            }
            TokenKind::Struct => {
                return Err(self.error("structs are declared as `Name :: struct { ... }`"))
            }
            TokenKind::Sequence => {
                return Err(self
                    .error("sequences are declared as `name :: sequence (state: Type) { ... }`"))
            }
            TokenKind::Start => {
                self.advance();
                let name = self.expect_ident("a sequence name after `start`")?;
                self.expect(TokenKind::LParen, "`(` and the arguments of the sequence")?;
                StmtKind::Start(name, self.arguments()?)
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

    fn declaration(&mut self, line: usize) -> Result<StmtKind, ParseError> {
        let kind = self.kind_at(2).cloned();
        let what = match kind {
            Some(TokenKind::Struct) => "structs",
            Some(TokenKind::Sequence) => "sequences",
            _ => "functions",
        };
        if self.blocks > 0 {
            return Err(self.error(format!("{what} can only be declared at the top level")));
        }
        if kind == Some(TokenKind::Struct)
            && matches!(self.peek(), TokenKind::Ident(name) if !starts_uppercase(name))
        {
            return Err(self.error("a struct name starts with an uppercase letter"));
        }

        let name = self.expect_ident("a name")?;
        self.advance();
        match self.peek() {
            TokenKind::Struct => {
                self.advance();
                self.struct_declaration(name)
            }
            TokenKind::Sequence => {
                self.advance();
                self.sequence_declaration(name, line)
            }
            TokenKind::LParen => {
                self.advance();
                self.fn_declaration(name)
            }
            _ => Err(self.error(
                "expected `(`, `struct` or `sequence` after `::`; values are bound with `let`",
            )),
        }
    }

    fn fn_declaration(&mut self, name: String) -> Result<StmtKind, ParseError> {
        let params = self.parameters()?;
        self.expect(TokenKind::Arrow, "`->` and a return type")?;
        let ret = self.type_expr()?;
        let body = self.block_after("`{` to start the function body")?;
        Ok(StmtKind::Fn(Rc::new(FnDecl {
            name,
            params,
            ret,
            body,
        })))
    }

    fn parameters(&mut self) -> Result<Vec<Param>, ParseError> {
        self.comma_list(TokenKind::RParen, "`)` to close the parameter list", |p| {
            let name = p.expect_ident("a parameter name")?;
            p.expect(TokenKind::Colon, "`:` and a type after the parameter name")?;
            Ok(Param {
                name,
                ty: p.type_expr()?,
            })
        })
    }

    fn type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let name = self.expect_ident("a type")?;
        let args = if self.matches(&[TokenKind::LBracket]) {
            self.comma_list(
                TokenKind::RBracket,
                "`]` to close the type arguments",
                Self::type_expr,
            )?
        } else {
            Vec::new()
        };
        Ok(TypeExpr { name, args })
    }

    fn struct_declaration(&mut self, name: String) -> Result<StmtKind, ParseError> {
        self.expect(TokenKind::LBrace, "`{` after `struct`")?;
        let mut seen = Vec::new();
        let fields = self.lines(true, "expected `}` to close the struct", |p| {
            let name = p.field_name(&mut seen, "this field is already declared")?;
            p.expect(TokenKind::Colon, "`:` and a type after the field name")?;
            Ok(Param {
                name,
                ty: p.type_expr()?,
            })
        })?;
        Ok(StmtKind::Struct(Rc::new(StructDecl { name, fields })))
    }

    fn sequence_declaration(&mut self, name: String, line: usize) -> Result<StmtKind, ParseError> {
        self.expect(TokenKind::LParen, "`(` after `sequence`")?;
        let params = self.parameters()?;
        let Some(state) = params.first().map(|p| p.name.clone()) else {
            return Err(ParseError::at(
                line,
                format!(
                    "`{name}` needs a parameter: the first one is the state the sequence runs over"
                ),
            ));
        };
        self.expect(TokenKind::LBrace, "`{` to start the sequence body")?;
        self.state = Some(state);
        let body = self.lines(false, "expected `}` to close the sequence", |p| {
            p.step(false)
        });
        self.state = None;
        Ok(StmtKind::Sequence(Rc::new(SeqDecl {
            name,
            params,
            body: body?,
        })))
    }

    fn step(&mut self, in_over: bool) -> Result<Step, ParseError> {
        let line = self.line();
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
                let var = match self.matches(&[TokenKind::As]) {
                    true => Some(self.expect_ident("a name after `as`")?),
                    false => None,
                };
                self.expect(TokenKind::LBrace, "`{` to start the body of `over`")?;
                let body = self.lines(false, "expected `}` to close the body of `over`", |p| {
                    p.step(true)
                })?;
                StepKind::Over {
                    duration,
                    var,
                    body,
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
                self.current += 2;
                let field = self.expect_ident("a field name")?;
                self.advance();
                StepKind::Set(target, field, self.expression()?)
            }
            _ => StepKind::Stmt(self.statement()?),
        };
        Ok(Step { kind, line })
    }

    fn is_field_assignment(&self) -> bool {
        self.kind_at(1) == Some(&TokenKind::Dot)
            && matches!(self.kind_at(2), Some(TokenKind::Ident(_)))
            && self.kind_at(3) == Some(&TokenKind::Eq)
    }

    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        self.binary(Self::and, &[(TokenKind::Or, BinaryOp::Or)])
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        self.binary(Self::equality, &[(TokenKind::And, BinaryOp::And)])
    }

    fn equality(&mut self) -> Result<Expr, ParseError> {
        self.binary(
            Self::comparison,
            &[
                (TokenKind::EqEq, BinaryOp::Eq),
                (TokenKind::BangEq, BinaryOp::Ne),
            ],
        )
    }

    fn comparison(&mut self) -> Result<Expr, ParseError> {
        self.binary(
            Self::pipe,
            &[
                (TokenKind::Less, BinaryOp::Lt),
                (TokenKind::LessEq, BinaryOp::Le),
                (TokenKind::Greater, BinaryOp::Gt),
                (TokenKind::GreaterEq, BinaryOp::Ge),
            ],
        )
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
        self.binary(
            Self::factor,
            &[
                (TokenKind::Plus, BinaryOp::Add),
                (TokenKind::Minus, BinaryOp::Sub),
            ],
        )
    }

    fn factor(&mut self) -> Result<Expr, ParseError> {
        self.binary(
            Self::unary,
            &[
                (TokenKind::Star, BinaryOp::Mul),
                (TokenKind::Slash, BinaryOp::Div),
                (TokenKind::Percent, BinaryOp::Rem),
            ],
        )
    }

    fn binary(
        &mut self,
        operand: fn(&mut Self) -> Result<Expr, ParseError>,
        ops: &[(TokenKind, BinaryOp)],
    ) -> Result<Expr, ParseError> {
        let mut left = operand(self)?;
        while let Some(&(_, op)) = ops.iter().find(|(kind, _)| kind == self.peek()) {
            self.advance();
            let right = operand(self)?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            _ => return self.postfix(),
        };
        self.advance();
        Ok(Expr::Unary(op, Box::new(self.unary()?)))
    }

    fn postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.primary()?;
        loop {
            if self.matches(&[TokenKind::LParen]) {
                expr = Expr::Call(Box::new(expr), self.arguments()?);
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
                return Ok(expr);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let expr = match self.peek().clone() {
            TokenKind::Int(v) => Expr::Int(v),
            TokenKind::Float(v) => Expr::Float(v),
            TokenKind::Str(v) => Expr::Str(v),
            TokenKind::Symbol(v) => Expr::Symbol(v),
            TokenKind::True => Expr::Bool(true),
            TokenKind::False => Expr::Bool(false),
            TokenKind::Ident(name)
                if self.struct_ok
                    && starts_uppercase(&name)
                    && self.kind_at(1) == Some(&TokenKind::LBrace) =>
            {
                self.current += 2;
                return self.with_structs(true, |p| p.struct_literal(name));
            }
            TokenKind::Ident(v) => Expr::Ident(v),
            TokenKind::LParen => {
                self.advance();
                let inner = self.with_structs(true, Self::expression)?;
                self.expect(TokenKind::RParen, "`)` to close the group")?;
                return Ok(inner);
            }
            TokenKind::LBracket => {
                self.advance();
                return self.with_structs(true, |p| {
                    p.comma_list(
                        TokenKind::RBracket,
                        "`,` or `]` in the list",
                        Self::expression,
                    )
                    .map(Expr::List)
                });
            }
            TokenKind::LBrace => return self.block(),
            TokenKind::If => return self.if_expression(),
            TokenKind::Wait | TokenKind::Over => {
                return Err(
                    self.error("`wait` and `over` only go directly in the body of a sequence")
                )
            }
            TokenKind::Start => {
                return Err(
                    self.error("`start` is a statement, not a value: it goes on its own line")
                )
            }
            _ => return Err(self.error("expected an expression")),
        };
        self.advance();
        Ok(expr)
    }

    fn block(&mut self) -> Result<Expr, ParseError> {
        let open_line = self.advance().line;
        let mut statements = self.with_structs(true, |p| {
            p.lines(false, "expected `}` to close the block", Self::statement)
        })?;
        match statements.pop() {
            Some(Stmt {
                kind: StmtKind::Expr(tail),
                line,
            }) => Ok(Expr::Block(statements, Box::new(tail), line)),
            Some(stmt) => Err(ParseError::at(
                stmt.line,
                "a block must end with an expression",
            )),
            None => Err(ParseError::at(open_line, "an empty block has no value")),
        }
    }

    fn block_after(&mut self, what: &str) -> Result<Expr, ParseError> {
        if self.peek() != &TokenKind::LBrace {
            return Err(self.error(format!("expected {what}")));
        }
        self.block()
    }

    fn if_expression(&mut self) -> Result<Expr, ParseError> {
        self.advance();
        let cond = self.condition()?;
        let then = self.block_after("`{` after the condition of `if`")?;

        if matches!(self.next_significant(), TokenKind::Else | TokenKind::Eof) {
            self.skip_newlines();
        }
        if !self.matches(&[TokenKind::Else]) {
            return Err(self.error("expected `else`: an `if` without one has no value"));
        }

        let otherwise = if self.peek() == &TokenKind::If {
            self.if_expression()?
        } else {
            self.block_after("`{` or `if` after `else`")?
        };
        Ok(Expr::If(
            Box::new(cond),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    fn condition(&mut self) -> Result<Expr, ParseError> {
        self.with_structs(false, Self::expression)
    }

    fn struct_literal(&mut self, name: String) -> Result<Expr, ParseError> {
        self.skip_newlines();
        let starts_with_field = matches!(self.peek(), TokenKind::Ident(_))
            && self.kind_at(1) == Some(&TokenKind::Colon);
        let base = if starts_with_field || self.peek() == &TokenKind::RBrace {
            None
        } else {
            let base = self.expression()?;
            self.skip_newlines();
            self.expect(TokenKind::Bar, "`|` after the struct being updated")?;
            Some(Box::new(base))
        };

        let mut seen = Vec::new();
        let fields = self.lines(true, "expected `}` to close the struct", |p| {
            let field = p.field_name(&mut seen, "this field is already given")?;
            p.expect(TokenKind::Colon, "`:` after the field name")?;
            Ok((field, p.expression()?))
        })?;
        Ok(Expr::Struct { name, base, fields })
    }

    fn field_name(
        &mut self,
        seen: &mut Vec<String>,
        duplicate: &str,
    ) -> Result<String, ParseError> {
        if matches!(self.peek(), TokenKind::Ident(name) if seen.contains(name)) {
            return Err(self.error(duplicate));
        }
        let name = self.expect_ident("a field name")?;
        seen.push(name.clone());
        Ok(name)
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.comma_list(
            TokenKind::RParen,
            "`)` to close the argument list",
            Self::expression,
        )
    }

    fn lines<T>(
        &mut self,
        commas: bool,
        closing: &str,
        mut item: impl FnMut(&mut Self) -> Result<T, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        self.blocks += 1;
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.matches(&[TokenKind::RBrace]) {
                break;
            }
            if self.is_at_end() {
                return Err(self.error(closing));
            }
            items.push(item(self)?);
            if self.is_at_end() {
                return Err(self.error(closing));
            }
            let separated = self.matches(&[TokenKind::Newline])
                || (commas && self.matches(&[TokenKind::Comma]));
            if !separated && self.peek() != &TokenKind::RBrace {
                return Err(self.error(if commas {
                    "expected `,`, a new line or `}` after the field"
                } else {
                    "expected a new line or `}` after the statement"
                }));
            }
        }
        self.blocks -= 1;
        Ok(items)
    }

    fn comma_list<T>(
        &mut self,
        close: TokenKind,
        what: &str,
        mut item: impl FnMut(&mut Self) -> Result<T, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        let mut items = Vec::new();
        while self.peek() != &close {
            items.push(item(self)?);
            if !self.matches(&[TokenKind::Comma]) {
                break;
            }
        }
        self.expect(close, what)?;
        Ok(items)
    }

    fn with_structs<T>(&mut self, allowed: bool, parse: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.struct_ok, allowed);
        let result = parse(self);
        self.struct_ok = outer;
        result
    }

    fn skip_newlines(&mut self) {
        while self.matches(&[TokenKind::Newline]) {}
    }

    fn next_significant(&self) -> &TokenKind {
        self.tokens[self.current..]
            .iter()
            .map(|t| &t.kind)
            .find(|kind| **kind != TokenKind::Newline)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.current].kind
    }

    fn kind_at(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens.get(self.current + offset).map(|t| &t.kind)
    }

    fn line(&self) -> usize {
        self.tokens[self.current].line
    }

    fn is_at_end(&self) -> bool {
        self.peek() == &TokenKind::Eof
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.current += 1;
        }
        &self.tokens[self.current - 1]
    }

    fn matches(&mut self, kinds: &[TokenKind]) -> bool {
        let found = kinds.contains(self.peek());
        if found {
            self.advance();
        }
        found
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<(), ParseError> {
        if self.matches(&[kind]) {
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
        _ => Err(ParseError::at(
            line,
            "the right side of `|>` must be a function or a call",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::scan_tokens;

    fn tree(source: &str) -> Result<String, String> {
        let join = |lines: Vec<String>| lines.join("\n");
        match parse(scan_tokens(source).expect("source should scan cleanly")) {
            Ok(statements) => Ok(join(statements.iter().map(|s| s.to_string()).collect())),
            Err(errors) => Err(join(errors.into_iter().map(|e| e.message).collect())),
        }
    }

    fn parses(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(tree(source), Ok(expected.to_string()), "{source:?}");
        }
    }

    fn rejects(cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(tree(source), Err(expected.to_string()), "{source:?}");
        }
    }

    #[test]
    fn expressions_follow_the_precedence_ladder() {
        parses(&[
            (
                "1\n0.2s\n\"hi\"\n:jump\ntrue",
                "1\n0.2\n\"hi\"\n:jump\ntrue",
            ),
            ("1 - 2 - 3", "(- (- 1 2) 3)"),
            ("(1 + 2) * 3", "(* (+ 1 2) 3)"),
            ("-1 * 2\n!!a", "(* (- 1) 2)\n(! (! a))"),
            (
                "a or b and c == d < e + f * g",
                "(or a (and b (== c (< d (+ e (* f g))))))",
            ),
        ]);
        rejects(&[
            ("(1 + 2", "expected `)` to close the group, at end of input"),
            ("1 + )", "expected an expression, found `)`"),
            ("let x = 1 +\n2", "expected an expression, at end of line"),
        ]);
        let error = parse_expr(scan_tokens("1 2").unwrap()).unwrap_err();
        assert_eq!(error.message, "unexpected trailing input, found `2`");
    }

    #[test]
    fn calls_dots_and_pipes_build_calls() {
        parses(&[
            (
                "f()\nf(1)(2)\nf(\n  1,\n  2\n)",
                "(call f)\n(call (call f 1) 2)\n(call f 1 2)",
            ),
            ("p.pos.x", "(. (. p pos) x)"),
            (
                "f(p, d)\np.f(d)\np |> f(d)",
                "(call f p d)\n(call f p d)\n(call f p d)",
            ),
            ("p |> g", "(call g p)"),
            (
                "let p = q\n  |> f(a)\n  |> g",
                "(let p (call g (call f q a)))",
            ),
            (
                "a + b |> f\nx |> v() and y",
                "(call f (+ a b))\n(and (call v x) y)",
            ),
        ]);
        rejects(&[(
            "a |> 1",
            "the right side of `|>` must be a function or a call",
        )]);
    }

    #[test]
    fn a_program_is_one_statement_per_line() {
        parses(&[("\nlet x = 1\n\nx\n", "(let x 1)\nx"), ("", "")]);
        rejects(&[
            (
                "let x = 1 2",
                "expected a new line after the statement, found `2`",
            ),
            (
                "let 1 = 2\nlet ok = 1\nlet x 1\n1 +",
                "expected a name after `let`, found `1`\n\
                 expected `=` after the name, found `1`\n\
                 expected an expression, at end of input",
            ),
            (
                "x = 1",
                "there is no assignment: bind a new value with `let`, \
                 or change a field of the state of a sequence, found `=`",
            ),
        ]);
    }

    #[test]
    fn errors_know_their_line_and_whether_the_input_ended() {
        let errors = |source: &str| parse(scan_tokens(source).unwrap()).unwrap_err();
        let lines: Vec<usize> = errors("let ok = 1\nlet 1 = 2\n\nlet x")
            .iter()
            .map(|e| e.line)
            .collect();
        assert_eq!(lines, [2, 4]);
        assert_eq!(errors("let a = {\n  let 1 = 2\n  3\n}")[0].line, 2);

        let at_end = |source: &str| errors(source).last().unwrap().at_end;
        assert!(at_end("f :: () -> Int {\n") && at_end("if a { 1 }\n") && at_end("(1 +\n"));
        assert!(!at_end("let x =\n") && !at_end("1 + )\n") && !at_end("{ let x = 1 }\n"));
    }

    #[test]
    fn a_block_is_an_expression() {
        parses(&[
            (
                "let a = {\n  let w = 4\n  w * 5\n}",
                "(let a (block (let w 4) (* w 5)))",
            ),
            ("{ 1 } + { { 2 } }", "(+ (block 1) (block (block 2)))"),
        ]);
        rejects(&[
            ("{\n  let a = 1\n}", "a block must end with an expression"),
            ("{ }", "an empty block has no value"),
            ("{\n  1", "expected `}` to close the block, at end of input"),
            (
                "let a = {\n  let 1 = 2\n  3\n}\nlet 2 = b",
                "expected a name after `let`, found `1`\nexpected a name after `let`, found `2`",
            ),
        ]);
    }

    #[test]
    fn parses_functions() {
        parses(&[
            (
                "add :: (a: Int, b: Int) -> Int {\n  a + b\n}",
                "(fn add (a: Int, b: Int) -> Int (block (+ a b)))",
            ),
            ("one :: () -> Int { 1 }", "(fn one () -> Int (block 1))"),
            (
                "f :: (m: Map[String, List[Int]]) -> Int { 1 }",
                "(fn f (m: Map[String, List[Int]]) -> Int (block 1))",
            ),
        ]);
        rejects(&[
            ("f :: (x) -> Int { x }", "expected `:` and a type after the parameter name, found `)`"),
            ("f :: (x: Int) { x }", "expected `->` and a return type, found `{`"),
            ("f :: (x: Int) -> Int x", "expected `{` to start the function body, found `x`"),
            ("{\n  f :: () -> Int { 1 }\n  1\n}", "functions can only be declared at the top level, found `f`"),
            (
                "speed :: 320.0",
                "expected `(`, `struct` or `sequence` after `::`; values are bound with `let`, found `320.0`",
            ),
        ]);
    }

    #[test]
    fn an_if_is_an_expression_with_an_else() {
        parses(&[
            (
                "if a { 1 } else if b { 2 } else { 3 }",
                "(if a (block 1) (if b (block 2) (block 3)))",
            ),
            (
                "if a {\n  1\n}\nelse {\n  2\n}",
                "(if a (block 1) (block 2))",
            ),
        ]);
        rejects(&[
            (
                "if a { 1 }",
                "expected `else`: an `if` without one has no value, at end of input",
            ),
            (
                "if a { 1 } else 2",
                "expected `{` or `if` after `else`, found `2`",
            ),
        ]);
    }

    #[test]
    fn parses_structs() {
        parses(&[
            (
                "B :: struct {\n  x: Float\n  y: Float,\n}\nE :: struct {}",
                "(struct B (x: Float, y: Float))\n(struct E ())",
            ),
            ("B { x: 1.0, y: 2.0 }\nE {}", "(B x: 1.0 y: 2.0)\n(E)"),
            ("let b = B {\n  x: 1.0,\n}", "(let b (B x: 1.0))"),
            ("B { f(b) | x: 1.0 }", "(B (call f b) | x: 1.0)"),
            (
                "if x > MAX { 1 } else { 2 }",
                "(if (> x MAX) (block 1) (block 2))",
            ),
            (
                "if (B { x: 1.0 }) == b { 1 } else { 2 }",
                "(if (== (B x: 1.0) b) (block 1) (block 2))",
            ),
        ]);
        rejects(&[
            (
                "struct B { x: Float }",
                "structs are declared as `Name :: struct { ... }`, found `struct`",
            ),
            (
                "b :: struct { x: Float }",
                "a struct name starts with an uppercase letter, found `b`",
            ),
            (
                "B :: struct { x: Float, x: Int }",
                "this field is already declared, found `x`",
            ),
            (
                "B :: struct { x }",
                "expected `:` and a type after the field name, found `}`",
            ),
            (
                "f :: () -> Int {\n  A :: struct {}\n  1\n}",
                "structs can only be declared at the top level, found `A`",
            ),
            (
                "B { x: 1.0, x: 2.0 }",
                "this field is already given, found `x`",
            ),
            (
                "B { b x: 1.0 }",
                "expected `|` after the struct being updated, found `x`",
            ),
        ]);
    }

    #[test]
    fn parses_lists() {
        parses(&[(
            "[]\n[1, 2 + 3]\n[\n  1,\n]",
            "(list)\n(list 1 (+ 2 3))\n(list 1)",
        )]);
        rejects(&[("[1 2]", "expected `,` or `]` in the list, found `2`")]);
    }

    #[test]
    fn parses_sequences() {
        parses(&[
            (
                "s :: sequence (b: Ball, v: Float) {\n  let from = b.x\n  wait 1.0s\n  over 0.5s as t {\n    b.x = from + t\n  }\n  over d {}\n}",
                "(sequence s (b: Ball, v: Float) (let from (. b x)) (over 1.0 _) (over 0.5 t (set b.x (+ from t))) (over d _))",
            ),
            ("start serve(w, 1.0)", "(start serve w 1.0)"),
        ]);
        rejects(&[
            (
                "sequence s(b: Ball) {}",
                "sequences are declared as `name :: sequence (state: Type) { ... }`, found `sequence`",
            ),
            ("s :: sequence () {}", "`s` needs a parameter: the first one is the state the sequence runs over"),
            (
                "s :: sequence (b: Ball, o: Ball) {\n  o.x = 1.0\n}",
                "only `b`, the state of the sequence, can have its fields changed, found `o`",
            ),
            (
                "s :: sequence (b: Ball) {\n  over 1.0s {\n    wait 1.0s\n  }\n}",
                "`wait` and `over` cannot go inside `over`: a sequence is a flat list of steps, found `wait`",
            ),
            (
                "s :: sequence (b: Ball) {\n  if ok { wait 1.0s } else { 1 }\n}",
                "`wait` and `over` only go directly in the body of a sequence, found `wait`",
            ),
            (
                "let x = start s(w)",
                "`start` is a statement, not a value: it goes on its own line, found `start`",
            ),
        ]);
    }
}
