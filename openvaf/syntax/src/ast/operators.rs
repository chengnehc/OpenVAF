use stdx::{impl_debug, impl_display};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// The `~` operator for bit inversion
    BitNegate,
    /// The `!` operator for logical inversion
    Not,
    /// The `-` operator for negation
    Neg,
    /// The `+` operator (does absolutely nothing)
    Identity,
}

impl_display! {
    match UnaryOp{
        UnaryOp::BitNegate => "~";
        UnaryOp::Not => "!";
        UnaryOp::Neg => "-";
        UnaryOp::Identity => "+";
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// The `||` operator for boolean OR
    BooleanOr,
    /// The `&&` operator for boolean AND
    BooleanAnd,
    /// The `==` operator for equality testing
    EqualityTest,
    /// The `!=` operator for equality testing
    NegatedEqualityTest,
    /// The `<=` operator for lesser-equal testing
    LesserEqualTest,
    /// The `>=` operator for greater-equal testing
    GreaterEqualTest,
    /// The `<` operator for comparison
    LesserTest,
    /// The `>` operator for comparison
    GreaterTest,
    /// The `+` operator for addition
    Addition,
    /// The `-` operator for subtraction
    Subtraction,
    /// The `*` operator for multiplication
    Multiplication,
    /// The `/` operator for division
    Division,
    /// The `%` operator for remainder after division
    Remainder,
    /// The `**` operator for exponents
    Power,
    /// The `<<` operator for left shift
    LeftShift,
    /// The `>>` operator for right shift
    RightShift,
    /// The `|` operator for bitwise OR
    BitwiseOr,
    /// The `&` operator for bitwise AND
    BitwiseAnd,
    /// The `^` operator for bitwise XOR (exclusive OR)
    BitwiseXor,
    /// The `~^`/`^~` operator for bitwise XNOR (exclusive NOR), also named bitwise equivalence
    BitwiseXnor,
}

impl_display! {
    match BinaryOp{
        BinaryOp::BooleanOr => "||";
        BinaryOp::BooleanAnd => "&&";
        BinaryOp::EqualityTest => "==";
        BinaryOp::NegatedEqualityTest => "!=";
        BinaryOp::LesserEqualTest => "<=";
        BinaryOp::GreaterEqualTest => ">=";
        BinaryOp::LesserTest => "<";
        BinaryOp::GreaterTest => ">";
        BinaryOp::Addition => "+";
        BinaryOp::Subtraction => "-";
        BinaryOp::Multiplication => "*";
        BinaryOp::Division => "/";
        BinaryOp::Remainder => "%";
        BinaryOp::Power => "**";
        BinaryOp::LeftShift => "<<";
        BinaryOp::RightShift => ">>";
        BinaryOp::BitwiseOr => "|";
        BinaryOp::BitwiseAnd => "&";
        BinaryOp::BitwiseXor => "^";
        BinaryOp::BitwiseXnor => "~^";
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum AssignOp {
    /// a contribute (<+) stmt
    Contribute,
    /// a variable assignment (=) stmt
    Assign,
}

impl_debug! {
    match AssignOp {
        AssignOp::Contribute => "<+";
        AssignOp::Assign => "=";
    }
}
