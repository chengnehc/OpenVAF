use stdx::{impl_display, pretty};

use crate::{ast, AstPtr, SyntaxKind, SyntaxNodePtr, TextRange, TextSize};

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub enum SyntaxError {
    /* Parser */
    UnexpectedToken {
        expected: pretty::List<Vec<SyntaxKind>>,
        found: SyntaxKind,
        range: TextRange,
        panic_end: Option<TextSize>,
        expected_at: Option<TextRange>,
        missing_delimiter: bool,
    },

    /* Name */
    ReservedIdentifier {
        src: SyntaxNodePtr,
        compat: bool,
        name: String,
    },

    /* Path */
    IllegalRootSegment {
        path_segment: TextRange,
        prefix: Option<TextRange>,
    },

    /* Literal */
    IllegalInfToken {
        range: TextRange,
    },

    /* Nature and Discipline */
    IllegalNatureIdent {
        range: TextRange,
    },
    IllegalAttribute {
        range: TextRange,
        attr: &'static str,
        expected: &'static str,
    },
    SurplusToken {
        found: SyntaxKind,
        range: TextRange,
    },
    MissingToken {
        expected: SyntaxKind,
        range: TextRange,
        expected_at: TextRange,
    },
    IllegalDisciplineAttrPath {
        range: TextRange,
    },

    /* Module */
    IllegalBodyPorts {
        head: TextRange,
        body_ports: Vec<TextRange>,
    },
    PortNotDeclaredInModuleHead {
        head: TextRange,
        pos: TextRange,
        name: String,
    },
    MixedModuleHead {
        module_ports: AstPtr<ast::ModulePorts>,
    },
    DuplicatePort {
        pos: Vec<TextRange>,
        name: String,
    },

    /* Net type */
    IllegalNetType {
        found: String,
        range: TextRange,
    },

    /* Branch */
    IllegalBranchNodeCnt {
        arg_list: TextRange,
        cnt: usize,
    },
    IllegalBranchNodeExpr {
        single: bool,
        illegal_nodes: Vec<TextRange>,
    },

    /* Parameter */
    RangeConstraintForNonNumericParameter {
        name: String,
        range: TextRange,
        ty: TextRange,
    },

    /* Block */
    BlockDeclsAfterStmt {
        decls: Vec<AstPtr<ast::BlockItem>>,
        first_stmt: TextRange,
    },
    BlockDeclsWithoutScope {
        decls: Vec<AstPtr<ast::BlockItem>>,
        begin_token: TextRange,
    },

    /* Function */
    FuncWithoutBody {
        fun: TextRange,
    },
    FuncWithoutArg {
        fun: TextRange,
    },
    ItemsAfterFuncBody {
        items: Vec<AstPtr<ast::FunctionItem>>,
        body: TextRange,
    },
    MultipleFuncBodies {
        additional_bodies: Vec<TextRange>,
        body: AstPtr<ast::Stmt>,
    },
    NamedFuncBodyBlock {
        name: TextRange,
    },
}

impl_display! {
    match SyntaxError{
        Self::UnexpectedToken{expected, found, ..} => "unexpected token {}; expected {}", found, expected;
        Self::ReservedIdentifier{name, ..} => "reserved keyword '{name}' was used as an identifier";
        Self::IllegalRootSegment{..} =>  "$root is only allowed as a prefix";
        Self::IllegalInfToken{..} => "unexpected token 'inf'; expected an expression";
        Self::IllegalNatureIdent{..} => "illegal nature identifier";
        Self::IllegalAttribute{attr, ..} => "illegal value provided for {} attribute", attr;
        Self::SurplusToken{found, ..} => "unexpected token {}", found;
        Self::MissingToken{expected, ..} => "unexpected token; expected {}", expected;
        Self::IllegalDisciplineAttrPath{..} => "illegal discipline attribute path";
        Self::IllegalBodyPorts{..} => "ports declared in module head and body";
        Self::PortNotDeclaredInModuleHead{name, ..} => "port '{name}' was not declared in the module head";
        Self::MixedModuleHead{..} => "module header contains mix of port references and port declarations";
        Self::DuplicatePort{name, ..} => "port '{name}' was declared multiple times";
        Self::IllegalNetType{found, ..} => "{} nets are currently not supported", found;
        Self::IllegalBranchNodeCnt{cnt, ..} => "branch declaration require 1 or 2 nets; found {cnt}";
        Self::IllegalBranchNodeExpr{..} => "illegal expr was used to declare a branch node!";
        Self::RangeConstraintForNonNumericParameter{name, ..} => "non-numeric parameter '{name}' has range bounds";
        Self::BlockDeclsAfterStmt{..}  => "declarations in blocks are only allowed before the first stmt";
        Self::BlockDeclsWithoutScope{..} => "declarations in blocks require an explicit scope";
        Self::FuncWithoutBody{..} => "function is missing a body";
        Self::FuncWithoutArg{..} => "function is missing arguments";
        Self::ItemsAfterFuncBody{..} => "functions may not contain any items after the function body";
        Self::MultipleFuncBodies{..} => "functions may only contain one body";
        Self::NamedFuncBodyBlock{..} => "functions shall not use named blocks";
    }
}
