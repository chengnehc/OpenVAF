//! A `Path` is a list of hierarchial names separated by `.`
//!
//! See: LRM chapter 6.7

use stdx::{impl_debug, pretty};
use syntax::ast::{self, PathSegmentKind};
use syntax::name::{AsIdent, AsName, Name};

#[derive(PartialEq, Eq, Clone, Hash)]
pub struct Path {
    pub is_root: bool,
    pub segments: Vec<Name>,
}

impl Path {
    pub fn from_ident(ident: Name) -> Path {
        Path { is_root: false, segments: vec![ident] }
    }

    /// Resolve an `ast::Path` into a `hir::Path` recursively.
    pub fn resolve(syntax: ast::Path) -> Option<Path> {
        let prefix = if let Some(qual) = syntax.qualifier() { Path::resolve(qual) } else { None };
        let segment = syntax.segment()?;

        match (prefix, segment.kind) {
            (Some(_), PathSegmentKind::Root) => None, // error: `$root` is not the first segment
            (Some(mut prefix), PathSegmentKind::Name) => {
                prefix.segments.push(segment.as_name());
                Some(prefix)
            }
            (None, PathSegmentKind::Root) => Some(Path { is_root: true, segments: vec![] }),
            (None, PathSegmentKind::Name) => {
                Some(Path { is_root: false, segments: vec![segment.as_name()] })
            }
        }
    }
}

impl AsIdent for Path {
    fn as_ident(&self) -> Option<Name> {
        match self.segments.as_slice() {
            [name] if !self.is_root => Some(name.clone()),
            _ => None,
        }
    }
}

impl_debug!(match Path{
    Path{is_root: false, segments} => "{}", pretty::List::path(segments.as_slice());
    Path{is_root: true, segments} => "$root.{}", pretty::List::path(segments.as_slice());
});
