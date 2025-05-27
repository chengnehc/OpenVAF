use rowan::TextRange;
use tokens::{SyntaxKind, T};

use crate::ast::{
    self, support, ArgListOwner, BlockItem, ConstraintValue, Expr, FunctionItem, LiteralKind,
    PathSegmentKind, Stmt,
};
use crate::name::{kw, kw_comp};
use crate::{match_ast, AsName, AstNode, AstPtr, SyntaxError, SyntaxNode, SyntaxNodePtr};

pub(crate) fn validate(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    for node in root.descendants() {
        match_ast! {
            match node {
                ast::Name(name) => validate_name(name, errors),
                ast::Path(path) => validate_path(path, errors),
                ast::Literal(lit) => validate_literal(lit, errors),
                ast::NatureDecl(decl) => validate_nature_decl(decl, errors),
                ast::NatureAttr(attr) => validate_nature_attr(attr, errors),
                ast::DisciplineDecl(decl) => validate_discipline_decl(decl, errors),
                ast::ModuleDecl(module) => validate_module(module, errors),
                ast::BranchDecl(branch) => validate_branch(branch, errors),
                ast::ParamDecl(param) => validate_param(param, errors),
                ast::BlockStmt(block) => validate_block(block, errors),
                ast::Function(fun) => validate_function(fun, errors),
                _ => validate_net_type_token(node, errors)
            }
        }
    }
}

fn validate_name(name: ast::Name, errors: &mut Vec<SyntaxError>) {
    let Some(ident) = name.ident_token() else { return };
    let parent = name.syntax().parent();
    let p = parent.as_ref();

    let compat = match ident.text() {
        kw::raw::units if p.is_some_and(|p| p.kind() == SyntaxKind::ATTR) => return,
        kw::raw::units
        | kw::raw::idt_nature
        | kw::raw::ddt_nature
        | kw::raw::abstol
        | kw::raw::access
            if p.is_some_and(|p| p.kind() == SyntaxKind::NATURE_ATTR) =>
        {
            return
        }
        kw::raw::domain | kw::raw::potential | kw::raw::flow
            if p.is_some_and(|p| p.kind() == SyntaxKind::DISCIPLINE_ATTR) =>
        {
            return
        }
        ident if kw::is_reserved(ident) => false,
        ident if kw_comp::is_reserved(ident) => true,
        _ => return,
    };

    errors.push(SyntaxError::ReservedIdentifier {
        src: SyntaxNodePtr::new(name.syntax()),
        compat,
        name: ident.text().to_owned(),
    })
}

fn validate_path(path: ast::Path, errors: &mut Vec<SyntaxError>) {
    // $root without subsequent identifier
    if path.segment_kind() == Some(PathSegmentKind::Root) && path.parent().is_none() {
        errors.push(SyntaxError::IllegalRootSegment {
            path_segment: path.segment_token().unwrap().text_range(),
            prefix: None,
        })
    }
    // $root does not appear as ultimate prefix
    for qual in path.qualifiers() {
        if qual.qualifier().is_some() && path.segment_kind() == Some(PathSegmentKind::Root) {
            errors.push(SyntaxError::IllegalRootSegment {
                path_segment: path.segment_token().unwrap().text_range(),
                prefix: Some(path.top_path().syntax().text_range()),
            })
        }
    }
}

fn validate_literal(literal: ast::Literal, errors: &mut Vec<SyntaxError>) {
    if literal.kind() == ast::LiteralKind::Inf
        && !literal.syntax().parent().is_none_or(is_valid_inf_position)
    {
        errors.push(SyntaxError::IllegalInfToken { range: literal.syntax().text_range() });
    }
}

fn is_valid_inf_position(s: SyntaxNode) -> bool {
    if s.kind() == SyntaxKind::RANGE {
        return true;
    }
    if s.parent().is_some_and(|parent| parent.kind() == SyntaxKind::RANGE) {
        if let Some(expr) = ast::PrefixExpr::cast(s) {
            if matches!(expr.op_kind(), Some(ast::UnaryOp::Neg) | None) {
                return true;
            }
        }
    }
    false
}

fn validate_nature_decl(nature: ast::NatureDecl, errors: &mut Vec<SyntaxError>) {
    if let Some(parent) = nature.parent() {
        check_nature_path(&parent, errors)
    }
    for attr in nature.nature_attrs() {
        if let (Some(name), Some(val)) = (attr.name(), attr.val()) {
            match &*name.as_name() {
                "ddt_nature" | "idt_nature" => check_nature_ref(&val, errors),
                "access" if val.as_raw_ident().is_none() => {
                    errors.push(SyntaxError::IllegalAttribute {
                        attr: "access",
                        expected: "an identifier",
                        range: val.syntax().text_range(),
                    })
                }
                _ => (),
            }
        }
    }
}

fn check_nature_ref(expr: &Expr, errors: &mut Vec<SyntaxError>) {
    if let Expr::PathExpr(path) = expr {
        if let Some(path) = path.path() {
            check_nature_path(&path, errors)
        }
    } else {
        errors.push(SyntaxError::IllegalNatureIdent { range: expr.syntax().text_range() })
    }
}

fn check_nature_path(path: &ast::Path, errors: &mut Vec<SyntaxError>) {
    if path.qualifier().is_none() && path.segment_kind() == Some(ast::PathSegmentKind::Name) {
        return;
    }
    if let Some(name) = path.segment_token().as_ref().map(|name| name.text()) {
        if name == "potential" || name == "flow" {
            let qual = path.qualifier().unwrap();
            if qual.qualifier().is_none() && qual.segment_kind() != Some(ast::PathSegmentKind::Root)
            {
                return;
            }
        }
    }
    errors.push(SyntaxError::IllegalNatureIdent { range: path.syntax().text_range() });
}

fn validate_nature_attr(attr: ast::NatureAttr, errors: &mut Vec<SyntaxError>) {
    if attr.name().is_some_and(|name| name.text() == "units") {
        let expr = attr.val().unwrap();
        if let Expr::Literal(literal) = &expr {
            if let LiteralKind::StrLit(_) = literal.kind() {
                return;
            }
        }
        errors.push(SyntaxError::IllegalAttribute {
            attr: "units",
            expected: "a string literal",
            range: expr.syntax().text_range(),
        });
    }
}

fn validate_discipline_decl(discipline: ast::DisciplineDecl, errors: &mut Vec<SyntaxError>) {
    for attr in discipline.discipline_attrs() {
        if let Some(name) = attr.name() {
            let is_overwrite = match name.qualifier() {
                None => false,
                Some(qual) => {
                    let text = qual.syntax().text();
                    if (text == "potential" || text == "flow") && qual.qualifier().is_none() {
                        true
                    } else {
                        errors.push(SyntaxError::IllegalDisciplineAttrPath {
                            range: name.syntax().text_range(),
                        });
                        continue;
                    }
                }
            };

            let name_text = name.syntax().text().to_string();
            match &*name_text {
                "domain" | "potential" | "flow" => {
                    if let Some(tok) = attr.eq_token() {
                        errors.push(SyntaxError::SurplusToken {
                            found: T![=],
                            range: tok.text_range(),
                        })
                    }
                }
                _ if attr.eq_token().is_none() => {
                    if let Some(val) = attr.val() {
                        errors.push(SyntaxError::MissingToken {
                            expected: T![=],
                            range: val.syntax().text_range(),
                            expected_at: TextRange::at(name.syntax().text_range().end(), 0.into()),
                        })
                    }
                }
                _ => (),
            }

            if let Some(val) = attr.val() {
                match &*name_text {
                    "potential" | "flow" => check_nature_ref(&val, errors),
                    "idt_nature" | "ddt_nature" if is_overwrite => {
                        // TODO(JW): is it really OK to override these two?
                        check_nature_ref(&val, errors)
                    }
                    "domain" => {
                        let text = val.syntax().text();
                        if text != "continuous" && text != "discrete" {
                            errors.push(SyntaxError::IllegalAttribute {
                                attr: "domain",
                                expected: "continuous or discrete",
                                range: val.syntax().text_range(),
                            })
                        }
                    }
                    _ => (),
                }
            }
        }
    }
}

fn validate_module(module: ast::ModuleDecl, errors: &mut Vec<SyntaxError>) {
    let Some(ports) = module.module_ports() else { return };
    match validate_module_ports(&ports, errors) {
        Some((true, _)) => {
            let body_ports: Vec<_> =
                module.body_ports().map(|port| port.syntax().text_range()).collect();
            if !body_ports.is_empty() {
                errors.push(SyntaxError::IllegalBodyPorts {
                    head: ports.syntax().text_range(),
                    body_ports,
                })
            }
        }
        Some((false, names)) => {
            for port in module.body_ports() {
                if let Some(decl) = port.port_decl() {
                    for name in decl.names() {
                        if names.binary_search_by(|locs| locs[0].text().cmp(&name.text())).is_err()
                        {
                            errors.push(SyntaxError::PortNotDeclaredInModuleHead {
                                head: ports.syntax().text_range(),
                                pos: name.syntax().text_range(),
                                name: name.text().to_owned(),
                            })
                        }
                    }
                }
            }
        }
        None => (),
    }
}

fn validate_module_ports(
    ports: &ast::ModulePorts,
    errors: &mut Vec<SyntaxError>,
) -> Option<(bool, Vec<Vec<ast::Name>>)> {
    let mut names: Vec<Vec<ast::Name>> = Vec::new();
    let mut has_decl = false;
    for port in ports.ports() {
        if let Some(name) = port.name() {
            match names.binary_search_by(|locs| locs[0].text().cmp(&name.text())) {
                Ok(pos) => names[pos].push(name.clone()),
                Err(pos) => names.insert(pos, vec![name.clone()]),
            }
        } else {
            has_decl = true
        }
    }

    if !names.is_empty() && has_decl {
        errors.push(SyntaxError::MixedModuleHead { module_ports: AstPtr::new(ports) });
        // Don't lint body ports when the head is ambiguous
        return None;
    }

    for locs in &names {
        if locs.len() == 1 {
            continue;
        }
        let name = locs[0].text().to_owned();
        errors.push(SyntaxError::DuplicatePort {
            pos: locs.iter().map(|it| it.syntax().text_range()).collect(),
            name,
        })
    }

    Some((has_decl, names))
}

fn validate_branch(decl: ast::BranchDecl, errors: &mut Vec<SyntaxError>) {
    let Some(arg_list) = decl.arg_list() else { return };
    match arg_list.args().count() {
        1 => {
            let arg = arg_list.args().next().unwrap();
            match arg {
                ast::Expr::PortFlow(_) => (),
                ast::Expr::PathExpr(path)
                    if path.path().is_none_or(|path| path.qualifier().is_none()) => {}
                _ => errors.push(SyntaxError::IllegalBranchNodeExpr {
                    single: true,
                    illegal_nodes: vec![arg.syntax().text_range()],
                }),
            }
        }
        2 => {
            let illegal_nodes: Vec<_> = arg_list
                .args()
                .filter_map(|arg| arg.as_path().is_none().then_some(arg.syntax().text_range()))
                .collect();

            if !illegal_nodes.is_empty() {
                errors.push(SyntaxError::IllegalBranchNodeExpr { single: false, illegal_nodes })
            }
        }
        cnt => errors.push(SyntaxError::IllegalBranchNodeCnt {
            arg_list: arg_list.syntax().text_range(),
            cnt,
        }),
    }
}

fn validate_param(param_decl: ast::ParamDecl, errors: &mut Vec<SyntaxError>) {
    let range_allowed =
        param_decl.ty().is_none_or(|ty| ty.integer_token().is_some() | ty.real_token().is_some());
    if range_allowed {
        return;
    }
    for param in param_decl.params() {
        for constraint in param.constraints() {
            if matches!(constraint.val(), Some(ConstraintValue::Range(_))) {
                errors.push(SyntaxError::RangeConstraintForNonNumericParameter {
                    name: param.name().unwrap().text().to_owned(),
                    range: constraint.syntax().text_range(),
                    ty: param_decl.ty().unwrap().syntax().text_range(),
                });
            }
        }
    }
}

fn validate_function(fun: ast::Function, errors: &mut Vec<SyntaxError>) {
    let mut items = fun.function_items();

    let body = loop {
        match items.next() {
            Some(FunctionItem::Stmt(stmt)) => break stmt,
            None => {
                errors.push(SyntaxError::FuncWithoutBody { fun: fun.syntax().text_range() });
                return;
            }
            _ => (),
        }
    };

    if fun.args().next().is_none() {
        errors.push(SyntaxError::FuncWithoutArg { fun: fun.syntax().text_range() });
    };

    if let Stmt::BlockStmt(blk) = &body {
        if let Some(scope) = blk.block_scope() {
            let name = scope.name().unwrap();
            errors.push(SyntaxError::NamedFuncBodyBlock { name: name.syntax().text_range() })
        }
    }

    let illegal_items: Vec<_> = items
        .clone()
        .filter(|item| !matches!(item, FunctionItem::Stmt(_)))
        .map(|item| AstPtr::new(&item))
        .collect();

    if !illegal_items.is_empty() {
        errors.push(SyntaxError::ItemsAfterFuncBody {
            items: illegal_items,
            body: body.syntax().text_range(),
        })
    }

    let additional_bodies: Vec<_> = items
        .filter_map(|item| {
            if let FunctionItem::Stmt(stmt) = item {
                Some(stmt.syntax().text_range())
            } else {
                None
            }
        })
        .collect();

    if !additional_bodies.is_empty() {
        errors.push(SyntaxError::MultipleFuncBodies { additional_bodies, body: AstPtr::new(&body) })
    }
}

fn validate_block(block: ast::BlockStmt, errors: &mut Vec<SyntaxError>) {
    if block.block_scope().is_some() {
        let mut items = block.items();

        let first_stmt = loop {
            match items.next() {
                Some(BlockItem::Stmt(stmt)) => break stmt,
                Some(BlockItem::ParamDecl(_) | BlockItem::VarDecl(_)) => (),
                None => return,
            }
        };

        let misplaced_decls: Vec<_> = items
            .filter_map(|item| {
                matches!(item, ast::BlockItem::VarDecl(_) | ast::BlockItem::ParamDecl(_))
                    .then(|| AstPtr::new(&item))
            })
            .collect();

        if !misplaced_decls.is_empty() {
            errors.push(SyntaxError::BlockDeclsAfterStmt {
                decls: misplaced_decls,
                first_stmt: first_stmt.syntax().text_range(),
            })
        }
    } else {
        let decls: Vec<_> = block
            .items()
            .filter_map(|item| {
                matches!(item, ast::BlockItem::VarDecl(_) | ast::BlockItem::ParamDecl(_))
                    .then(|| AstPtr::new(&item))
            })
            .collect();

        if !decls.is_empty() {
            if let Some(begin_token) = block.begin_token() {
                errors.push(SyntaxError::BlockDeclsWithoutScope {
                    decls,
                    begin_token: begin_token.text_range(),
                })
            }
        }
    }
}

fn validate_net_type_token(node: SyntaxNode, errors: &mut Vec<SyntaxError>) {
    if matches!(node.kind(), SyntaxKind::NET_DECL | SyntaxKind::PORT_DECL) {
        if let Some(token) = support::token(&node, SyntaxKind::NET_TYPE) {
            if token.text() != kw::raw::ground {
                errors.push(SyntaxError::IllegalNetType {
                    found: token.text().to_owned(),
                    range: token.text_range(),
                })
            }
        }
    }
}
