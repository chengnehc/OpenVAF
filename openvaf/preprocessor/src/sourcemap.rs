use std::convert::{TryFrom, TryInto};
use std::sync::Arc;
use stdx::{impl_debug_display, impl_idx_from};

use ahash::AHashMap;
use text_size::{TextRange, TextSize};
use typed_index_collections::TiVec;
use vfs::{FileId, VfsPath};

use crate::SourceProvider;

/// Represents a (global) continuous range of Text inside a particular file.
///
/// This representation is the only one that can be used to actually
/// obtain the source code of a particular TextRange.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct FileSpan {
    pub range: TextRange,
    pub file: FileId,
}

impl FileSpan {
    #[must_use]
    pub fn with_file(self, file: FileId) -> FileSpan {
        FileSpan { range: self.range, file }
    }

    #[must_use]
    pub fn with_range(self, range: TextRange) -> FileSpan {
        FileSpan { range, file: self.file }
    }

    #[must_use]
    pub fn with_len(self, len: TextSize) -> FileSpan {
        FileSpan { range: TextRange::at(self.range.start(), len), file: self.file }
    }

    #[must_use]
    pub fn with_subrange(self, relative_range: TextRange) -> FileSpan {
        let range = relative_range + self.range.start();
        assert!(
            range.end() <= self.range.end(),
            "subrange {:?} -> {:?} must fit into the total range {:?}",
            relative_range,
            range,
            self.range
        );
        FileSpan { range, file: self.file }
    }

    /* Below is not used. */
    #[inline]
    #[must_use]
    pub fn path(&self, sources: &dyn SourceProvider) -> VfsPath {
        sources.file_path(self.file)
    }

    #[inline]
    #[must_use]
    pub fn file_text(&self, db: &dyn SourceProvider) -> Arc<str> {
        db.file_text(self.file).expect("FileSpan was created with invalid FileId")
        // Spans are only created after a file was read successfully
    }

    pub fn extend_to_line_end(&mut self, db: &dyn SourceProvider) -> TextRange {
        let src = self.file_text(db);

        let range = self.range;
        // Map or is used because the start of slice is inclusive but we dont want to include the newline
        let line_start =
            src[..range.start().into()].rfind('\n').map_or(0, |pos| pos + 1).try_into().unwrap();
        let line_end = src[range.end().into()..]
            .find('\n')
            .map_or(TextSize::of(&*src), |pos| TextSize::try_from(pos).unwrap() + range.end());

        self.range = TextRange::new(line_start, line_end);
        range - line_start
    }
}

/// Represents a (local) continous range of text within a `SourceContext`
/// (macro expansion or included file).
///
/// The range is relative to the start of the particular `SourceContext`.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct CtxSpan {
    pub range: TextRange,
    pub ctx: SourceContextId,
}

impl CtxSpan {
    #[inline]
    pub fn dummy() -> CtxSpan {
        Self { range: TextRange::empty(0.into()), ctx: SourceContextId::ROOT }
    }

    #[must_use]
    pub fn with_ctx(self, ctx: SourceContextId) -> CtxSpan {
        CtxSpan { range: self.range, ctx }
    }

    #[must_use]
    pub fn with_range(self, range: TextRange) -> CtxSpan {
        CtxSpan { range, ctx: self.ctx }
    }

    #[must_use]
    pub fn with_len(self, len: TextSize) -> CtxSpan {
        CtxSpan { range: TextRange::at(self.range.start(), len), ctx: self.ctx }
    }

    #[must_use]
    pub fn cover(self, range: TextRange) -> CtxSpan {
        CtxSpan { range: self.range.cover(range), ctx: self.ctx }
    }

    #[must_use]
    pub fn to_file_span(self, sm: &SourceMap) -> FileSpan {
        sm.contexts[self.ctx].decl.with_subrange(self.range)
    }

    /// Extends `self` to reach to also include `other`
    ///
    /// # Note
    /// this is a non-trivial algorithm and should be called conservatively
    #[must_use]
    pub fn extend(self, other: CtxSpan, sm: &SourceMap) -> CtxSpan {
        if self.ctx == other.ctx {
            Self { range: self.range.cover(other.range), ctx: self.ctx }
        } else {
            let (ctx, range1, range2) = sm.lowest_common_parent(self, other);

            Self { range: range1.cover(range2), ctx }
        }
    }

    // JW: not used.
    #[must_use]
    pub fn lookup_expansion(&self, sm: &SourceMap) -> Vec<FileSpan> {
        sm.lookup_expansion(*self)
    }
}

/// Mapping from source context id to the original source code context.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct SourceMap {
    contexts: TiVec<SourceContextId, SourceContext>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct SourceContextId(u32);
impl_idx_from!(SourceContextId(u32));
impl_debug_display!(match SourceContextId {SourceContextId(id) => "ctx{id}";});

impl SourceContextId {
    pub const ROOT: Self = SourceContextId(0);
}

// TODO find rowan like solution to make this cheap to update
// (currently requires full preprocessor and parser rerun)

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub struct SourceContext {
    pub decl: FileSpan,
    pub call_site: Option<CtxSpan>,
}

impl SourceMap {
    pub(crate) fn new(root_file: FileId, root_file_len: TextSize) -> SourceMap {
        SourceMap {
            contexts: vec![SourceContext {
                decl: FileSpan { range: TextRange::up_to(root_file_len), file: root_file },
                call_site: None,
            }]
            .into(),
        }
    }

    pub(crate) fn add_ctx(&mut self, decl: FileSpan, call_site: CtxSpan) -> SourceContextId {
        self.contexts.push_and_get_key(SourceContext { decl, call_site: Some(call_site) })
    }

    pub fn to_file_spans(&self, spans: &mut [CtxSpan]) -> (FileId, Vec<TextRange>) {
        let ctx = self.to_same_ctx(spans);
        let decl = self.ctx_data(ctx).decl;
        let ranges = spans.iter_mut().map(|span| decl.with_subrange(span.range).range).collect();

        (decl.file, ranges)
    }

    pub fn to_same_ctx(&self, spans: &mut [CtxSpan]) -> SourceContextId {
        let mut merge = (0, spans[0]);
        for i in 1..spans.len() {
            if spans[i].ctx != merge.1.ctx {
                let (ctx, range1, range2) = self.lowest_common_parent(spans[i], spans[i - 1]);
                spans[i] = CtxSpan { ctx, range: range1 };
                if ctx != merge.1.ctx {
                    merge = (i, CtxSpan { ctx, range: range2 });
                }
            }
        }
        spans[..merge.0].fill(merge.1);
        merge.1.ctx
    }

    pub fn ctx_data(&self, id: SourceContextId) -> &SourceContext {
        &self.contexts[id]
    }

    #[must_use]
    pub fn lookup_expansion(&self, span: CtxSpan) -> Vec<FileSpan> {
        self.lookup_expansion_until(span, SourceContextId::ROOT)
    }

    #[must_use]
    pub fn lookup_expansion_until(&self, span: CtxSpan, until: SourceContextId) -> Vec<FileSpan> {
        let mut res = Vec::with_capacity(8);
        let mut current = span;

        loop {
            res.push(current.to_file_span(self));
            if current.ctx == until {
                return res;
            }
            match self.contexts[current.ctx].call_site {
                Some(call_site) => current = call_site,
                None => return res,
            }
        }
    }

    /// Finds the smallest `SourceContext` that contains both `self` and `other`
    /// # Note
    /// This is a fairly efficient algorithm but still requires a building a HashMap
    /// so avoid calling this if not necessary.
    ///
    /// Furthermore this algorithm does not special case self==other
    /// This is a special case that tends to be of interest to the caller and and as such
    /// is checked there. Even if this is not of interest to you this is a fairly common case
    /// so you should special case it for better performance
    #[must_use]
    pub fn lowest_common_parent(
        &self,
        span1: CtxSpan,
        span2: CtxSpan,
    ) -> (SourceContextId, TextRange, TextRange) {
        // self.index() is used as a capacity because it is the lowest upper bound we know
        let mut ancestors = AHashMap::new();
        ancestors.insert(span1.ctx, span1.range);
        let mut current = span1.ctx;
        while let Some(call_site) = self.contexts[current].call_site {
            current = call_site.ctx;
            ancestors.insert(current, call_site.range);
        }

        current = span2.ctx;
        let mut current_call_span = span2.range;

        loop {
            if let Some(call_span) = ancestors.get(&current).copied() {
                return (current, call_span, current_call_span);
            }

            let span = self.contexts[current].call_site.expect("CTXT paths dont intersect at root");
            current = span.ctx;
            current_call_span = span.range;
        }
    }
}
