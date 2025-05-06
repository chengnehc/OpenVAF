//! Collect `DefMap` for the root scope (global scope) and items that define new scopes.
//!
//! In terms of Verilog-A subset, according to the scope rules given by LRM 6.8,
//! the following elements define a new scope:
//! — modules
//! — named blocks
//! — analog functions

use std::sync::Arc;

use arena::Arena;
use basedb::{AstId, FileId};
use indexmap::IndexMap;
use syntax::ast;
use syntax::name::Name;

use crate::builtin;
use crate::db::HirDefDB;
use crate::item_tree::{
    BlockItem, Function, FunctionItem, ItemTree, ItemTreeId, ItemTreeNode, Module, ModuleItem,
    RootItem,
};
use crate::{
    BlockId, BlockLoc, DisciplineLoc, FunctionArgLoc, FunctionId, FunctionLoc, Intern, ItemLoc,
    Lookup, ModuleLoc, NatureAttrLoc, NatureLoc, NodeLoc, Scope,
};

use super::{
    DefDiagnostic, DefMap, DefMapSource, LocalScopeId, ScopeData, ScopeItemDef, ScopeOrigin,
};

impl DefMap {
    pub fn root_query(db: &dyn HirDefDB, root_file: FileId) -> Arc<DefMap> {
        let tree = &db.item_tree(root_file);
        let scope_cnt =
            tree.data.natures.len() + tree.data.disciplines.len() + tree.data.modules.len();
        let def_map = DefMap {
            src: DefMapSource::Root,
            scopes: Arena::with_capacity(scope_cnt),
            root_scope: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        };
        let c = Collector { root_file, db, tree, def_map };

        c.collect_root_map()
    }

    pub fn block_query(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
        let BlockLoc { parent, ast_id } = block.lookup(db);
        let tree = &db.item_tree(parent.root_file);
        let items = &tree[ast_id].block_items;
        if items.is_empty() {
            return None;
        }
        let def_map = DefMap {
            src: DefMapSource::Block(block),
            scopes: Arena::with_capacity(1),
            root_scope: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        };
        let c = Collector { def_map, tree, db, root_file: parent.root_file };

        Some(c.collect_block_map(block, items))
    }

    pub fn function_query(db: &dyn HirDefDB, fun: FunctionId) -> Arc<DefMap> {
        let FunctionLoc { scope: Scope { root_file, local_id, .. }, id } = fun.lookup(db);
        let tree = &db.item_tree(root_file);
        let def_map = DefMap {
            src: DefMapSource::Function(fun),
            scopes: Arena::with_capacity(tree.data.modules.len() + 1),
            root_scope: LocalScopeId::from(0u32), // This will be changed once the scope has been created
            diagnostics: Vec::new(),
        };
        let c = Collector { root_file, db, tree, def_map };

        c.collect_function_map(id, local_id, fun)
    }
}

struct Collector<'a> {
    def_map: DefMap,

    root_file: FileId,
    db: &'a dyn HirDefDB,
    tree: &'a ItemTree,
}

impl Collector<'_> {
    // Verilog-ams standard does not specify any way to access user-defined discipline attributes
    // I am guessing this is an oversight but until this is clarified we are not adding this.
    // TODO talk to committee about discipline attributes

    // Identifiers defined in natures and disciplines are visible in all modules,
    // while each module creates its own scope.
    fn collect_root_map(mut self) -> Arc<DefMap> {
        let root_file = self.root_file;
        let root_scope = self.def_map.open_new_scope(ScopeOrigin::Root, None);
        debug_assert_eq!(root_scope, self.def_map.entry_scope());
        debug_assert_eq!(root_scope, self.def_map.root_scope());

        for item in &self.tree.top_level {
            match *item {
                RootItem::Nature(id) => {
                    let name = self.tree[id].name.clone();
                    let nature_def = NatureLoc { root_file, id }.intern(self.db);
                    self.insert_def(root_scope, name, nature_def);
                    if let Some((name, attr_id)) = self.tree[id].access.clone() {
                        // special treatment for nature access function
                        let attr_def =
                            NatureAttrLoc { nature: nature_def, id: attr_id }.intern(self.db);
                        let access_def = ScopeItemDef::NatureAccess(attr_def.into());
                        self.insert_def(root_scope, name, access_def)
                    }
                }
                RootItem::Discipline(id) => {
                    let name = self.tree[id].name.clone();
                    let def = DisciplineLoc { root_file, id }.intern(self.db);
                    self.insert_def(root_scope, name, def);
                }
                RootItem::Module(id) => self.collect_module(id, root_scope),
            }
        }
        Arc::new(self.def_map)
    }

    fn collect_module(&mut self, id: ItemTreeId<Module>, parent: LocalScopeId) {
        let module = &self.tree[id];
        let module_name = module.name.clone();

        // Each module opens up its own scope. Refer to LRM 6.8: Scope rules.
        let next_scope =
            Scope::from(self.root_file, self.def_map.src, self.def_map.scopes.next_key());
        let module_def = ModuleLoc { scope: next_scope, id }.intern(self.db);
        let scope = self.def_map.open_new_scope(ScopeOrigin::Module(module_def), Some(parent));
        self.insert_child_scope(scope, parent, module_name, module_def);

        // [LRM 6.3.6] Hierarchical system parameters.
        // In addition to the parameters explicitly declared in a module’s header,
        // there are six system parameters that are implicitly declared for every
        // module: $mfactor, $xposition, $yposition, $angle, $hflip, and $vflip.
        builtin::insert_param_sysfun(&mut self.def_map[scope].declarations);

        // collect module items
        use ModuleItem::*;
        for item in &module.items {
            match *item {
                Node(id) => {
                    let name = module.nodes[id].name.clone();
                    let def = NodeLoc { module: module_def, id }.intern(self.db);
                    self.insert_def(scope, name, def)
                }
                Branch(branch) => self.insert_item(scope, self.tree[branch].name.clone(), branch),
                Variable(var) => self.insert_item(scope, self.tree[var].name.clone(), var),
                Parameter(param) => self.insert_item(scope, self.tree[param].name.clone(), param),
                AliasParam(alias) => self.insert_item(scope, self.tree[alias].name.clone(), alias),
                Function(fun) => self.insert_item(scope, self.tree[fun].name.clone(), fun),
                Block(block) => self.collect_block(scope, block),
            }
        }
    }

    fn collect_block(&mut self, scope: LocalScopeId, block: AstId<ast::BlockStmt>) {
        let name =
            self.tree[block].name.clone().expect("ModuleItem should only contain named blocks");
        let parent = Scope::from(self.root_file, self.def_map.src, scope);
        let def = BlockLoc { parent, ast_id: block }.intern(self.db);
        self.insert_def(scope, name, def);
    }

    fn collect_block_map(mut self, block: BlockId, items: &[BlockItem]) -> Arc<DefMap> {
        let scope = self.def_map.open_new_scope(block.into(), None);
        debug_assert_eq!(scope, self.def_map.entry_scope());
        for item in items {
            match *item {
                BlockItem::Variable(var) => {
                    self.insert_item(scope, self.tree[var].name.clone(), var)
                }
                BlockItem::Parameter(param) => {
                    self.insert_item(scope, self.tree[param].name.clone(), param)
                }
                BlockItem::Block(block) => self.collect_block(scope, block),
            }
        }
        Arc::new(self.def_map)
    }

    fn collect_function_map(
        mut self,
        id: ItemTreeId<Function>,
        parent_module: LocalScopeId,
        func: FunctionId,
    ) -> Arc<DefMap> {
        debug_assert_eq!(self.def_map.src, DefMapSource::Function(func));

        let root_def_map = self.db.root_def_map(self.root_file);
        let scope = self.def_map.open_new_scope(ScopeOrigin::Function(func), Some(0u32.into()));
        assert_eq!(scope, self.def_map.entry_scope());
        self.def_map[scope]
            .declarations
            .insert(self.tree[id].name.clone(), ScopeItemDef::FunctionReturn(func));

        for item in &self.tree[id].items {
            match *item {
                FunctionItem::Block(ast) => self.collect_block(scope, ast),
                FunctionItem::Parameter(param) => {
                    self.insert_item(scope, self.tree[param].name.clone(), param)
                }
                FunctionItem::Variable(var) => {
                    self.insert_item(scope, self.tree[var].name.clone(), var)
                }
                FunctionItem::FunctionArg(arg) => {
                    let def = FunctionArgLoc { fun: func, id: arg }.intern(self.db);
                    self.insert_def(scope, self.tree[id].args[arg].name.clone(), def)
                }
            }
        }

        let root = self.def_map.open_new_scope(ScopeOrigin::Root, None);
        self.def_map.root_scope = root;
        debug_assert_eq!(self.def_map.root_scope(), root);

        // Copy the modules and their parameters since these are the only declarations outside
        // of the function itself that are accessible insdie an analog function
        let main_root_scope = &root_def_map.scopes[root_def_map.root_scope()];

        let mut parent_module_ = None;

        for (module_name, scope_id) in main_root_scope.children.iter() {
            let scope = &root_def_map[*scope_id];

            if let ScopeOrigin::Module(module) = scope.origin {
                let declarations = scope
                    .declarations
                    .iter()
                    .filter(|&(_, decl)| {
                        matches!(decl, ScopeItemDef::ParamId(_) | ScopeItemDef::FunctionId(_))
                    })
                    .map(|(name, decl)| (name.clone(), *decl))
                    .collect();

                debug_assert_eq!(scope.parent, Some(root_def_map.root_scope()));

                let scope = ScopeData {
                    origin: scope.origin,
                    parent: Some(root),
                    children: IndexMap::default(),
                    declarations,
                };

                debug_assert_eq!(scope.parent, Some(root));
                let scope = self.def_map.scopes.push_and_get_key(scope);

                if *scope_id == parent_module {
                    parent_module_ = Some(scope);
                }

                self.def_map.scopes[root].children.insert(module_name.clone(), scope);
                self.def_map.scopes[root].declarations.insert(module_name.clone(), module.into());
            }
        }

        assert!(parent_module_.is_some(), "parent module was not among the root modules");
        self.def_map[scope].parent = parent_module_;

        Arc::new(self.def_map)
    }

    fn insert_child_scope(
        &mut self,
        scope: LocalScopeId,
        parent: LocalScopeId,
        name: Name,
        def: impl Into<ScopeItemDef>,
    ) {
        self.insert_def(parent, name.clone(), def);
        self.def_map[parent].children.entry(name).or_insert(scope);
    }

    fn insert_item<N>(&mut self, dst: LocalScopeId, name: Name, item: ItemTreeId<N>)
    where
        N: ItemTreeNode,
        ItemLoc<N>: Intern,
        <ItemLoc<N> as Intern>::Id: Into<ScopeItemDef>,
    {
        let scope = Scope::from(self.root_file, self.def_map.src, dst);
        let def = ItemLoc { scope, id: item }.intern(self.db);
        self.insert_def(dst, name, def)
    }

    fn insert_def(&mut self, dst: LocalScopeId, name: Name, def: impl Into<ScopeItemDef>) {
        let def = def.into();
        if let Some(old) = self.def_map[dst].declarations.insert(name.clone(), def) {
            let diag = DefDiagnostic::AlreadyDeclared { old, new: def, name };
            self.def_map.diagnostics.push(diag);
        }
    }
}
