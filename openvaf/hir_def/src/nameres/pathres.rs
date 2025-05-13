use stdx::impl_display;
use syntax::name::kw;

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPath {
    FlowAttr { branch: BranchId, name: Name },
    PotentialAttr { branch: BranchId, name: Name },
    ScopeItemDef(ScopeItemDef),
}
impl_from!(ScopeItemDef for ResolvedPath);
impl_display! {
    match ResolvedPath{
        ResolvedPath::FlowAttr{..}  => "nature attribute";
        ResolvedPath::PotentialAttr{..} => "nature attribute";
        ResolvedPath::ScopeItemDef(item) => "{}", item.item_kind();
    }
}

impl DefMap {
    pub fn resolve_normal_item_path<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_normal_path(scope, segments, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(item) = res {
            if let Ok(res) = item.try_into() {
                return Ok(res);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: segments.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    /// # Scoping Rules
    ///
    /// ## RootItem: [LRM 6.8]
    /// An identifier shall be used to declare only one item within a scope. It is illegal to declare
    /// two or more variables which have the same name, or to name a task the same as a variable within
    /// the same module, or to give an instance the same name as the name of the net connected to its output.
    ///
    /// If an identifier is referenced directly (without a hierarchical path) within a named block, it
    /// shall be declared within the named block locally or within a module, or within a named block that
    /// is higher in the same branch of the name tree containing the named block. If it is declared locally,
    /// the local item shall be used; if not, the search shall continue upward until an item by that name is
    /// found or until a module boundary is encountered. If the item is a variable, it shall stop at a module
    /// boundary; if the item is a named block, it continues to search higher level modules until found.
    ///
    /// ## FunctionItem: [LRM 4.7.1]
    /// * shall only reference locally-defined variables, variables passed as arguments, locally-defined
    ///   parameters and module-level parameters
    /// * if a locally-defined parameter with the specified name does not exist, then the module-level
    ///   parameter of the specified name will be used.
    ///
    /// ## BlockItem: [LRM 5.3.2]
    /// The naming of a block allows *local* variables to be declared for that block.
    /// * All named block variables are static -- that is, an unique location exists for all variables and
    ///   leaving or entering the block do not affect the values stored in them.
    /// * All identifiers declared within a named block can be *accessed* outside the scope in which they are
    ///   declared.
    /// * Named block variables cannot be *assigned* outside the scope of the block in which they are declared.
    /// * Parameters declared within a named block have local scope and cannot be assigned outside the scope.
    pub fn resolve_normal_path(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let mut cur_scope = scope;
        let mut def_map = self;
        let mut arc;

        // resolve the first segment
        let name = segments.first().unwrap();
        let def = loop {
            // try resolving in current scope
            if let Some(def) = def_map[cur_scope].declarations.get(name) {
                break *def;
            }
            // try resolving in parent scopes
            match def_map[cur_scope].parent {
                Some(parent) => cur_scope = parent,
                None => match def_map.src {
                    // for named blocks, the parent scope is stored in `BlockLoc`
                    // rather than the def map
                    DefMapSource::Block(block) => {
                        // switch to the def map of block's parent
                        let parent = block.lookup(db).parent;
                        cur_scope = parent.id;
                        arc = parent.def_map(db);
                        def_map = &arc;
                    }
                    DefMapSource::Root | DefMapSource::Function(_) => {
                        // TODO when dealing with function def map, give hint if a builtin decl
                        // is found in root def map. So far, these two are identical.
                        let Some(builtin) = BUILTIN_ITEM_DEF.get(name) else {
                            return Err(PathResolveError::NotFound { name: name.clone() });
                        };
                        break *builtin;
                    }
                },
            }
        };
        // If there is only one path segment, then resolution is done.
        if segments.len() == 1 {
            return Ok(def.into());
        }
        // Now we have multiple path segments, inspect the definition kind of the first segment
        // to visit the scope corresponding to it, so that we could resolve other segments
        // within this scope.
        cur_scope = match def {
            ScopeItemDef::ModuleId(module) => module.lookup(db).scope.id,
            ScopeItemDef::BlockId(block) => {
                let Some(block_map) = db.block_def_map(block) else {
                    let name = segments[1].clone();
                    let scope = segments[0].clone();
                    return Err(PathResolveError::NotFoundIn { name, scope });
                };
                arc = block_map;
                def_map = &arc;
                def_map.entry_scope()
            }
            _ => return Err(PathResolveError::ExpectedScope { name: name.clone(), found: def }),
        };

        def_map.resolve_path(cur_scope, name, &segments[1..], db)
    }

    pub fn resolve_root_item_path<T: ScopeItemKind>(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_root_path(segments, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(def) = res {
            if let Ok(item) = def.try_into() {
                return Ok(item);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: segments.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    /// Resolve a path with `$root` prefix. [LRM 6.2.1]
    pub fn resolve_root_path(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        // JW: paths in named block should be resolved in the root def map
        // paths in analog function should be resolved in the function's own def map
        // even if $root is given
        debug_assert!(matches!(self.src, DefMapSource::Root | DefMapSource::Function(_)));
        self.resolve_path(self.root_scope(), &kw::root, segments, db)
    }

    /// Main routine to search for identifiers with hierarchical paths.
    fn resolve_path<'a>(
        &self,
        scope_id: LocalScopeId,
        scope_name: &'a Name,
        segments: &'a [Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let [segments @ .., name] = segments else { unreachable!() };
        let mut scope = scope_id;
        let mut scope_name = scope_name;
        let mut arc;
        let mut def_map = self;

        for (i, seg) in segments.iter().enumerate() {
            match def_map[scope].children.get(seg) {
                Some(child) => scope = *child,
                None => match def_map[scope].declarations.get(seg) {
                    Some(ScopeItemDef::BlockId(block)) => {
                        if let Some(block_map) = db.block_def_map(*block) {
                            arc = block_map;
                            def_map = &arc;
                            scope = def_map.entry_scope();
                        } else {
                            return Err(PathResolveError::NotFoundIn {
                                name: segments[i + 1].clone(),
                                scope: seg.clone(),
                            });
                        };
                    }
                    // Refer to LRM: 3.13.4
                    Some(ScopeItemDef::BranchId(branch))
                        if segments.get(i + 1) == Some(&kw::potential) =>
                    {
                        let rem = &segments[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::PotentialAttr {
                                branch: *branch,
                                name: name.clone(),
                            });
                        } else {
                            return Err(PathResolveError::ExpectedNatureAttrIdent {
                                found: rem.to_owned().into_boxed_slice(),
                            });
                        }
                    }
                    Some(ScopeItemDef::BranchId(branch))
                        if segments.get(i + 1) == Some(&kw::flow) =>
                    {
                        let rem = &segments[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::FlowAttr {
                                branch: *branch,
                                name: name.clone(),
                            });
                        } else {
                            return Err(PathResolveError::ExpectedNatureAttrIdent {
                                found: rem.to_owned().into_boxed_slice(),
                            });
                        }
                    }
                    Some(found) => {
                        return Err(PathResolveError::ExpectedScope {
                            name: seg.clone(),
                            found: *found,
                        });
                    }
                    None => {
                        return Err(PathResolveError::NotFoundIn {
                            name: seg.clone(),
                            scope: scope_name.clone(),
                        })
                    }
                },
            }
            scope_name = seg;
        }

        match self[scope].declarations.get(name) {
            Some(decl) => Ok(ResolvedPath::ScopeItemDef(*decl)),
            None => {
                Err(PathResolveError::NotFoundIn { name: name.clone(), scope: scope_name.clone() })
            }
        }
    }
}
