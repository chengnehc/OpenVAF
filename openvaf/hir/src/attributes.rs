use std::sync::Arc;

use basedb::{AstId, AstIdMap, BaseDB, FileId};
use syntax::{ast, AstNode, SourceFile};

use crate::CompilationDB;

pub struct AstCache {
    ast: SourceFile,
    map: Arc<AstIdMap>,
}

impl AstCache {
    pub(crate) fn new(db: &CompilationDB, root_file: FileId) -> AstCache {
        AstCache { ast: db.parse(root_file).tree(), map: db.ast_id_map(root_file) }
    }

    /// Tries to resolve an attribute as a string if it exists.
    ///
    /// Emits an error to `sink` if the attribute exists but is not a string literal.
    ///
    /// Returns The (unescaped) string literal assigned to `attribute`. If `attribute`
    /// doesn't exist or it is not a string literal, `None` is returned.
    pub(crate) fn resolve_attr<N: AstNode>(&self, name: &str, id: AstId<N>) -> Option<ast::Attr> {
        let idx = self.map.get_attr(id, name)?;
        let node = self.map.get(id).to_node(self.ast.syntax());
        let node = node.syntax();
        let mut attrs = if ast::Var::can_cast(node.kind()) || ast::Param::can_cast(node.kind()) {
            ast::attrs(&node.parent().unwrap())
        } else {
            ast::attrs(node)
        };
        attrs.nth(idx)
    }
}
