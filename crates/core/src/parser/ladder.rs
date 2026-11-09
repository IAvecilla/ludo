//! The precedence ladder, as in jlox: one function per level, each parsing
//! the operands of its operators with the level above it, which binds
//! tighter. Where a level sits in the chain is its precedence.

use super::{pipe_into, ParseError, Parser};
use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::token::TokenKind;

impl Parser {
    pub(super) fn ladder_expression(&mut self) -> Result<Expr, ParseError> {
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
                expr = self.dot(expr)?;
            } else {
                return Ok(expr);
            }
        }
    }
}
