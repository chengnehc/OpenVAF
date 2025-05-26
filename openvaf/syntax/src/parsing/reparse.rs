//! Incremental reparsing (todo)

use super::Parse;
use crate::SourceFile;

impl Parse<SourceFile> {
    // pub fn reparse(&self, indel: &Indel) -> Parse<SourceFile> {
    //     self.full_reparse(indel)
    // self.incremental_reparse(indel).unwrap_or_else(|| self.full_reparse(indel))
    // }
    //
    // fn incremental_reparse(&self, indel: &Indel) -> Option<Parse<SourceFile>> {
    //     parsing::incremental_reparse(self.tree().syntax(), indel, self.errors.to_vec()).map(
    //         |(green_node, errors, _reparsed_range)| Parse {
    //             green: green_node,
    //             errors: Arc::new(errors),
    //             _ty: PhantomData,
    //         },
    //     )
    // }
    //
    // fn full_reparse(&self, indel: &Indel) -> Parse<SourceFile> {
    //     let mut text = self.tree().syntax().text().to_string();
    //     indel.apply(&mut text);
    //     SourceFile::parse(&text)
    // }
}
