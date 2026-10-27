//! Lazy, read-only object access over an in-memory PDF.
//!
//! [`Document::load_mem`](crate::Document::load_mem) parses every indirect object up
//! front, content streams and images included. A caller that needs only part of the
//! object graph (page annotations, the outline, the info dictionary) pays for the whole
//! file. [`LazyDocument`] resolves the cross-reference table and trailer, then parses
//! each object only when it is asked for. Object streams are decoded at most once each,
//! since one stream holds many of the small dictionaries a graph walk visits.

use std::collections::HashSet;

use super::{MAX_NESTING_DEPTH, Reader};
use crate::error::ParseError;
use crate::load_options::LoadOptions;
use crate::parser;
use crate::{Dictionary, Error, Object, ObjectId, Result};

/// A PDF whose objects are parsed on demand. See the module docs.
pub struct LazyDocument<'a> {
    reader: Reader<'a>,
}

impl<'a> LazyDocument<'a> {
    /// Resolve the cross-reference table and trailer of `buffer`, rebuilding the table
    /// when it is damaged (unless `options.strict`). An encrypted document is opened with
    /// the empty password, then `options.password`; objects come back decrypted.
    pub fn load_mem(buffer: &'a [u8], options: LoadOptions) -> Result<Self> {
        let offset = buffer.windows(5).position(|w| w == b"%PDF-").unwrap_or(0);
        let mut reader = Reader::with_options(&buffer[offset..], options);
        reader.document.version = parser::header(reader.buffer, reader.strict).ok_or(ParseError::InvalidFileHeader)?;
        let (xref, trailer) = match reader.resolve_xref_and_trailer() {
            Ok(resolved) => resolved,
            Err(err) => reader.reconstruct_xref_and_trailer().ok_or(err)?,
        };
        reader.set_reference_table(xref);
        reader.document.trailer = trailer;
        if reader.document.trailer.get(b"Encrypt").is_ok() {
            reader.parse_encryption_dictionary()?;
            reader.authenticate_and_setup_encryption(true)?;
        }
        Ok(Self { reader })
    }

    /// The PDF version from the file header.
    pub fn version(&self) -> &str {
        &self.reader.document.version
    }

    pub fn trailer(&self) -> &Dictionary {
        &self.reader.document.trailer
    }

    /// Parse the object `id`.
    pub fn get_object(&self, id: ObjectId) -> Result<Object> {
        self.reader.get_object(id, &mut HashSet::new())
    }

    /// Follow `object` through indirect references to the direct object they name.
    pub fn resolve(&self, object: &Object) -> Result<Object> {
        let mut current = object.clone();
        let mut seen = HashSet::new();
        while let Object::Reference(id) = current {
            if !seen.insert(id) || seen.len() > MAX_NESTING_DEPTH {
                return Err(Error::ReferenceCycle(id));
            }
            current = self.get_object(id)?;
        }
        Ok(current)
    }

    /// The document catalog, from the trailer's `/Root`.
    pub fn catalog(&self) -> Result<Dictionary> {
        Ok(self.resolve(self.trailer().get(b"Root")?)?.as_dict()?.clone())
    }

    /// The page objects in document order, from the catalog's page tree.
    ///
    /// A node with a `/Kids` array is an intermediate node; any other node is a page.
    /// Nodes already visited, nodes that fail to load and subtrees deeper than
    /// [`MAX_NESTING_DEPTH`] are skipped, so a damaged tree yields the pages it can reach.
    pub fn page_ids(&self) -> Result<Vec<ObjectId>> {
        let root = self.catalog()?.get(b"Pages")?.as_reference()?;
        let mut pages = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((id, depth)) = stack.pop() {
            if depth >= MAX_NESTING_DEPTH || !seen.insert(id) {
                continue;
            }
            let Ok(Object::Dictionary(node)) = self.get_object(id) else {
                continue;
            };
            match node.get(b"Kids").ok().map(|kids| self.resolve(kids)) {
                Some(Ok(Object::Array(kids))) => {
                    // Reversed so the stack pops kids in document order.
                    stack.extend(
                        kids.iter()
                            .rev()
                            .filter_map(|kid| kid.as_reference().ok())
                            .map(|kid| (kid, depth + 1)),
                    );
                }
                _ => pages.push(id),
            }
        }
        Ok(pages)
    }
}
