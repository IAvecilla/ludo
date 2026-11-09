//! Pratt parsing, as in clox: one loop reads a prefix expression and keeps
//! folding infix operators into it while they bind at least as tightly as the
//! level it was asked for. Precedence is data in a table, not the shape of a
//! chain of calls.

use super::{pipe_into, ParseError, Parser};
use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::token::TokenKind;

/// How tightly an infix operator binds, from loosest to tightest. The loop in
/// `parse_precedence` compares levels with this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Or,
    And,
    Equality,
    Comparison,
    Pipe,
    Term,
    Factor,
    Unary,
    Call,
}

impl Prec {
    /// The level right above this one. A left-associative operator parses its
    /// right operand there, so in `1 - 2 - 3` the second `-` is left for the
    /// loop that already holds `1 - 2`.
    fn next(self) -> Prec {
        match self {
            Prec::Or => Prec::And,
            Prec::And => Prec::Equality,
            Prec::Equality => Prec::Comparison,
            Prec::Comparison => Prec::Pipe,
            Prec::Pipe => Prec::Term,
            Prec::Term => Prec::Factor,
            Prec::Factor => Prec::Unary,
            Prec::Unary | Prec::Call => Prec::Call,
        }
    }
}

/// What an infix token builds out of the expression to its left.
#[derive(Debug, Clone, Copy)]
enum Infix {
    Binary(BinaryOp),
    Pipe,
    Call,
    Dot,
}

/// The table of infix rules: how tightly each token that can follow an
/// expression binds, and what it builds. A token without a rule ends the
/// expression.
fn infix_rule(kind: &TokenKind) -> Option<(Prec, Infix)> {
    let binary = |prec: Prec, op: BinaryOp| Some((prec, Infix::Binary(op)));
    match kind {
        TokenKind::Or => binary(Prec::Or, BinaryOp::Or),
        TokenKind::And => binary(Prec::And, BinaryOp::And),
        TokenKind::EqEq => binary(Prec::Equality, BinaryOp::Eq),
        TokenKind::BangEq => binary(Prec::Equality, BinaryOp::Ne),
        TokenKind::Less => binary(Prec::Comparison, BinaryOp::Lt),
        TokenKind::LessEq => binary(Prec::Comparison, BinaryOp::Le),
        TokenKind::Greater => binary(Prec::Comparison, BinaryOp::Gt),
        TokenKind::GreaterEq => binary(Prec::Comparison, BinaryOp::Ge),
        TokenKind::PipeGt => Some((Prec::Pipe, Infix::Pipe)),
        TokenKind::Plus => binary(Prec::Term, BinaryOp::Add),
        TokenKind::Minus => binary(Prec::Term, BinaryOp::Sub),
        TokenKind::Star => binary(Prec::Factor, BinaryOp::Mul),
        TokenKind::Slash => binary(Prec::Factor, BinaryOp::Div),
        TokenKind::Percent => binary(Prec::Factor, BinaryOp::Rem),
        TokenKind::LParen => Some((Prec::Call, Infix::Call)),
        TokenKind::Dot => Some((Prec::Call, Infix::Dot)),
        _ => None,
    }
}

impl Parser {
    pub(super) fn pratt_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_precedence(Prec::Or)
    }

    /// Parses an expression made only of operators that bind at least as
    /// tightly as `min`; a looser one is left for the caller.
    fn parse_precedence(&mut self, min: Prec) -> Result<Expr, ParseError> {
        let mut left = self.prefix()?;
        while let Some((prec, rule)) = infix_rule(self.peek()).filter(|&(prec, _)| prec >= min) {
            let line = self.advance().line;
            left = match rule {
                Infix::Binary(op) => {
                    let right = self.parse_precedence(prec.next())?;
                    Expr::Binary(Box::new(left), op, Box::new(right))
                }
                Infix::Pipe => pipe_into(left, self.parse_precedence(prec.next())?, line)?,
                Infix::Call => Expr::Call(Box::new(left), self.arguments()?),
                Infix::Dot => self.dot(left)?,
            };
        }
        Ok(left)
    }

    /// The prefix rules: `-` and `!` take an operand that only calls and
    /// fields bind to, and every other token starts a primary expression.
    fn prefix(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            _ => return self.primary(),
        };
        self.advance();
        Ok(Expr::Unary(
            op,
            Box::new(self.parse_precedence(Prec::Unary)?),
        ))
    }
}
