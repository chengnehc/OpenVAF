use std::sync::Arc;

use arena::Arena;
use basedb::{AstId, FileId};
use indexmap::IndexMap;
use syntax::ast;
use syntax::name::Name;

use crate::builtin::insert_module_builtin_scope;
use crate::db::HirDefDB;
use crate::item_tree::{
    BlockItem, Function, FunctionItem, ItemTree, ItemTreeId, ItemTreeNode, Module, ModuleItem,
    RootItem,
};
use crate::{
    BlockId, BlockLoc, DisciplineLoc, FunctionArgLoc, FunctionId, FunctionLoc, Intern, ItemLoc,
    Lookup, ModuleLoc, NatureAttrLoc, NatureLoc, NodeLoc, ScopeId,
};

use super::diagnostics::DefDiagnostic;
use super::{DefMap, DefMapSource, LocalScopeId, ScopeData, ScopeOrigin, ScopeItemDef};

// Root items can only be `discipline`, `nature` or `module`.
pub fn collect_root_def_map(db: &dyn HirDefDB, root_file: FileId) -> Arc<DefMap> {
    let tree = &db.item_tree(root_file);
    let scope_cnt = tree.data.natures.len() + tree.data.disciplines.len() + tree.data.modules.len();
    let mut collector = DefCollector {
        def_map: DefMap {
            src: DefMapSource::Root,
            scopes: Arena::with_capacity(scope_cnt),
            root_id: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        },
        tree,
        db,
        root_file,
    };
    collector.collect_root_map();

    Arc::new(collector.def_map)
}

pub fn collect_function_map(db: &dyn HirDefDB, function: FunctionId) -> Arc<DefMap> {
    let FunctionLoc { scope: ScopeId { root_file, local_id: local_scope, .. }, id } =
        function.lookup(db);
    let tree = &db.item_tree(root_file);
    let mut collector = DefCollector {
        def_map: DefMap {
            src: DefMapSource::Function(function),
            scopes: Arena::with_capacity(tree.data.modules.len() + 1),
            root_id: LocalScopeId::from(0u32), // This will be changed once the scope has been created
            diagnostics: Vec::new(),
        },
        tree,
        db,
        root_file,
    };
    collector.collect_function_map(id, local_scope, function);

    Arc::new(collector.def_map)
}

pub fn collect_block_map(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
    // Note: `BlockLoc`s are only created for named blocks.
    let BlockLoc { parent, ast_id } = block.lookup(db);
    let tree = &db.item_tree(parent.root_file);
    let items = &tree[ast_id].block_items;
    if items.is_empty() {
        return None;
    }
    let mut collector = DefCollector {
        def_map: DefMap {
            src: DefMapSource::Block(block),
            scopes: Arena::with_capacity(1),
            root_id: LocalScopeId::from(0u32),
            diagnostics: Vec::new(),
        },
        tree,
        db,
        root_file: parent.root_file,
    };
    collector.collect_block_map(block, items);

    Some(Arc::new(collector.def_map))
}

struct DefCollector<'a> {
    root_file: FileId,
    db: &'a dyn HirDefDB,
    tree: &'a ItemTree,
    def_map: DefMap,
}

impl DefCollector<'_> {
    fn collect_function_map(
        &mut self,
        item_tree: ItemTreeId<Function>,
        parent_module: LocalScopeId,
        func: FunctionId,
    ) {
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
        self.def_map.root_id = root;
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
                    .filter_map(|(name, decl)| {
                        matches!(decl, ScopeItemDef::ParamId(_) | ScopeItemDef::FunctionId(_))
                            .then(|| (name.clone(), *decl))
                    })
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
    }

    fn collect_block_map(&mut self, id: BlockId, items: &[BlockItem]) {
        let scope = self.def_map.new_root_scope(id.into());
        debug_assert_eq!(scope, self.def_map.entry_scope());
        for item in items {
            match *item {
                BlockItem::Block(ast) => {
                    if let Some(name) = &self.tree[ast].name {
                        let loc = BlockLoc {
                            ast_id: ast,
                            parent: ScopeId {
                                root_file: self.root_file,
                                local_id: scope,
                                src: self.def_map.src,
                            },
                        };
                        let id = loc.intern(self.db);
                        self.insert_decl(scope, name.clone(), id);
                    } else {
                        debug_assert!(false)
                    }
                }
                BlockItem::Parameter(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
                BlockItem::Variable(id) => {
                    self.insert_item_decl(scope, self.tree[id].name.clone(), id)
                }
            }
        }
    }

    // Verilog-ams standard does not specify any way to access user-defined discipline attributes
    // I am guessing this is an oversight but until this is clarified we are not adding this
    // TODO talk to committee about discipline attributes

    fn collect_root_map(&mut self) {
        let root_scope = self.def_map.new_root_scope(ScopeOrigin::Root);

        debug_assert_eq!(root_scope, self.def_map.entry_scope());
        debug_assert_eq!(root_scope, self.def_map.root_scope());

        for item in &self.tree.top_level {
            match *item {
                RootItem::Module(module) => self.collect_module(module, root_scope),
                RootItem::Nature(nature) => {
                    let id = NatureLoc { root_file: self.root_file, id: nature }.intern(self.db);
                    self.insert_decl(root_scope, self.tree[nature].name.clone(), id);
                    if let Some((name, attr)) = self.tree[nature].access.clone() {
                        self.insert_decl(
                            root_scope,
                            name,
                            ScopeItemDef::NatureAccess(
                                NatureAttrLoc { nature: id, id: attr }.intern(self.db).into(),
                            ),
                        )
                    }
                }
                RootItem::Discipline(discipline) => {
                    self.insert_decl(
                        root_scope,
                        self.tree[discipline].name.clone(),
                        DisciplineLoc { root_file: self.root_file, id: discipline }.intern(self.db),
                    );
                }
            }
        }
    }

    fn collect_module(&mut self, id: ItemTreeId<Module>, parent: LocalScopeId) {
        let module = &self.tree[id];
        let module_name = module.name.clone();
        let module_id = ModuleLoc { scope: self.next_scope(), id }.intern(self.db);
        let scope = self.def_map.new_scope(ScopeOrigin::Module(module_id), parent);

        self.insert_scope(scope, parent, module_name, module_id);
        insert_module_builtin_scope(&mut self.def_map[scope].declarations);

        for item in &module.items {
            match *item {
                ModuleItem::Node(local_id) => {
                    let loc = NodeLoc { module: module_id, id: local_id };
                    let id = loc.intern(self.db);
                    self.insert_decl(scope, module.nodes[local_id].name.clone(), id)
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

    fn next_scope(&self) -> ScopeId {
        ScopeId {
            root_file: self.root_file,
            src: self.def_map.src,
            local_id: self.def_map.scopes.next_key(),
        }
    }

    fn collect_block(&mut self, scope: LocalScopeId, block: AstId<ast::BlockStmt>) {
        let parent = ScopeId { root_file: self.root_file, local_id: scope, src: self.def_map.src };
        let decl = BlockLoc { ast_id: block, parent }.intern(self.db);
        let name = self.tree[block].name.clone().expect("Item tree must only contain named blocks");
        self.insert_decl(scope, name, decl);
    }

    fn insert_scope(
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
        <ItemLoc<N> as Intern>::ID: Into<ScopeItemDef>,
    {
        let scope = ScopeId { root_file: self.root_file, src: self.def_map.src, local_id: dst };
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
