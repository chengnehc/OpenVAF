use stdx::{impl_display, pretty};
use text_size::TextRange;

use crate::{ast, AstPtr, SyntaxKind, SyntaxNodePtr, TextSize};

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
    PortNotDeclaredInModule {
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

use SyntaxError::*;

impl_display! {
    match SyntaxError{
        UnexpectedToken{expected, found, ..} => "unexpected token {}; expected {}", found, expected;
        ReservedIdentifier{name, ..} => "reserved keyword '{name}' was used as an identifier";
        IllegalRootSegment{..} =>  "$root is only allowed as a prefix";
        IllegalInfToken{..} => "unexpected token 'inf'; expected an expression";
        IllegalNatureIdent{..} => "illegal nature identifier";
        IllegalAttribute{attr, ..} => "illegal value provided for {} attribute", attr;
        SurplusToken{found, ..} => "unexpected token {}", found;
        MissingToken{expected, ..} => "unexpected token; expected {}", expected;
        IllegalDisciplineAttrPath{..} => "illegal discipline attribute path";
        IllegalBodyPorts{..} => "ports declared in module head and body";
        PortNotDeclaredInModule{name, ..} => "port '{name}' was not declared in the module head";
        MixedModuleHead{..} => "module header contains mix of port references and port declarations";
        DuplicatePort{name, ..} => "port '{name}' was declared multiple times";
        IllegalNetType{found, ..} => "{} nets are currently not supported", found;
        IllegalBranchNodeCnt{cnt, ..} => "branch declaration require 1 or 2 nets; found {cnt}";
        IllegalBranchNodeExpr{..} => "illegal expr was used to declare a branch node!";
        RangeConstraintForNonNumericParameter{name, ..} => "non-numeric parameter '{name}' has range bounds";
        BlockDeclsAfterStmt{..}  => "declarations in blocks are only allowed before the first stmt";
        BlockDeclsWithoutScope{..} => "declarations in blocks require an explicit scope";
        FuncWithoutBody{..} => "function is missing a body";
        FuncWithoutArg{..} => "function is missing arguments";
        ItemsAfterFuncBody{..} => "functions may not contain any items after the function body";
        MultipleFuncBodies{..} => "functions may only contain one body";
        NamedFuncBodyBlock{..} => "functions shall not use named blocks";
    }
}
