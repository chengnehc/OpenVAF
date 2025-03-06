//! Defines input for code generation process.

use crate::to_upper_snake_case;

/// `SyntaxKind` sources
pub(crate) struct KindsSrc<'a> {
    pub(crate) punct: &'a [(&'a str, &'a str)],
    pub(crate) keywords: &'a [&'a str],
    pub(crate) literals: &'a [&'a str],
    pub(crate) tokens: &'a [&'a str],
    pub(crate) nodes: &'a [&'a str],
}

pub(crate) const KINDS_SRC: KindsSrc = KindsSrc {
    punct: &[
        (";", "SEMICOLON"),
        (",", "COMMA"),
        ("(", "L_PAREN"),
        (")", "R_PAREN"),
        ("{", "L_CURLY"),
        ("}", "R_CURLY"),
        ("[", "L_BRACK"),
        ("]", "R_BRACK"),
        ("<", "L_ANGLE"),
        (">", "R_ANGLE"),
        ("@", "AT"),
        ("#", "POUND"),
        ("~", "TILDE"),
        ("?", "QUESTION"),
        ("$", "DOLLAR"),
        ("&", "AMP"),
        ("|", "PIPE"),
        ("+", "PLUS"),
        ("*", "STAR"),
        ("/", "SLASH"),
        ("^", "CARET"),
        ("%", "PERCENT"),
        ("_", "UNDERSCORE"),
        (".", "DOT"),
        (":", "COLON"),
        ("=", "EQ"),
        ("==", "EQ2"),
        ("!", "BANG"),
        ("!=", "NEQ"),
        ("-", "MINUS"),
        ("<=", "LTEQ"),
        (">=", "GTEQ"),
        ("&&", "AMP2"),
        ("||", "PIPE2"),
        ("<<<", "ASHL"),
        (">>>", "ASHR"),
        ("<<", "SHL"),
        (">>", "SHR"),
        ("(*", "L_ATTR_PAREN"),
        ("*)", "R_ATTR_PAREN"),
        ("'{", "ARR_START"),
        ("<+", "CONTR"),
        ("**", "POW"),
        ("~^", "L_NXOR"),
        ("^~", "R_NXOR"),
    ],
    keywords: &[
        "analog",
        "begin",
        "branch",
        "case",
        "default",
        "disable",
        "discipline",
        "else",
        "end",
        "endcase",
        "enddiscipline",
        "endfunction",
        "endmodule",
        "endnature",
        "exclude",
        "for",
        "from",
        "function",
        "if",
        "inf",
        "inout",
        "input",
        "integer",
        "module",
        "nature",
        "output",
        "parameter",
        "localparam",
        "real",
        "string",
        "while",
        "root",
        "initial_step",
        "initial",
        "final_step",
        "aliasparam",
    ],
    literals: &["INT_NUMBER", "STD_REAL_NUMBER", "SI_REAL_NUMBER", "STR_LIT"],
    tokens: &["ERROR", "IDENT", "SYSFUN", "NET_TYPE", "WHITESPACE", "COMMENT"],
    nodes: &[
        "ALIAS_PARAM",
        "ANALOG_BEHAVIOUR",
        // "ARG",
        "ARG_LIST",
        "ARRAY_EXPR",
        "ASSIGN",
        "ASSIGN_STMT",
        // "ASSIGN_OR_EXPR",
        "ATTR",
        "ATTR_LIST",
        "BIN_EXPR",
        "BLOCK_SCOPE",
        "BLOCK_STMT",
        "BODY_PORT_DECL",
        "BRANCH_DECL",
        "CALL",
        "CASE",
        "CASE_STMT",
        "CONSTRAINT",
        "DIRECTION",
        "DISCIPLINE_DECL",
        "DISCIPLINE_ATTR",
        "EMPTY_STMT",
        "EVENT_STMT",
        "EXPR_STMT",
        "FOR_STMT",
        "FUNCTION",
        "FUNCTION_ARG",
        "IF_STMT",
        "LITERAL",
        "MODULE_DECL",
        "MODULE_PORT",
        "MODULE_PORTS",
        "NAME",
        "NAME_REF",
        "NATURE_DECL",
        "NATURE_ATTR",
        "NET_DECL",
        // "NETS",
        "PARAM",
        "PARAM_DECL",
        "PAREN_EXPR",
        "PATH",
        "PATH_EXPR",
        "PORT_DECL",
        "PORT_FLOW",
        // "PORTS",
        "PREFIX_EXPR",
        "RANGE",
        "SELECT_EXPR",
        "SOURCE_FILE",
        "SYS_FUN",
        "TYPE",
        "VAR",
        "VAR_DECL",
        "WHILE_STMT",
    ],
};

/// The parsed ungrammar `Grammar` will be lowered into `AstSrc` for
/// source code generation.
#[derive(Default, Debug)]
pub(crate) struct AstSrc {
    pub(crate) tokens: Vec<String>,
    pub(crate) nodes: Vec<AstNodeSrc>,
    pub(crate) enums: Vec<AstEnumSrc>,
}

#[derive(Debug)]
pub(crate) struct AstNodeSrc {
    pub(crate) doc: Vec<String>,
    pub(crate) name: String,
    pub(crate) fields: Vec<Field>,
    pub(crate) traits: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Field {
    Token(String),
    Node { name: String, ty: String, cardinality: Cardinality },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Cardinality {
    Optional, // ?
    Many,     // *
}

pub(crate) const MANUAL_ENUMS: [&str; 1] = ["Literal"];

#[derive(Debug)]
pub(crate) struct AstEnumSrc {
    pub(crate) doc: Vec<String>,
    pub(crate) name: String,
    pub(crate) variants: Vec<AstEnumVariant>,
    pub(crate) nested_variant: Option<String>,
    pub(crate) traits: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum AstEnumVariant {
    Node(String),
    Token(String),
}

impl AstEnumVariant {
    pub(crate) fn syntax_kind(&self) -> String {
        match self {
            AstEnumVariant::Token(ref name) => format!("{}_KW", to_upper_snake_case(name)),
            AstEnumVariant::Node(ref name) => to_upper_snake_case(name),
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            AstEnumVariant::Node(ref name) | AstEnumVariant::Token(ref name) => name,
        }
    }
}
