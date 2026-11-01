// SPDX-License-Identifier: GPL-3.0-or-later
//! Dataset-local uniqueness; repeating a macro in another frame is a different scope.
use crate::load::LoadError;
use dicom_core::Tag;
use std::collections::HashSet;

pub(super) struct Scopes {
    // None is a sequence; Some is a root/item dataset with its own attribute tags.
    levels: Vec<Option<HashSet<Tag>>>,
}
impl Scopes {
    pub fn new() -> Result<Self, LoadError> {
        let mut levels = Vec::new();
        levels.try_reserve_exact(65).map_err(|_| LoadError::Limit)?;
        levels.push(Some(HashSet::new()));
        Ok(Self { levels })
    }
    pub fn depth(&self) -> usize {
        self.levels.len() - 1
    }
    pub fn attribute(&mut self, tag: Tag) -> Result<(), LoadError> {
        let Some(Some(tags)) = self.levels.last_mut() else {
            return Err(LoadError::Unsupported);
        };
        if tags.contains(&tag) {
            return Err(LoadError::Unsupported);
        }
        tags.try_reserve(1).map_err(|_| LoadError::Limit)?;
        tags.insert(tag);
        Ok(())
    }
    fn enter(&mut self, scope: Option<HashSet<Tag>>) -> Result<(), LoadError> {
        if self.levels.len() >= 65 {
            return Err(LoadError::Limit);
        };
        self.levels.push(scope);
        Ok(())
    }
    pub fn sequence(&mut self, tag: Tag) -> Result<(), LoadError> {
        self.attribute(tag)?;
        self.enter(None)
    }
    pub fn item(&mut self) -> Result<(), LoadError> {
        if !matches!(self.levels.last(), Some(None)) {
            return Err(LoadError::Unsupported);
        };
        self.enter(Some(HashSet::new()))
    }
    pub fn end_sequence(&mut self) -> Result<(), LoadError> {
        if self.levels.len() <= 1 || !matches!(self.levels.last(), Some(None)) {
            return Err(LoadError::Unsupported);
        };
        self.levels.pop();
        Ok(())
    }
    pub fn end_item(&mut self) -> Result<(), LoadError> {
        if self.levels.len() <= 1 || !matches!(self.levels.last(), Some(Some(_))) {
            return Err(LoadError::Unsupported);
        };
        self.levels.pop();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_frame_macros_are_distinct_but_repeated_attributes_in_one_item_are_not() {
        let mut scopes = Scopes::new().unwrap();
        scopes.sequence(Tag(0x5200, 0x9230)).unwrap();
        for _ in 0..3 {
            scopes.item().unwrap();
            scopes.sequence(Tag(0x0020, 0x9113)).unwrap();
            scopes.item().unwrap();
            scopes.attribute(Tag(0x0020, 0x0032)).unwrap();
            assert_eq!(
                scopes.attribute(Tag(0x0020, 0x0032)),
                Err(LoadError::Unsupported)
            );
            scopes.end_item().unwrap();
            scopes.end_sequence().unwrap();
            scopes.end_item().unwrap();
        }
        scopes.end_sequence().unwrap();
        assert_eq!(scopes.depth(), 0);
        assert_eq!(scopes.end_item(), Err(LoadError::Unsupported));
        scopes.sequence(Tag(0x5200, 0x9229)).unwrap();
        scopes.end_sequence().unwrap();
        assert_eq!(
            scopes.sequence(Tag(0x5200, 0x9229)),
            Err(LoadError::Unsupported)
        );
    }
}
