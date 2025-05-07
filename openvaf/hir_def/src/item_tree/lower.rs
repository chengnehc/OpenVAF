use std::mem;
use std::sync::Arc;

use arena::IdxRange;
use basedb::{AstId, AstIdMap, FileId};
use syntax::ast::{self, ParamRef, PathSegmentKind};
use syntax::name::{kw, AsIdent, AsName};
use syntax::{match_ast, AstNode, WalkEvent};
use typed_index_collections::TiVec;

use crate::db::HirDefDB;
use crate::path::Path;
use crate::types::{AsType, Type};
use crate::{LocalFunctionArgId, LocalNodeId};

use super::{
    AliasParam, Block, Branch, BranchKind, Discipline, DisciplineAttr, DisciplineAttrKind, Domain,
    Function, FunctionArg, FunctionItem, ItemTree, ItemTreeId, Module, ModuleItem, Nature,
    NatureAttr, NatureRef, NatureRefKind, Net, Node, Param, Port, RootItem, Var,
};

fn is_input(direction: &Option<ast::Direction>) -> bool {
    direction.as_ref().is_some_and(|it| it.input_token().is_some() || it.inout_token().is_some())
}

fn is_output(direction: &Option<ast::Direction>) -> bool {
    direction.as_ref().is_some_and(|it| it.output_token().is_some() || it.inout_token().is_some())
}

pub(super) struct Context {
    tree: ItemTree,
    ast_id_map: Arc<AstIdMap>,
}

impl Context {
    pub(super) fn new(db: &dyn HirDefDB, file: FileId) -> Self {
        Self { tree: ItemTree::default(), ast_id_map: db.ast_id_map(file) }
    }

    pub(super) fn lower_root_items(mut self, file: &ast::SourceFile) -> ItemTree {
        self.tree.top_level = file.items().filter_map(|it| self.lower_root_item(it)).collect();
        self.tree
    }

    fn lower_root_item(&mut self, item: ast::Item) -> Option<RootItem> {
        let item = match item {
            ast::Item::NatureDecl(nature) => self.lower_nature(nature)?.into(),
            ast::Item::DisciplineDecl(discipline) => self.lower_discipline(discipline)?.into(),
            ast::Item::ModuleDecl(module) => self.lower_module(module)?.into(),
        };
        Some(item)
    }

    fn lower_nature(&mut self, decl: ast::NatureDecl) -> Option<ItemTreeId<Nature>> {
        let name = decl.name()?.as_name();
        let parent = decl.parent().and_then(|it| Self::lower_nature_path(&it));
        let attr_start = self.tree.data.nature_attrs.next_key();

        let mut access = None;
        let mut ddt_nature = None;
        let mut idt_nature = None;
        let mut units = None;
        let mut abstol = None;

        for (id, attr) in decl.nature_attrs().enumerate() {
            use kw::raw as kw;
            let Some(name) = attr.name().map(|name| name.as_name()) else { continue };
            // Handle predefined nature attributes
            match &*name {
                kw::access if access.is_none() => {
                    if let Some(name) = attr.val().and_then(|expr| expr.as_ident()) {
                        access = Some((name, id.into()));
                    }
                }
                kw::ddt_nature if ddt_nature.is_none() => {
                    if let Some(name) = attr.val().and_then(Self::lower_nature_expr) {
                        ddt_nature = Some((name, id.into()));
                    }
                }
                kw::idt_nature if idt_nature.is_none() => {
                    if let Some(name) = attr.val().and_then(Self::lower_nature_expr) {
                        idt_nature = Some((name, id.into()));
                    }
                }
                kw::units if units.is_none() => {
                    if let Some(ast::LiteralKind::StrLit(lit)) =
                        attr.val().and_then(|e| e.as_literal())
                    {
                        units = Some((lit.unescaped_value(), id.into()));
                    }
                }
                kw::abstol if abstol.is_none() => {
                    abstol = Some(id.into());
                }
                _ => (),
            };
            let ast_id = self.ast_id_map.id_of(&attr);
            self.tree.data.nature_attrs.push(NatureAttr { name, ast_id });
        }
        let attr_end = self.tree.data.nature_attrs.next_key();
        let ast_id = self.ast_id_map.id_of(&decl);

        let res = Nature {
            name,
            parent,
            access,
            ddt_nature,
            idt_nature,
            units,
            abstol,
            attrs: IdxRange::new(attr_start..attr_end),
            ast_id,
        };
        Some(self.tree.data.natures.push_and_get_key(res))
    }

    fn lower_nature_expr(expr: ast::Expr) -> Option<NatureRef> {
        let path = expr.as_path()?;
        Self::lower_nature_path(&path)
    }

    fn lower_nature_path(decl: &ast::Path) -> Option<NatureRef> {
        let mut name = decl.segment_token()?.as_name();

        let kind = match &*name {
            kw::raw::potential => NatureRefKind::DisciplinePotential,
            kw::raw::flow => NatureRefKind::DisciplineFlow,
            _ if decl.qualifier().is_none() && decl.segment_kind()? == PathSegmentKind::Name => {
                NatureRefKind::Nature
            }
            _ => return None,
        };

        if matches!(kind, NatureRefKind::DisciplineFlow | NatureRefKind::DisciplinePotential) {
            let qual = decl.qualifier()?;
            let segment = qual.segment()?;
            if segment.kind == PathSegmentKind::Root || qual.qualifier().is_some() {
                return None;
            }
            name = segment.as_name();
        }

        Some(NatureRef { name, kind })
    }

    fn lower_discipline(&mut self, decl: ast::DisciplineDecl) -> Option<ItemTreeId<Discipline>> {
        use kw::raw as kw;
        let name = decl.name()?.as_name();
        let ast_id = self.ast_id_map.id_of(&decl);
        let attr_start = self.tree.data.discipline_attrs.next_key();

        let mut potential = None;
        let mut flow = None;
        let mut domain = None;

        for (id, attr) in decl.discipline_attrs().enumerate() {
            let Some(name) = attr.name() else { continue };
            let kind = if let Some(qual) = name.qualifier() {
                let qual = qual.segment_token();
                match qual.as_ref().map(|t| t.text()) {
                    Some(kw::potential) => DisciplineAttrKind::PotentialOverride,
                    Some(kw::flow) => DisciplineAttrKind::FlowOverride,
                    _ => continue,
                }
            } else {
                DisciplineAttrKind::UserDefined
            };

            let Some(name) = name.segment_token().map(|t| t.as_name()) else { continue };
            match &*name {
                kw::potential if potential.is_none() => {
                    if let Some(name_ref) = attr.val().and_then(Self::lower_nature_expr) {
                        potential = Some((name_ref, id.into()))
                    }
                }
                kw::flow if flow.is_none() => {
                    if let Some(name_ref) = attr.val().and_then(Self::lower_nature_expr) {
                        flow = Some((name_ref, id.into()))
                    }
                }
                kw::domain if domain.is_none() => {
                    match attr.val().and_then(|expr| expr.as_ident()).as_deref() {
                        Some(kw::continuous) => {
                            domain = Some((Domain::Continuous, id.into()));
                        }
                        Some(kw::discrete) => {
                            domain = Some((Domain::Discrete, id.into()));
                        }
                        _ => (),
                    }
                }
                _ => (),
            };

            let ast_id = self.ast_id_map.id_of(&attr);

            self.tree.data.discipline_attrs.push(DisciplineAttr {
                name: name.clone(),
                kind,
                ast_id,
            });
        }

        let attr_end = self.tree.data.discipline_attrs.next_key();

        let res = Discipline {
            name,
            potential,
            flow,
            domain,
            attrs: IdxRange::new(attr_start..attr_end),
            ast_id,
        };
        Some(self.tree.data.disciplines.push_and_get_key(res))
    }

    fn lower_module(&mut self, decl: ast::ModuleDecl) -> Option<ItemTreeId<Module>> {
        let name = decl.name()?.as_name();
        let ast_id = self.ast_id_map.id_of(&decl);

        let mut nodes = TiVec::new();
        let mut items = Vec::new();
        if let Some(ports) = decl.module_ports() {
            self.lower_module_ports(ports, &mut nodes, &mut items);
        }
        let num_ports = nodes.len() as u32;
        self.lower_module_items(decl, &mut nodes, &mut items);

        let res = Module { name, num_ports, nodes, items, ast_id };
        Some(self.tree.data.modules.push_and_get_key(res))
    }

    fn lower_module_ports(
        &mut self,
        ports: ast::ModulePorts,
        nodes: &mut TiVec<LocalNodeId, Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        for port in ports.ports() {
            let ast_id = self.ast_id_map.id_of(&port);
            if let Some(decl) = port.decl() {
                self.lower_port(decl, nodes, dst);
            } else if let Some(name) = port.name() {
                let name = name.as_name();
                if nodes.iter().all(|node| node.name != name) {
                    let node = nodes.push_and_get_key(Node {
                        name,
                        is_port: true,
                        decls: Vec::new(),
                        ast_id: ast_id.into(),
                    });
                    dst.push(node.into())
                }
            }
        }
    }

    fn lower_module_items(
        &mut self,
        decl: ast::ModuleDecl,
        nodes: &mut TiVec<LocalNodeId, Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        for item in decl.module_items() {
            match item {
                ast::ModuleItem::BodyPortDecl(decl) => {
                    if let Some(port) = decl.port_decl() {
                        self.lower_port(port, nodes, dst);
                    }
                }
                ast::ModuleItem::AnalogBehaviour(behaviour) => {
                    if let Some(stmt) = behaviour.stmt() {
                        self.lower_stmt(stmt, dst);
                    }
                }
                ast::ModuleItem::NetDecl(net) => self.lower_net(net, nodes, dst),
                ast::ModuleItem::BranchDecl(branch) => self.lower_branch(branch, dst),
                ast::ModuleItem::VarDecl(var) => self.lower_var(var, dst),
                ast::ModuleItem::ParamDecl(param) => self.lower_param(param, dst),
                ast::ModuleItem::AliasParam(alias) => self.lower_aliasparam(alias, dst),
                ast::ModuleItem::Function(fun) => self.lower_func(fun, dst),
            };
        }
    }

    fn lower_port(
        &mut self,
        decl: ast::PortDecl,
        nodes: &mut TiVec<LocalNodeId, Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        let discipline = decl.discipline().map(|it| it.as_name());
        let direction = decl.direction();
        let is_gnd = decl.net_type_token().is_some_and(|it| it.text() == kw::raw::ground);
        let ast_id = self.ast_id_map.id_of(&decl);

        for (name_idx, name) in decl.names().enumerate() {
            let name = name.as_name();
            let id = self.tree.data.ports.push_and_get_key(Port {
                name: name.clone(),
                name_idx,
                discipline: discipline.clone(),
                is_input: is_input(&direction),
                is_output: is_output(&direction),
                is_gnd,
                ast_id,
            });
            match nodes.iter_mut().find(|node| node.name == name) {
                Some(node) => node.decls.push(id.into()),
                None => {
                    let node = nodes.push_and_get_key(Node {
                        name,
                        is_port: true,
                        decls: vec![id.into()],
                        ast_id: ast_id.into(),
                    });
                    dst.push(node.into())
                }
            }
        }
    }

    fn lower_net(
        &mut self,
        decl: ast::NetDecl,
        nodes: &mut TiVec<LocalNodeId, Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        let discipline = decl.discipline().map(|it| it.as_name());
        let is_gnd = decl.net_type_token().is_some_and(|it| it.text() == kw::raw::ground);
        let ast_id = self.ast_id_map.id_of(&decl);

        for (name_idx, name) in decl.names().enumerate() {
            let name = name.as_name();
            let id = self.tree.data.nets.push_and_get_key(Net {
                name: name.clone(),
                name_idx,
                discipline: discipline.clone(),
                is_gnd,
                ast_id,
            });
            match nodes.iter_mut().find(|node| node.name == name) {
                Some(node) => node.decls.push(id.into()),
                None => {
                    let node = nodes.push_and_get_key(Node {
                        name,
                        is_port: false,
                        decls: vec![id.into()],
                        ast_id: ast_id.into(),
                    });
                    dst.push(node.into());
                }
            }
        }
    }

    fn lower_branch(&mut self, decl: ast::BranchDecl, dst: &mut Vec<ModuleItem>) {
        let ast_id = self.ast_id_map.id_of(&decl);
        let kind = decl
            .branch_kind()
            .and_then(|kind| {
                let res = match kind {
                    ast::BranchKind::PortFlow(flow) => {
                        BranchKind::PortFlow(Path::resolve(flow.port()?)?)
                    }
                    ast::BranchKind::NodeGnd(path) => BranchKind::NodeGnd(Path::resolve(path)?),
                    ast::BranchKind::Nodes(hi, lo) => {
                        BranchKind::Nodes(Path::resolve(hi)?, Path::resolve(lo)?)
                    }
                };
                Some(res)
            })
            .unwrap_or(BranchKind::Missing);
        for (name_idx, name) in decl.names().enumerate() {
            let branch = Branch { name: name.as_name(), name_idx, kind: kind.clone(), ast_id };
            let id = self.tree.data.branches.push_and_get_key(branch);
            dst.push(id.into());
        }
    }

    fn lower_var<T: From<ItemTreeId<Var>>>(&mut self, decl: ast::VarDecl, dst: &mut Vec<T>) {
        let ty = decl.ty().as_type();
        for var in decl.vars() {
            let Some(name) = var.name() else { continue };
            let ast_id = self.ast_id_map.id_of(&var);
            let var = Var { name: name.as_name(), ty: ty.clone(), ast_id };
            let id = self.tree.data.variables.push_and_get_key(var);
            dst.push(id.into())
        }
    }

    fn lower_param<T: From<ItemTreeId<Param>>>(&mut self, decl: ast::ParamDecl, dst: &mut Vec<T>) {
        let ty = decl.ty().map(|ty| ty.as_type());
        for param in decl.params() {
            let Some(name) = param.name() else { continue };
            let ast_id = self.ast_id_map.id_of(&param);
            let param = Param {
                name: name.as_name(),
                ty: ty.clone(),
                is_local: decl.localparam_token().is_some(),
                ast_id,
            };
            let id = self.tree.data.parameters.push_and_get_key(param);
            dst.push(id.into())
        }
    }

    fn lower_aliasparam<T: From<ItemTreeId<AliasParam>>>(
        &mut self,
        decl: ast::AliasParam,
        dst: &mut Vec<T>,
    ) {
        let name = decl.name();
        let src = decl.src();

        if let (Some(name), Some(src)) = (name, src) {
            let ast_id = self.ast_id_map.id_of(&decl);
            let src = match src {
                ParamRef::Path(path) => Path::resolve(path),
                ParamRef::SysFun(fun) => Some(Path::from_ident(fun.as_name())),
            };
            let param = AliasParam { name: name.as_name(), src, ast_id };
            let param = self.tree.data.aliasparams.push_and_get_key(param);
            dst.push(param.into())
        }
    }

    fn lower_func(&mut self, fun: ast::Function, dst: &mut Vec<ModuleItem>) {
        let mut items = Vec::new();
        let mut args: TiVec<LocalFunctionArgId, FunctionArg> = TiVec::new();

        for item in fun.function_items() {
            match item {
                ast::FunctionItem::ParamDecl(param) => self.lower_param(param, &mut items),
                ast::FunctionItem::VarDecl(var) => self.lower_var(var, &mut items),
                ast::FunctionItem::Stmt(stmt) => self.lower_stmt(stmt, &mut items),
                ast::FunctionItem::FunctionArg(arg) => {
                    let ast_id = self.ast_id_map.id_of(&arg);
                    let is_input = is_input(&arg.direction());
                    let is_output = is_output(&arg.direction());
                    for (name_idx, name) in arg.names().enumerate() {
                        let name = name.as_name();
                        if let Some(arg) = args.iter_mut().find(|arg| arg.name == name) {
                            // TODO validation
                            arg.ast_ids.push(ast_id)
                        }
                        let arg = args.push_and_get_key(FunctionArg {
                            name,
                            name_idx,
                            is_input,
                            is_output,
                            declarations: Vec::new(),
                            ast_ids: vec![ast_id],
                        });
                        items.push(arg.into());
                    }
                }
            }
        }

        // de-duplicate corresponding FunctionArg and Variable and put the variable
        // as the argument declaration.
        items.retain(|decl| {
            if let FunctionItem::Variable(var) = decl {
                if let Some(arg) = args.iter_mut().find(|arg| arg.name == self.tree[*var].name) {
                    // TODO validation
                    arg.declarations.push(*var);
                    return false;
                }
            };
            true
        });

        if let Some(name) = fun.name() {
            let fun = Function {
                name: name.as_name(),
                ty: fun.ty().map_or(Type::Real, |ty| ty.as_type()),
                args,
                items,
                ast_id: self.ast_id_map.id_of(&fun),
            };
            let fun = self.tree.data.functions.push_and_get_key(fun);
            dst.push(fun.into())
        }
    }

    // TODO: separate out lower_func_arg to a fn

    fn lower_stmt<T>(&mut self, stmt: ast::Stmt, dst: &mut Vec<T>)
    where
        T: From<ItemTreeId<Param>> + From<ItemTreeId<Var>> + From<AstId<ast::BlockStmt>>,
    {
        let mut block_stack = Vec::new();
        let mut block_scope_stack = Vec::new();
        let mut blocks = mem::take(&mut self.tree.blocks);

        for event in stmt.syntax().preorder() {
            match event {
                WalkEvent::Enter(node) => {
                    match_ast! {
                    match node {
                        ast::BlockStmt(block) => {
                            let ast_id = self.ast_id_map.id_of(&block);
                            let name = block.block_scope().and_then(|it| Some(it.name()?.as_name()));
                            let block_info = Block { name, block_items: Vec::new()};
                            // # Note
                            // - only register named blocks as ModuleItem/FunctionItem/BlockItem
                            // - only insert block items when the it is named (has a scope)
                            if block.block_scope().is_some() {
                                match block_scope_stack.last() {
                                    Some(block) => {
                                        let block_info = blocks.get_mut(block).unwrap();
                                        block_info.block_items.push(ast_id.into());
                                    }
                                    None => dst.push(ast_id.into()),
                                };
                                block_scope_stack.push(ast_id);
                            }
                            blocks.insert(ast_id, block_info);
                            block_stack.push(ast_id);
                        },
                        ast::VarDecl(var) => {
                            match block_stack.last() {
                                Some(block) => {
                                    let block = blocks.get_mut(block).unwrap();
                                    self.lower_var(var, &mut block.block_items)
                                }
                                None => self.lower_var(var, dst),
                            }
                        },
                        ast::ParamDecl(param) => {
                            match block_stack.last() {
                                Some(block) => {
                                    let block = blocks.get_mut(block).unwrap();
                                    self.lower_param(param, &mut block.block_items)
                                }
                                None => self.lower_param(param, dst),
                            }
                        },
                        _ => ()
                    }
                    }
                }
                WalkEvent::Leave(node) => {
                    if let Some(block) = ast::BlockStmt::cast(node) {
                        block_stack.pop();
                        if block.block_scope().is_some() {
                            block_scope_stack.pop();
                        }
                    }
                }
            }
        }
        self.tree.blocks = blocks;
    }
}
