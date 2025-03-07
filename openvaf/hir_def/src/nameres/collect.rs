//! Collect DefMaps for the root scope (global scope) and items that define new scopes.
//!
//! In terms of Verilog-A subset, according to the scope rules given by LRM 6.8,
//! the following elements define a new scope:
//! — modules
//! — named blocks
//! — analog functions
//!
//! An identifier shall be used to declare only one item within a scope. It is illegal to declare
//! two or more variables which have the same name, or to name a task the same as a variable within
//! the same module, or to give an instance the same name as the name of the net connected to its output.
//!
//! If an identifier is referenced directly (without a hierarchical path) within a named block, it
//! shall be declared within the named block locally or within a module, or within a named block that
//! is higher in the same branch of the name tree containing the named block. If it is declared locally,
//! the local item shall be used; if not, the search shall continue upward until an item by that name is
//! found or until a module boundary is encountered. If the item is a variable, it shall stop at a module
//! boundary; if the item is a named block, it continues to search higher level modules until found.

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

use super::diagnostics::DefDiagnostic;
use super::{DefMap, DefMapSource, LocalScopeId, ScopeData, ScopeItemDef, ScopeOrigin};

impl DefMap {
    pub fn root_def_map_query(db: &dyn HirDefDB, root_file: FileId) -> Arc<DefMap> {
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

    pub fn block_def_map_query(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
        // Note: `BlockLoc`s are only created for named blocks.
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

    pub fn function_def_map_query(db: &dyn HirDefDB, fun: FunctionId) -> Arc<DefMap> {
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
        let root_scope = self.def_map.new_root_scope(ScopeOrigin::Root);
        debug_assert_eq!(root_scope, self.def_map.entry_scope());
        debug_assert_eq!(root_scope, self.def_map.root_scope());

        for item in &self.tree.top_level {
            match *item {
                RootItem::Module(item) => self.collect_module(item, root_scope),
                RootItem::Discipline(item) => {
                    let name = self.tree[item].name.clone();
                    let disc_id = DisciplineLoc { root_file, id: item }.intern(self.db);
                    self.insert_decl(root_scope, name, disc_id);
                }
                RootItem::Nature(item) => {
                    // Intern and insert nature definition
                    let nature_id = NatureLoc { root_file, id: item }.intern(self.db);
                    self.insert_decl(root_scope, self.tree[item].name.clone(), nature_id);
                    // Intern and insert nature access attribute as well
                    if let Some((name, local_id)) = self.tree[item].access.clone() {
                        let attr_id =
                            NatureAttrLoc { nature: nature_id, id: local_id }.intern(self.db);
                        let access_id = ScopeItemDef::NatureAccess(attr_id.into());
                        self.insert_decl(root_scope, name, access_id)
                    }
                }
            }
        }
        Arc::new(self.def_map)
    }

    fn collect_module(&mut self, id: ItemTreeId<Module>, parent: LocalScopeId) {
        let module = &self.tree[id];
        let module_name = module.name.clone();
        let module_id = ModuleLoc { scope: self.next_scope(), id }.intern(self.db);

        // Each module opens up its own scope. Refer to LRM 6.8 Scope rules.
        let scope = self.def_map.new_scope(ScopeOrigin::Module(module_id), parent);
        self.insert_child_scope(scope, parent, module_name, module_id);

        // Refer to LRM 6.3.6 Hierarchical system parameters
        // In addition to the parameters explicitly declared in a module’s header,
        // there are six system parameters that are implicitly declared for every
        // module: $mfactor, $xposition, $yposition, $angle, $hflip, and $vflip.
        builtin::insert_param_sysfun(&mut self.def_map[scope].declarations);

        for item in &module.items {
            match *item {
                ModuleItem::Node(id) => {
                    let decl = NodeLoc { module: module_id, id }.intern(self.db);
                    self.insert_decl(scope, module.nodes[id].name.clone(), decl)
                }
                ModuleItem::Branch(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                ModuleItem::Parameter(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                ModuleItem::AliasParam(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                ModuleItem::Variable(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                ModuleItem::Function(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                ModuleItem::Block(id) => self.collect_block(scope, id),
            }
        }
    }

    fn next_scope(&self) -> Scope {
        Scope {
            root_file: self.root_file,
            src: self.def_map.src,
            local_id: self.def_map.scopes.next_key(),
        }
    }

    fn collect_block(&mut self, scope: LocalScopeId, block: AstId<ast::BlockStmt>) {
        let parent = Scope { root_file: self.root_file, local_id: scope, src: self.def_map.src };
        let decl = BlockLoc { parent, ast_id: block }.intern(self.db);
        let name = self.tree[block].name.clone().expect("Item tree must only contain named blocks");
        self.insert_decl(scope, name, decl);
    }

    fn collect_block_map(mut self, block: BlockId, items: &[BlockItem]) -> Arc<DefMap> {
        let scope = self.def_map.new_root_scope(block.into());
        debug_assert_eq!(scope, self.def_map.entry_scope());
        for item in items {
            match *item {
                BlockItem::Parameter(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                BlockItem::Variable(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                BlockItem::Block(ast_id) => {
                    if let Some(name) = &self.tree[ast_id].name {
                        let parent = Scope {
                            root_file: self.root_file,
                            local_id: scope,
                            src: self.def_map.src,
                        };
                        let id = BlockLoc { ast_id, parent }.intern(self.db);
                        self.insert_decl(scope, name.clone(), id);
                    } else {
                        panic!("Item tree must only contain named blocks")
                    }
                }
            }
        }
        Arc::new(self.def_map)
    }

    fn collect_function_map(
        mut self,
        item_tree: ItemTreeId<Function>,
        parent_module: LocalScopeId,
        func: FunctionId,
    ) -> Arc<DefMap> {
        debug_assert_eq!(self.def_map.src, DefMapSource::Function(func));

        let root_def_map = self.db.root_def_map(self.root_file);

        // parent is a placeholder here...
        let scope = self.def_map.new_scope(ScopeOrigin::Function(func), LocalScopeId::from(0u32));
        assert_eq!(scope, self.def_map.entry_scope());

        self.def_map[scope]
            .declarations
            .insert(self.tree[item_tree].name.clone(), ScopeItemDef::FunctionReturn(func));

        for item in &self.tree[item_tree].items {
            match *item {
                FunctionItem::Block(ast) => self.collect_block(scope, ast),
                FunctionItem::Parameter(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                FunctionItem::Variable(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                FunctionItem::FunctionArg(arg) => {
                    let id = FunctionArgLoc { fun: func, id: arg }.intern(self.db);
                    self.insert_decl(scope, self.tree[item_tree].args[arg].name.clone(), id)
                }
            }
        }

        let root = self.def_map.new_root_scope(ScopeOrigin::Root);
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
        decl: impl Into<ScopeItemDef>,
    ) {
        self.insert_decl(parent, name.clone(), decl);
        self.def_map[parent].children.entry(name).or_insert(scope);
    }

    fn insert_item_decl<N>(&mut self, dst: LocalScopeId, name: Name, id: ItemTreeId<N>)
    where
        N: ItemTreeNode,
        ItemLoc<N>: Intern,
        <ItemLoc<N> as Intern>::Id: Into<ScopeItemDef>,
    {
        let scope = Scope::new(self.root_file, self.def_map.src, dst);
        let decl = ItemLoc { scope, id }.intern(self.db);
        self.insert_decl(dst, name, decl)
    }

    fn insert_decl(&mut self, dst: LocalScopeId, name: Name, decl: impl Into<ScopeItemDef>) {
        let decl = decl.into();
        if let Some(old) = self.def_map[dst].declarations.insert(name.clone(), decl) {
            let diag = DefDiagnostic::AlreadyDeclared { old, new: decl, name };
            self.def_map.diagnostics.push(diag);
        }
    }
}
