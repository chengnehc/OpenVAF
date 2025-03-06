use rowan::TextSize;
use stdx::{impl_display, pretty};
use text_size::TextRange;

use crate::{ast, AstPtr, SyntaxKind, SyntaxNodePtr};

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

    /* Discipline */
    SurplusToken {
        found: SyntaxKind,
        range: TextRange,
    },
    MissingToken {
        expected: SyntaxKind,
        range: TextRange,
        expected_at: TextRange,
    },
    IllegalDisciplineAttrIdent {
        range: TextRange,
    },

    /* Path */
    IllegalRootSegment {
        path_segment: TextRange,
        prefix: Option<TextRange>,
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
    ItemsAfterFuncBody {
        items: Vec<AstPtr<ast::FunctionItem>>,
        body: TextRange,
    },
    MultipleFuncBodies {
        additional_bodys: Vec<TextRange>,
        body: AstPtr<ast::Stmt>,
    },
    FuncWithoutBody {
        fun: TextRange,
    },

    /* Branch declaration */
    IllegalBranchNodeCnt {
        arg_list: TextRange,
        cnt: usize,
    },
    IllegalBranchNodeExpr {
        single: bool,
        illegal_nodes: Vec<TextRange>,
    },

    /* Literal */
    IllegalInfToken {
        range: TextRange,
    },
    UnitsExpectedStringLiteral {
        range: TextRange,
    },

    /* Nature */
    IllegalNatureIdent {
        range: TextRange,
    },
    IllegalAttribute {
        range: TextRange,
        attr: &'static str,
        expected: &'static str,
    },

    /* Name */
    ReservedIdentifier {
        src: SyntaxNodePtr,
        compat: bool,
        name: String,
    },

    /* Module port */
    DuplicatePort {
        pos: Vec<TextRange>,
        name: String,
    },
    PortNotDeclaredInModule {
        head: TextRange,
        pos: TextRange,
        name: String,
    },
    MixedModuleHead {
        module_ports: AstPtr<ast::ModulePorts>,
    },
    IllegalBodyPorts {
        head: TextRange,
        body_ports: Vec<TextRange>,
    },

    /* Net type */
    IllegalNetType {
        found: String,
        range: TextRange,
    },

    /* Parameter */
    RangeConstraintForNonNumericParameter {
        name: String,
        range: TextRange,
        ty: TextRange,
    },
}

use SyntaxError::*;

impl_display! {
    match SyntaxError{
        UnexpectedToken{expected, found, ..} => "unexpected token {}; expected {}", found, expected;
        SurplusToken{found, ..} => "unexpected token {}", found;
        MissingToken{expected, ..} => "unexpected token; expected {}", expected;
        IllegalRootSegment{..} =>  "$root is only allowed as a prefix";
        BlockDeclsAfterStmt{..}  => "declarations in blocks are only allowed before the first stmt";
        BlockDeclsWithoutScope{..} => "declarations in blocks require an explicit scope";
        ItemsAfterFuncBody{..} => "functions may not contain any items after the function body";
        MultipleFuncBodies{..} => "functions may only contain one body";
        FuncWithoutBody{..} => "function is missing a body";
        IllegalBranchNodeCnt{cnt, ..} => "branch declaration require 1 or 2 nets; found {}", cnt;
        IllegalBranchNodeExpr{..} => "illegal expr was used to declare a branch node!";
        IllegalInfToken{..} => "unexpected token 'inf'; expected an expression";
        UnitsExpectedStringLiteral{..} => "'units' attribute must be a string literal";
        IllegalDisciplineAttrIdent{..} => "illegal discipline attribute identifier!";
        IllegalNatureIdent{..} => "illegal nature identifier";
        IllegalAttribute{attr, ..} => "illegal value provided for {} attribute", attr;
        ReservedIdentifier{name, ..} => "reserved keyword '{}' was used as an identifier", name;
        DuplicatePort{name, ..} => "port '{}' was declared multiple times!", name;
        MixedModuleHead{..} => "module header contains mix of port references and port declarations";
        IllegalBodyPorts{..} => "ports declared in module head and body";
        IllegalNetType{found, ..} => "{} nets are currently not supported!", found;
        RangeConstraintForNonNumericParameter{name, ..} => "non-numeric parameter '{}' has range bounds", name;
        PortNotDeclaredInModule{name, ..} => "port '{name}' was not declared in the module head";
    }
}
