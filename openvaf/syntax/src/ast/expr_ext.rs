//! Various extension methods to ast Expr Nodes, which are hard to code-generate.

use crate::ast::operators::{AssignOp, BinaryOp, UnaryOp};
use crate::ast::{self, support, AstChildren, AstNode, AstToken};
use crate::{SyntaxToken, T};

impl ast::Expr {
    pub fn as_literal(&self) -> Option<LiteralKind> {
        if let ast::Expr::Literal(lit) = self {
            Some(lit.kind())
        } else {
            None
        }
    }

    pub fn as_str_literal(&self) -> Option<String> {
        if let Some(LiteralKind::StrLit(lit)) = self.as_literal() {
            Some(lit.unescaped_value())
        } else {
            None
        }
    }
}

impl ast::Literal {
    pub fn token(&self) -> SyntaxToken {
        self.syntax()
            .children_with_tokens()
            .find(|e| !e.kind().is_trivia())
            .and_then(|e| e.into_token())
            .unwrap()
    }

    pub fn kind(&self) -> LiteralKind {
        let token = self.token();

        if let Some(t) = ast::IntNumber::cast(token.clone()) {
            return LiteralKind::IntNumber(t);
        }
        if let Some(t) = ast::SiRealNumber::cast(token.clone()) {
            return LiteralKind::SiRealNumber(t);
        }
        if let Some(t) = ast::StdRealNumber::cast(token.clone()) {
            return LiteralKind::StdRealNumber(t);
        }
        if let Some(t) = ast::StrLit::cast(token.clone()) {
            return LiteralKind::StrLit(t);
        }

        match token.kind() {
            T![inf] => LiteralKind::Inf,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LiteralKind {
    StrLit(ast::StrLit),
    IntNumber(ast::IntNumber),
    SiRealNumber(ast::SiRealNumber),
    StdRealNumber(ast::StdRealNumber),
    Inf,
}

impl ast::PrefixExpr {
    pub fn op_kind(&self) -> Option<UnaryOp> {
        match self.op_token()?.kind() {
            T![~] => Some(UnaryOp::BitNegate),
            T![!] => Some(UnaryOp::Not),
            T![-] => Some(UnaryOp::Neg),
            T![+] => Some(UnaryOp::Identity),
            _ => None,
        }
    }

    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.syntax().first_child_or_token()?.into_token()
    }
}

impl ast::BinExpr {
    pub fn op_details(&self) -> Option<(SyntaxToken, BinaryOp)> {
        self.syntax().children_with_tokens().filter_map(|it| it.into_token()).find_map(|c| {
            let bin_op = match c.kind() {
                T![||] => BinaryOp::BooleanOr,
                T![&&] => BinaryOp::BooleanAnd,
                T![==] => BinaryOp::EqualityTest,
                T![!=] => BinaryOp::NegatedEqualityTest,
                T![<=] => BinaryOp::LesserEqualTest,
                T![>=] => BinaryOp::GreaterEqualTest,
                T![<] => BinaryOp::LesserTest,
                T![>] => BinaryOp::GreaterTest,
                T![+] => BinaryOp::Addition,
                T![*] => BinaryOp::Multiplication,
                T![-] => BinaryOp::Subtraction,
                T![/] => BinaryOp::Division,
                T![%] => BinaryOp::Remainder,
                T![<<] => BinaryOp::LeftShift,
                T![>>] => BinaryOp::RightShift,
                T![^] => BinaryOp::BitwiseXor,
                T![|] => BinaryOp::BitwiseOr,
                T![&] => BinaryOp::BitwiseAnd,
                T![**] => BinaryOp::Power,
                T![~^] | T![^~] => BinaryOp::BitwiseEq,
                _ => return None,
            };
            Some((c, bin_op))
        })
    }

    pub fn op_kind(&self) -> Option<BinaryOp> {
        self.op_details().map(|t| t.1)
    }

    pub fn op_token(&self) -> Option<SyntaxToken> {
        self.op_details().map(|t| t.0)
    }

    pub fn lhs(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).next()
    }

    pub fn rhs(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).nth(1)
    }

    // pub fn sub_exprs(&self) -> (Option<ast::Expr>, Option<ast::Expr>) {
    //     let mut children = support::children(self.syntax());
    //     let first = children.next();
    //     let second = children.next();
    //     (first, second)
    // }
}

pub enum ArrayExprKind {
    Repeat { initializer: Option<ast::Expr>, repeat: Option<ast::Expr> },
    ElementList(AstChildren<ast::Expr>),
}

impl ast::ArrayExpr {
    pub fn kind(&self) -> ArrayExprKind {
        if self.is_repeat() {
            ArrayExprKind::Repeat {
                initializer: support::children(self.syntax()).next(),
                repeat: support::children(self.syntax()).nth(1),
            }
        } else {
            ArrayExprKind::ElementList(support::children(self.syntax()))
        }
    }

    fn is_repeat(&self) -> bool {
        self.syntax().children_with_tokens().any(|it| it.kind() == T![;])
    }
}

impl ast::SelectExpr {
    pub fn then_val(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).nth(1)
    }

    pub fn else_val(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).nth(2)
    }
}

/* Statements */

impl ast::Assign {
    pub fn op(&self) -> Option<AssignOp> {
        if support::token(self.syntax(), T![=]).is_some() {
            Some(AssignOp::Assign)
        } else if support::token(self.syntax(), T![<+]).is_some() {
            Some(AssignOp::Contribute)
        } else {
            None
        }
    }

    pub fn lval(&self) -> Option<ast::Expr> {
        support::child(self.syntax())
    }

    pub fn rval(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).nth(1)
    }
}
