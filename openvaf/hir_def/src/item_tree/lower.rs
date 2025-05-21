//! Lower AST items and store them in item tree.

use std::mem;
use std::sync::Arc;

use arena::{Arena, IdxRange};
use basedb::{AstId, AstIdMap, FileId};
use syntax::name::{kw, AsIdent, AsName};
use syntax::{ast, AstNode, SyntaxNodePtr, WalkEvent};

use crate::db::HirDefDB;
use crate::path::Path;
use crate::types::{AsType, Type};

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
        self.tree.root_items = file.items().filter_map(|it| self.lower_root_item(it)).collect();
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
        let ast_id = self.ast_id_map.id_of(&decl);
        let parent = decl.parent().and_then(|path| Self::lower_nature_ref(&path));

        let attr_start = self.tree.data.nature_attrs.next_key();

        let mut access = None;
        let mut ddt_nature = None;
        let mut idt_nature = None;
        let mut units = None;
        let mut abstol = None;

        for (id, attr) in decl.nature_attrs().enumerate() {
            let id = id.into();
            let Some(name) = attr.name().map(|name| name.as_name()) else { continue };
            let ast_id = self.ast_id_map.id_of(&attr);

            use kw::raw as kw;
            match &*name {
                kw::access if access.is_none() => {
                    if let Some(name) = attr.val().and_then(|expr| expr.as_ident()) {
                        access = Some((name, id));
                    }
                }
                kw::ddt_nature if ddt_nature.is_none() => {
                    if let Some(name) = attr.val().and_then(Self::lower_nature_expr) {
                        ddt_nature = Some((name, id));
                    }
                }
                kw::idt_nature if idt_nature.is_none() => {
                    if let Some(name) = attr.val().and_then(Self::lower_nature_expr) {
                        idt_nature = Some((name, id));
                    }
                }
                kw::units if units.is_none() => {
                    if let Some(ast::LiteralKind::StrLit(lit)) =
                        attr.val().and_then(|e| e.as_literal())
                    {
                        units = Some((lit.unescaped_value(), id));
                    }
                }
                kw::abstol if abstol.is_none() => abstol = Some(id),

                _ => (),
            };
            self.tree.data.nature_attrs.push(NatureAttr { name, ast_id });
        }
        let attr_end = self.tree.data.nature_attrs.next_key();

        let res = Nature {
            name,
            ast_id,
            parent,
            access,
            ddt_nature,
            idt_nature,
            units,
            abstol,
            attrs: IdxRange::new(attr_start..attr_end),
        };

        Some(self.tree.data.natures.push_and_get_key(res))
    }

    fn lower_nature_expr(expr: ast::Expr) -> Option<NatureRef> {
        let path = expr.as_path()?;
        Self::lower_nature_ref(&path)
    }

    fn lower_nature_ref(path: &ast::Path) -> Option<NatureRef> {
        let mut name = path.segment_token()?.as_name();

        let kind = match &*name {
            kw::raw::potential => NatureRefKind::DisciplinePotential,
            kw::raw::flow => NatureRefKind::DisciplineFlow,
            _ if path.qualifier().is_none()
                && path.segment_kind()? == ast::PathSegmentKind::Name =>
            {
                NatureRefKind::Nature
            }
            _ => return None,
        };

        if matches!(kind, NatureRefKind::DisciplineFlow | NatureRefKind::DisciplinePotential) {
            let qual = path.qualifier()?;
            let segment = qual.segment()?;
            if segment.kind == ast::PathSegmentKind::Root || qual.qualifier().is_some() {
                return None;
            }
            name = segment.as_name();
        }

        Some(NatureRef { name, kind, src: SyntaxNodePtr::new(path.syntax()) })
    }

    fn lower_discipline(&mut self, decl: ast::DisciplineDecl) -> Option<ItemTreeId<Discipline>> {
        let name = decl.name()?.as_name();
        let ast_id = self.ast_id_map.id_of(&decl);

        let attr_start = self.tree.data.discipline_attrs.next_key();

        let mut potential = None;
        let mut flow = None;
        let mut domain = None;

        for (id, attr) in decl.discipline_attrs().enumerate() {
            let id = id.into();
            let Some(name) = attr.name() else { continue };
            let kind = if let Some(qual) = name.qualifier() {
                match qual.segment_token().as_ref().map(|t| t.text()) {
                    Some(kw::potential) => DisciplineAttrKind::PotentialOverride,
                    Some(kw::flow) => DisciplineAttrKind::FlowOverride,
                    _ => continue,
                }
            } else {
                DisciplineAttrKind::UserDefined
            };

            let Some(name) = name.segment_token().map(|t| t.as_name()) else { continue };

            use kw::raw as kw;
            match &*name {
                kw::potential if potential.is_none() => {
                    if let Some(nature_ref) = attr.val().and_then(Self::lower_nature_expr) {
                        potential = Some((nature_ref, id))
                    }
                }
                kw::flow if flow.is_none() => {
                    if let Some(nature_ref) = attr.val().and_then(Self::lower_nature_expr) {
                        flow = Some((nature_ref, id))
                    }
                }
                kw::domain if domain.is_none() => {
                    match attr.val().and_then(|expr| expr.as_ident()).as_deref() {
                        Some(kw::continuous) => domain = Some((Domain::Continuous, id)),
                        Some(kw::discrete) => domain = Some((Domain::Discrete, id)),
                        _ => (),
                    }
                }
                _ => (),
            };

            let ast_id = self.ast_id_map.id_of(&attr);
            self.tree.data.discipline_attrs.push(DisciplineAttr {
                name: name.clone(),
                ast_id,
                kind,
            });
        }

        let attr_end = self.tree.data.discipline_attrs.next_key();

        let res = Discipline {
            name,
            ast_id,
            potential,
            flow,
            domain,
            attrs: IdxRange::new(attr_start..attr_end),
        };

        Some(self.tree.data.disciplines.push_and_get_key(res))
    }

    fn lower_module(&mut self, decl: ast::ModuleDecl) -> Option<ItemTreeId<Module>> {
        let name = decl.name()?.as_name();
        let ast_id = self.ast_id_map.id_of(&decl);

        let mut nodes = Arena::new();
        let mut items = Vec::new();

        if let Some(ports) = decl.module_ports() {
            self.lower_module_ports(ports, &mut nodes, &mut items);
        }
        let num_ports = nodes.len() as u32;
        self.lower_module_items(decl, &mut nodes, &mut items);

        let module = Module { name, ast_id, num_ports, nodes, items };
        Some(self.tree.data.modules.push_and_get_key(module))
    }

    fn lower_module_ports(
        &mut self,
        ports: ast::ModulePorts,
        nodes: &mut Arena<Node>,
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
                        ast_id: ast_id.into(),
                        is_port: true,
                        decls: Vec::new(),
                    });
                    dst.push(node.into())
                }
            }
        }
    }

    fn lower_module_items(
        &mut self,
        decl: ast::ModuleDecl,
        nodes: &mut Arena<Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        for item in decl.module_items() {
            match item {
                ast::ModuleItem::BodyPortDecl(decl) => {
                    if let Some(port) = decl.port_decl() {
                        self.lower_port(port, nodes, dst);
                    }
                }
                ast::ModuleItem::AnalogBehavior(behaviour) => {
                    if let Some(stmt) = behaviour.stmt() {
                        self.lower_block_scope(stmt, dst);
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
        nodes: &mut Arena<Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        let ast_id = self.ast_id_map.id_of(&decl);
        let discipline = decl.discipline().map(|it| it.as_name());
        let direction = decl.direction();
        let is_input = is_input(&direction);
        let is_output = is_output(&direction);
        let is_gnd = decl.net_type_token().is_some_and(|it| it.text() == kw::raw::ground);

        for (name_idx, name) in decl.names().enumerate() {
            let name = name.as_name();
            let port = self.tree.data.ports.push_and_get_key(Port {
                name_idx,
                name: name.clone(),
                ast_id,
                discipline: discipline.clone(),
                is_input,
                is_output,
                is_gnd,
            });
            match nodes.iter_mut().find(|node| node.name == name) {
                Some(node) => node.decls.push(port.into()),
                None => {
                    let node = nodes.push_and_get_key(Node {
                        name,
                        ast_id: ast_id.into(),
                        is_port: true,
                        decls: vec![port.into()],
                    });
                    dst.push(node.into())
                }
            }
        }
    }

    fn lower_net(
        &mut self,
        decl: ast::NetDecl,
        nodes: &mut Arena<Node>,
        dst: &mut Vec<ModuleItem>,
    ) {
        let ast_id = self.ast_id_map.id_of(&decl);
        let discipline = decl.discipline().map(|it| it.as_name());
        let is_gnd = decl.net_type_token().is_some_and(|it| it.text() == kw::raw::ground);

        for (name_idx, name) in decl.names().enumerate() {
            let name = name.as_name();
            let net = self.tree.data.nets.push_and_get_key(Net {
                name_idx,
                name: name.clone(),
                ast_id,
                discipline: discipline.clone(),
                is_gnd,
            });
            match nodes.iter_mut().find(|node| node.name == name) {
                Some(node) => node.decls.push(net.into()),
                None => {
                    let node = nodes.push_and_get_key(Node {
                        name,
                        ast_id: ast_id.into(),
                        is_port: false,
                        decls: vec![net.into()],
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
            let branch = Branch { name_idx, name: name.as_name(), ast_id, kind: kind.clone() };
            let id = self.tree.data.branches.push_and_get_key(branch);
            dst.push(id.into());
        }
    }

    fn lower_var<T: From<ItemTreeId<Var>>>(&mut self, decl: ast::VarDecl, dst: &mut Vec<T>) {
        let ty = decl.ty().as_type();
        for var in decl.vars() {
            let Some(name) = var.name() else { continue };
            let ast_id = self.ast_id_map.id_of(&var);
            let var = Var { name: name.as_name(), ast_id, ty: ty.clone() };
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
                ast_id,
                ty: ty.clone(),
                is_local: decl.localparam_token().is_some(),
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
        if let (Some(name), Some(param_ref)) = (decl.name(), decl.param_ref()) {
            let ast_id = self.ast_id_map.id_of(&decl);
            let param = AliasParam { name: name.as_name(), ast_id, param_ref: param_ref.as_name() };
            let id = self.tree.data.aliasparams.push_and_get_key(param);
            dst.push(id.into())
        }
    }

    fn lower_func(&mut self, fun: ast::Function, dst: &mut Vec<ModuleItem>) {
        let Some(name) = fun.name() else { return };
        let ast_id = self.ast_id_map.id_of(&fun);

        let mut items = Vec::new();
        let mut args = Arena::new();

        for item in fun.function_items() {
            match item {
                ast::FunctionItem::ParamDecl(param) => self.lower_param(param, &mut items),
                ast::FunctionItem::VarDecl(var) => self.lower_var(var, &mut items),
                ast::FunctionItem::FunctionArg(arg) => {
                    self.lower_func_arg(arg, &mut args, &mut items)
                }
                _ => (),
            }
        }
        // combine correlated function argument and variable declarations
        // so that name resolution won't report false errors
        items.retain(|decl| {
            if let FunctionItem::Variable(var) = decl {
                if let Some(arg) = args.iter_mut().find(|arg| arg.name == self.tree[*var].name) {
                    arg.var_binds.push(*var);
                    return false;
                }
            }
            true
        });

        // return type is real by default if elided
        let ty = fun.ty().map_or(Type::Real, |ty| ty.as_type());

        let fun = Function { name: name.as_name(), ast_id, ty, args, items };
        let id = self.tree.data.functions.push_and_get_key(fun);
        dst.push(id.into())
    }

    fn lower_func_arg(
        &mut self,
        arg: ast::FunctionArg,
        args: &mut Arena<FunctionArg>,
        dst: &mut Vec<FunctionItem>,
    ) {
        let ast_id = self.ast_id_map.id_of(&arg);

        for (name_idx, name) in arg.names().enumerate() {
            let name = name.as_name();
            let arg = args.push_and_get_key(FunctionArg {
                name_idx,
                name,
                ast_id,
                is_input: is_input(&arg.direction()),
                is_output: is_output(&arg.direction()),
                var_binds: Vec::new(),
            });
            dst.push(arg.into());
        }
    }

    fn lower_block_scope<T>(&mut self, stmt: ast::Stmt, dst: &mut Vec<T>)
    where
        T: From<ItemTreeId<Param>> + From<ItemTreeId<Var>> + From<AstId<ast::BlockStmt>>,
    {
        let mut block_stack = Vec::new();
        let mut scoped_block_stack = Vec::new();
        let mut blocks = mem::take(&mut self.tree.blocks);

        for event in stmt.syntax().preorder() {
            match event {
                WalkEvent::Enter(node) => {
                    if let Some(block) = ast::BlockStmt::cast(node.clone()) {
                        let ast_id = self.ast_id_map.id_of(&block);
                        let name = block.block_scope().and_then(|it| Some(it.name()?.as_name()));
                        let block_info = Block { name, items: Vec::new() };
                        if block.block_scope().is_some() {
                            // only named blocks are counted as items
                            // only named blocks have items
                            match scoped_block_stack.last() {
                                Some(block) => {
                                    let block_info = blocks.get_mut(block).unwrap();
                                    block_info.items.push(ast_id.into());
                                }
                                None => dst.push(ast_id.into()),
                            };
                            scoped_block_stack.push(ast_id);
                        }
                        blocks.insert(ast_id, block_info);
                        block_stack.push(ast_id);
                    }
                    // variable and parameter declarations are only allowed in named blocks
                    else if let Some(var) = ast::VarDecl::cast(node.clone()) {
                        match block_stack.last() {
                            Some(block) => {
                                let block = blocks.get_mut(block).unwrap();
                                self.lower_var(var, &mut block.items)
                            }
                            None => self.lower_var(var, dst),
                        }
                    } else if let Some(param) = ast::ParamDecl::cast(node.clone()) {
                        match block_stack.last() {
                            Some(block) => {
                                let block = blocks.get_mut(block).unwrap();
                                self.lower_param(param, &mut block.items)
                            }
                            None => self.lower_param(param, dst),
                        }
                    }
                }
                WalkEvent::Leave(node) => {
                    if let Some(block) = ast::BlockStmt::cast(node) {
                        block_stack.pop();
                        if block.block_scope().is_some() {
                            scoped_block_stack.pop();
                        }
                    }
                }
            }
        }
        self.tree.blocks = blocks;
    }
}
