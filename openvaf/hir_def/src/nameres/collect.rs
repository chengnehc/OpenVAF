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
    Lookup, ModuleLoc, NatureAccess, NatureAttrLoc, NatureLoc, NodeLoc, ScopeId,
};

use super::{
    DefDiagnostic, DefMap, DefMapSource, LocalScopeId, RecScope, Scope, ScopeItem, ScopeOrigin,
};

impl DefMap {
    pub fn root_query(db: &dyn HirDefDB, root_file: FileId) -> Arc<DefMap> {
        let tree = &db.item_tree(root_file);
        // Fixed(JW): nature/discipline would not create scopes
        let scope_cnt = tree.data.modules.len();
        let def_map = DefMap {
            src: DefMapSource::Root,
            scopes: Arena::with_capacity(scope_cnt),
            root_scope: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        };

        Collector { def_map, root_file, db, tree }.collect_root_map()
    }

    pub fn block_query(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
        let BlockLoc { parent, ast_id } = block.lookup(db);
        let root_file = parent.root_file;
        let tree = &db.item_tree(root_file);
        let items = &tree.blocks[&ast_id].items;
        if items.is_empty() {
            // fast path for block scopes containing no items
            return None;
        }
        let def_map = DefMap {
            src: DefMapSource::Block(block),
            scopes: Arena::with_capacity(1), // the block scope itself
            root_scope: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        };

        Some(Collector { def_map, root_file, tree, db }.collect_block_map(block, items))
    }

    pub fn function_query(db: &dyn HirDefDB, fun: FunctionId) -> Arc<DefMap> {
        let FunctionLoc { scope, id } = fun.lookup(db);
        let ScopeId { root_file, id: parent_module, .. } = scope;
        let tree = &db.item_tree(root_file);
        let def_map = DefMap {
            src: DefMapSource::Function(fun),
            scopes: Arena::with_capacity(tree.data.modules.len() + 1), // one root scope + all module scopes
            root_scope: 0u32.into(), // This will be changed once the scope has been created
            diagnostics: Vec::new(),
        };

        Collector { def_map, root_file, db, tree }.collect_function_map(id, parent_module, fun)
    }
}

struct Collector<'a> {
    // output
    def_map: DefMap,
    // reads
    root_file: FileId,
    db: &'a dyn HirDefDB,
    tree: &'a ItemTree,
}

impl Collector<'_> {
    fn collect_root_map(mut self) -> Arc<DefMap> {
        let root_file = self.root_file;
        let root_scope = self.def_map.declare_scope(ScopeOrigin::Root, None);
        debug_assert_eq!(root_scope, self.def_map.entry_scope());
        debug_assert_eq!(root_scope, self.def_map.root_scope());

        // Identifiers defined in natures and disciplines are inserted in the root scope and as a
        // result they are visible to all modules. However, each module creates its own scope.
        // Refer to LRM 6.8: Scope rules.
        for item in &self.tree.root_items {
            match *item {
                RootItem::Nature(id) => {
                    let name = self.tree[id].name.clone();
                    let nature = NatureLoc { root_file, id }.intern(self.db);
                    self.insert_def(nature, root_scope, name);

                    if let Some((name, id)) = self.tree[id].access.clone() {
                        let attr = NatureAttrLoc { nature, id }.intern(self.db);
                        let access = ScopeItem::NatureAccess(NatureAccess(attr));
                        self.insert_def(access, root_scope, name)
                    }
                }
                // Verilog-ams standard does not specify any way to access user-defined discipline
                // attributes. I am guessing this is an oversight but until this is clarified we
                // are not adding this.
                // TODO talk to committee about discipline attributes
                RootItem::Discipline(id) => {
                    let name = self.tree[id].name.clone();
                    let discipline = DisciplineLoc { root_file, id }.intern(self.db);
                    self.insert_def(discipline, root_scope, name);
                }
                RootItem::Module(id) => self.collect_module(id, root_scope),
            }
        }

        Arc::new(self.def_map)
    }

    fn collect_module(&mut self, id: ItemTreeId<Module>, parent_scope: LocalScopeId) {
        let module = &self.tree[id];
        let module_name = module.name.clone();

        // create new scope
        let scope = ScopeId::from(self.root_file, self.def_map.src, self.def_map.scopes.next_key());
        let module_def = ModuleLoc { scope, id }.intern(self.db);
        let module_scope =
            self.def_map.declare_scope(ScopeOrigin::Module(module_def), Some(parent_scope));

        // insert module def into parent scope
        self.insert_def(module_def, parent_scope, module_name.clone());
        // update children scope for parent
        let Scope::Rec(root_scope) = &mut self.def_map[parent_scope] else { unreachable!() };
        root_scope.children.entry(module_name).or_insert(module_scope);

        // [LRM 6.3.6] Hierarchical system parameters.
        // In addition to the parameters explicitly declared in a module’s header,
        // there are six system parameters that are implicitly declared for every
        // module: $mfactor, $xposition, $yposition, $angle, $hflip, and $vflip.
        builtin::insert_param_sysfun(self.def_map[module_scope].decls_mut());

        for item in &module.items {
            match *item {
                ModuleItem::Node(id) => {
                    let name = module.nodes[id].name.clone();
                    let node = NodeLoc { module: module_def, id }.intern(self.db);
                    self.insert_def(node, module_scope, name)
                }
                ModuleItem::Branch(br) => {
                    self.intern_and_insert_item(br, module_scope, self.tree[br].name.clone())
                }
                ModuleItem::Variable(var) => {
                    self.intern_and_insert_item(var, module_scope, self.tree[var].name.clone());
                }
                ModuleItem::Parameter(param) => {
                    self.intern_and_insert_item(param, module_scope, self.tree[param].name.clone())
                }
                ModuleItem::AliasParam(alias) => {
                    self.intern_and_insert_item(alias, module_scope, self.tree[alias].name.clone())
                }
                ModuleItem::Function(fun) => {
                    self.intern_and_insert_item(fun, module_scope, self.tree[fun].name.clone());
                }
                ModuleItem::ScopedBlock(block) => {
                    self.intern_and_insert_scoped_block(block, module_scope);
                }
            }
        }
    }

    /// Collect the def map of a named block.
    fn collect_block_map(mut self, block: BlockId, items: &[BlockItem]) -> Arc<DefMap> {
        let block_scope = self.def_map.declare_scope(ScopeOrigin::Block(block), None);
        debug_assert_eq!(block_scope, self.def_map.entry_scope());
        for item in items {
            match *item {
                BlockItem::Variable(var) => {
                    self.intern_and_insert_item(var, block_scope, self.tree[var].name.clone())
                }
                BlockItem::Parameter(param) => {
                    self.intern_and_insert_item(param, block_scope, self.tree[param].name.clone())
                }
                BlockItem::ScopedBlock(block) => {
                    self.intern_and_insert_scoped_block(block, block_scope)
                }
            }
        }

        Arc::new(self.def_map)
    }

    fn collect_function_map(
        mut self,
        id: ItemTreeId<Function>,
        parent_module: LocalScopeId,
        fun_def: FunctionId,
    ) -> Arc<DefMap> {
        let fun = &self.tree[id];
        let fun_scope = self.def_map.declare_scope(ScopeOrigin::Function(fun_def), None);

        // [LRM: 4.7.2.1] The analog user-defined function definition implicitly
        // declares a variable internal to the analog user-defined function,
        // with the same name as the analog_function_identifier.
        self.def_map[fun_scope]
            .decls_mut()
            .insert(fun.name.clone(), ScopeItem::FunctionReturn(fun_def));

        for item in &fun.items {
            match *item {
                FunctionItem::FunctionArg(arg) => {
                    let def = FunctionArgLoc { fun: fun_def, id: arg }.intern(self.db);
                    self.insert_def(def, fun_scope, fun.args[arg].name.clone())
                }
                FunctionItem::Variable(var) => {
                    self.intern_and_insert_item(var, fun_scope, self.tree[var].name.clone())
                }
                FunctionItem::Parameter(param) => {
                    self.intern_and_insert_item(param, fun_scope, self.tree[param].name.clone())
                }
            }
        }

        // declare the root scope for the function def map
        let root = self.def_map.declare_scope(ScopeOrigin::Root, None);
        self.def_map.root_scope = root;

        // [LRM 4.7.1] Analog user-defined function:
        //
        // - shall only reference locally-defined variables, variables passed as arguments,
        //   locally-defined parameters and *module-level parameters*; and
        // - if a locally-defined parameter with the specified name does not exist, then
        //   the module-level parameter of the specified name will be used.
        //
        // Copy the modules and their parameters since these are the only declarations outside
        // of the function itself that are accessible inside an analog function
        let root_def_map = self.db.root_def_map(self.root_file);
        let main_root_scope = &root_def_map[root_def_map.root_scope()];

        for (name, scope_id) in main_root_scope.children().unwrap().iter() {
            let scope = &root_def_map[*scope_id];
            if let ScopeOrigin::Module(module) = scope.origin() {
                let declarations = scope
                    .decls()
                    .iter()
                    .filter_map(|(name, decl)| {
                        matches!(decl, ScopeItem::ParamId(_) | ScopeItem::FunctionId(_))
                            .then_some((name.clone(), *decl))
                    })
                    .collect();
                let id = self.def_map.scopes.push_and_get_key(
                    RecScope {
                        origin: scope.origin(),
                        parent: Some(root),
                        children: IndexMap::default(),
                        declarations,
                    }
                    .into(),
                );
                // update the parent module scope id when we've met it
                if parent_module == *scope_id {
                    let Scope::Flat(scope) = &mut self.def_map[fun_scope] else { unreachable!() };
                    scope.parent = Some(id);
                    // JW: add function scope to parent module scope's children
                    self.def_map[id].children_mut().unwrap().insert(fun.name.clone(), fun_scope);
                }
                self.def_map[root].children_mut().unwrap().insert(name.clone(), id);
                self.def_map[root].decls_mut().insert(name.clone(), module.into());
            }
        }
        assert!(
            self.def_map[fun_scope].parent().is_some(),
            "parent module was not among the root modules"
        );

        Arc::new(self.def_map)
    }

    /// Intern a named block and insert its definition into the given parent scope.
    fn intern_and_insert_scoped_block(
        &mut self,
        block: AstId<ast::BlockStmt>,
        parent_scope: LocalScopeId,
    ) {
        let name = self.tree.blocks[&block]
            .name
            .clone()
            .expect("should only create DefMap for named blocks");
        let parent = ScopeId::from(self.root_file, self.def_map.src, parent_scope);
        let block = BlockLoc { parent, ast_id: block }.intern(self.db);
        self.insert_def(block, parent_scope, name);
    }

    fn intern_and_insert_item<N>(&mut self, item: ItemTreeId<N>, dst: LocalScopeId, name: Name)
    where
        N: ItemTreeNode,
        ItemLoc<N>: Intern,
        <ItemLoc<N> as Intern>::Id: Into<ScopeItem>,
    {
        let scope = ScopeId::from(self.root_file, self.def_map.src, dst);
        let def = ItemLoc { scope, id: item }.intern(self.db);
        self.insert_def(def, dst, name)
    }

    fn insert_def(&mut self, def: impl Into<ScopeItem>, dst: LocalScopeId, name: Name) {
        let def = def.into();
        if let Some(old) = self.def_map[dst].decls_mut().insert(name.clone(), def) {
            let diag = DefDiagnostic::AlreadyDeclared { old, new: def, name };
            self.def_map.diagnostics.push(diag);
        }
    }
}
