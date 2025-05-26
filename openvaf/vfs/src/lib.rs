//! # Virtual File System
//!
//! VFS stores all files read by rust-analyzer. Reading file contents from VFS
//! always returns the same contents, unless VFS was explicitly modified with
//! [`set_file_contents`]. All changes to VFS are logged, and can be retrieved via
//! [`take_changes`] method. The pack of changes is then pushed to `salsa` and
//! triggers incremental recomputation.
//!
//! Files in VFS are identified with [`FileId`]s -- interned paths. The notion of
//! the path, [`VfsPath`] is somewhat abstract: at the moment, it is represented
//! as an [`std::path::PathBuf`] internally, but this is an implementation detail.
//!
//! VFS doesn't do IO or file watching itself. For that, see the [`loader`]
//! module. [`loader::Handle`] is an object-safe trait which abstracts both file
//! loading and file watching. [`Handle`] is dynamically configured with a set of
//! directory entries which should be scanned and watched. [`Handle`] then
//! asynchronously pushes file changes. Directory entries are configured in
//! free-form via list of globs, it's up to the [`Handle`] to interpret the globs
//! in any specific way.
//!
//! VFS stores a flat list of files. [`file_set::FileSet`] can partition this list
//! of files into disjoint sets of files. Traversal-like operations (including
//! getting the neighbor file by the relative path) are handled by the [`FileSet`].
//! [`FileSet`]s are also pushed to salsa and cause it to re-check `mod foo;`
//! declarations when files are created or deleted.
//!
//! [`set_file_contents`]: Vfs::set_file_contents
//! [`take_changes`]: Vfs::take_changes
//! [`FileSet`]: file_set::FileSet
//! [`Handle`]: loader::Handle
//! [`Entries`]: loader::Entry
//!
//! See Also:
//! - [rust analyzer vfs](https://github.com/rust-lang/rust-analyzer/tree/master/crates/vfs)

#![allow(rustdoc::broken_intra_doc_links)]

use std::char::REPLACEMENT_CHARACTER;
use std::ops::Range;
use std::sync::Arc;
use std::{fmt, io, mem};

pub use paths::{AbsPath, AbsPathBuf};

//mod anchored_path;
//pub mod loader;
mod path_interner;
mod va_std;
mod vfs_path;

//pub use crate::anchored_path::{AnchoredPath, AnchoredPathBuf};
use crate::path_interner::PathInterner;
pub use crate::vfs_path::VfsPath;

/// Virtual File System
#[derive(Default)]
pub struct Vfs {
    interner: PathInterner,
    data: Vec<VfsEntry>,
    changes: Vec<ChangedFile>,
}

impl fmt::Debug for Vfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vfs").field("n_files", &self.data.len()).finish()
    }
}

/// Handle to a file in [`Vfs`]
///
/// Most functions use this when they need to refer to a file.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct FileId(pub u16);

/// Entry of file data stored in [`Vfs`]
#[derive(Debug, PartialEq, Eq)]
pub struct VfsEntry {
    contents: Box<str>,
    err: Option<FileReadError>,
}

impl Default for VfsEntry {
    fn default() -> Self {
        Self { contents: Default::default(), err: Some(FileReadError::Io(io::ErrorKind::NotFound)) }
    }
}

/// Detect byte encoding in case of invalid text formatting of source code.
impl From<Vec<u8>> for VfsEntry {
    fn from(contents: Vec<u8>) -> Self {
        // TODO allow back transformations
        let mut detector = chardetng::EncodingDetector::new();
        if contents.len() > u16::MAX as usize {
            detector.feed(&contents[..u16::MAX as usize], false);
        } else {
            detector.feed(&contents, true);
        }
        let (res, _, malformed) = detector.guess(None, true).decode(&contents);
        let err = malformed
            .then(|| FileReadError::InvalidTextFormat(InvalidTextFormatErr::from_lossy(&res)));
        let contents = LineEndings::normalize(res.into()).0;

        VfsEntry { contents: contents.into_boxed_str(), err }
    }
}

impl From<Result<Vec<u8>, io::ErrorKind>> for VfsEntry {
    fn from(src: Result<Vec<u8>, io::ErrorKind>) -> Self {
        match src {
            Err(err) => VfsEntry { contents: Box::from(""), err: Some(FileReadError::Io(err)) },
            Ok(contents) => contents.into(),
        }
    }
}

impl From<io::Result<Vec<u8>>> for VfsEntry {
    fn from(src: io::Result<Vec<u8>>) -> Self {
        src.map_err(|err| err.kind()).into()
    }
}

impl From<String> for VfsEntry {
    fn from(contents: String) -> Self {
        VfsEntry { contents: contents.into_boxed_str(), err: None }
    }
}

impl From<Box<str>> for VfsEntry {
    fn from(contents: Box<str>) -> Self {
        VfsEntry { contents, err: None }
    }
}

#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub enum FileReadError {
    Io(io::ErrorKind),
    InvalidTextFormat(InvalidTextFormatErr),
}

impl FileReadError {
    pub fn is_io(&self) -> bool {
        matches!(self, FileReadError::Io(_))
    }
}

#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub struct InvalidTextFormatErr {
    pub pos: Arc<[Range<usize>]>,
}

impl InvalidTextFormatErr {
    pub fn from_lossy(src: &str) -> InvalidTextFormatErr {
        let mut start = None;
        let mut pos = 0;
        let mut spans = Vec::new();
        for it in src.chars() {
            #[allow(clippy::match_same_arms)]
            match (start, it) {
                (None, REPLACEMENT_CHARACTER) => start = Some(pos),
                (Some(_), REPLACEMENT_CHARACTER) => (),
                (Some(old), _) => {
                    spans.push(old..pos);
                    start = None
                }
                _ => (),
            }
            pos += it.len_utf8();
        }

        if let Some(start) = start {
            spans.push(start..src.len());
        }

        InvalidTextFormatErr { pos: Arc::from(spans.into_boxed_slice()) }
    }
}

/// Changed file in the [`Vfs`].
pub struct ChangedFile {
    /// Id of the changed file
    pub file_id: FileId,
    /// Kind of change
    pub change_kind: ChangeKind,
}

impl ChangedFile {
    /// Returns `true` if the change is not [`Delete`](ChangeKind::Delete).
    pub fn exists(&self) -> bool {
        self.change_kind != ChangeKind::Delete
    }

    /// Returns `true` if the change is [`Create`](ChangeKind::Create) or
    /// [`Delete`](ChangeKind::Delete).
    pub fn is_created_or_deleted(&self) -> bool {
        matches!(self.change_kind, ChangeKind::Create | ChangeKind::Delete)
    }
}

/// Kind of [file change](ChangedFile).
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum ChangeKind {
    /// The file was (re-)created
    Create,
    /// The file was modified
    Modify,
    /// The file was deleted
    Delete,
}

pub type VfsExport = Vec<(Box<str>, Box<str>)>;

impl Vfs {
    /// Amount of files currently stored.
    ///
    /// Note that this includes deleted files.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Return file ID associated with `path` stored in the `Vfs`, if any.
    pub fn file_id(&self, path: &VfsPath) -> Option<FileId> {
        self.interner.get(path)
    }

    /// Returns the id associated with `path`,
    ///
    /// - If `path` does not exist in the `Vfs`, allocate a new id for it;
    /// - Else, returns `path`'s id.
    ///
    /// Does not record a change.
    pub fn ensure_file_id(&mut self, path: VfsPath) -> FileId {
        let file_id = self.interner.intern(path);
        let idx = file_id.0 as usize;
        let len = self.data.len().max(idx + 1);
        self.data.resize_with(len, VfsEntry::default);
        file_id
    }

    /// Add a virtual file to `Vfs`.
    ///
    /// used by `va_std` and tests.
    pub fn add_virt_file(&mut self, name: &str, src: VfsEntry) -> FileId {
        let path = VfsPath::new_virtual_path(name.to_owned());
        let file_id = self.ensure_file_id(path);
        self.set_file_contents(file_id, src);
        file_id
    }

    /// File path corresponding to the given `file_id`.
    ///
    /// # Panics
    ///
    /// Panics if the id is not present in the `Vfs`.
    pub fn file_path(&self, file_id: FileId) -> VfsPath {
        self.interner.lookup(file_id).clone()
    }

    /// File content corresponding to the given `file_id`.
    pub fn file_contents_unchecked(&self, file_id: FileId) -> &str {
        &self.get(file_id).contents
    }

    /// File content corresponding to the given `file_id`.
    pub fn file_contents(&self, file_id: FileId) -> Result<&str, FileReadError> {
        let entry = self.get(file_id);
        if let Some(err) = &entry.err {
            Err(err.clone())
        } else {
            Ok(&entry.contents)
        }
    }

    /// Update the file with the given `contents`.
    ///
    /// Returns `true` if the file was modified, and saves the [change](ChangedFile).
    pub fn set_file_contents(&mut self, file_id: FileId, contents: VfsEntry) -> bool {
        let old = self.get(file_id);
        if old == &contents {
            return false;
        }
        let change_kind = if old.err.as_ref().is_some_and(|err| err.is_io()) {
            ChangeKind::Create
        } else if contents.err.as_ref().is_some_and(|err| err.is_io()) {
            ChangeKind::Delete
        } else {
            ChangeKind::Modify
        };
        *self.get_mut(file_id) = contents;
        self.changes.push(ChangedFile { file_id, change_kind });

        true
    }

    /// Returns `true` if the `Vfs` contains [changes](ChangedFile).
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Drain and returns all the changes in the `Vfs`.
    pub fn take_changes(&mut self) -> Vec<ChangedFile> {
        mem::take(&mut self.changes)
    }

    /// Returns the content associated with the given `file_id`.
    ///
    /// # Panics
    ///
    /// Panics if no file is associated to that id.
    fn get(&self, file_id: FileId) -> &VfsEntry {
        &self.data[file_id.0 as usize]
    }

    /// Mutably returns the content associated with the given `file_id`.
    ///
    /// # Panics
    ///
    /// Panics if no file is associated to that id.
    fn get_mut(&mut self, file_id: FileId) -> &mut VfsEntry {
        &mut self.data[file_id.0 as usize]
    }

    /// Returns an iterator over the stored ids and their corresponding paths.
    ///
    /// This will skip deleted/invalid files.
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &VfsPath)> + '_ {
        (0..self.data.len())
            .map(|it| FileId(it as u16))
            .filter(move |&file_id| self.get(file_id).err.is_none())
            .map(move |file_id| {
                let path = self.interner.lookup(file_id);
                (file_id, path)
            })
    }

    // used by verilogae
    pub fn export_native_paths_to_virt(
        &self,
        root_file: &AbsPath,
    ) -> Result<(VfsExport, Vec<&AbsPath>), &'static str> {
        let mut ignored_files = Vec::new();

        let anchor = root_file.parent().unwrap();
        let path: Result<Vec<_>, _> = self
            .iter()
            .filter_map(|(file, path)| {
                if let Some(path) = path.as_path() {
                    if let Some(rel_path) = path.strip_prefix(anchor) {
                        let path = match rel_path.as_ref().to_str() {
                            Some(path) => path,
                            None => {
                                return Some(Err("all paths must be valid utf8 for VFS export"))
                            }
                        };
                        let mut path = if std::path::MAIN_SEPARATOR != '/' {
                            if path.contains('/') {
                                return Some(Err("VFS paths must not contain '/'"));
                            }
                            path.replace(std::path::MAIN_SEPARATOR, "/")
                        } else {
                            path.to_owned()
                        };

                        // all paths must start with '/'
                        path.insert(0, '/');

                        Some(Ok((path.into_boxed_str(), self.get(file).contents.clone())))
                    } else {
                        ignored_files.push(path);
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        Ok((path?, ignored_files))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LineEndings {
    Unix,
    Dos,
}

impl LineEndings {
    /// Replaces `\r\n` with `\n` in-place in `src`.
    pub(crate) fn normalize(src: String) -> (String, LineEndings) {
        // We replace `\r\n` with `\n` in-place, which doesn't break utf-8 encoding.
        // While we *can* call `as_mut_vec` and do surgery on the live string
        // directly, let's rather steal the contents of `src`. This makes the code
        // safe even if a panic occurs.

        let mut buf = src.into_bytes();
        let mut gap_len = 0;
        let mut tail = buf.as_mut_slice();
        let mut crlf_seen = false;

        let find_crlf = |src: &[u8]| src.windows(2).position(|it| it == b"\r\n");

        loop {
            let idx = match find_crlf(&tail[gap_len..]) {
                None if crlf_seen => tail.len(),
                // SAFETY: buf is unchanged and therefore still contains utf8 data
                None => return (unsafe { String::from_utf8_unchecked(buf) }, LineEndings::Unix),
                Some(idx) => {
                    crlf_seen = true;
                    idx + gap_len
                }
            };
            tail.copy_within(gap_len..idx, 0);
            tail = &mut tail[idx - gap_len..];
            if tail.len() == gap_len {
                break;
            }
            gap_len += 1;
        }

        // Account for removed `\r`.
        // After `set_len`, `buf` is guaranteed to contain utf-8 again.
        let src = unsafe {
            let new_len = buf.len() - gap_len;
            buf.set_len(new_len);
            String::from_utf8_unchecked(buf)
        };
        (src, LineEndings::Dos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix() {
        let src = "a\nb\nc\n\n\n\n";
        let (res, endings) = LineEndings::normalize(src.into());
        assert_eq!(endings, LineEndings::Unix);
        assert_eq!(res, src);
    }

    #[test]
    fn dos() {
        let src = "\r\na\r\n\r\nb\r\nc\r\n\r\n\r\n\r\n";
        let (res, endings) = LineEndings::normalize(src.into());
        assert_eq!(endings, LineEndings::Dos);
        assert_eq!(res, "\na\n\nb\nc\n\n\n\n");
    }

    #[test]
    fn mixed() {
        let src = "a\r\nb\r\nc\r\n\n\r\n\n";
        let (res, endings) = LineEndings::normalize(src.into());
        assert_eq!(endings, LineEndings::Dos);
        assert_eq!(res, "a\nb\nc\n\n\n\n");
    }

    #[test]
    fn none() {
        let src = "abc";
        let (res, endings) = LineEndings::normalize(src.into());
        assert_eq!(endings, LineEndings::Unix);
        assert_eq!(res, src);
    }
}
