//! The Name Editor: the nine `name` table records Slice lets you rewrite.

use std::collections::BTreeMap;

/// The nameIDs the Name Editor shows, in editor row order.
pub const NAME_EDITOR_IDS: &[u16] = &[1, 2, 3, 4, 6, 16, 17, 21, 22];

/// nameIDs that must always be present in the output.
///
/// The original writes these unconditionally, even when the user blanked the field.
pub const MANDATORY_IDS: &[u16] = &[1, 2, 3, 4, 6];

/// nameIDs that are written when non-empty and *deleted* when the user clears them.
pub const OPTIONAL_IDS: &[u16] = &[16, 17, 21, 22];

/// The row label shown at the left of the Name Editor.
pub fn row_label(name_id: u16) -> &'static str {
    match name_id {
        1 => "01 Family",
        2 => "02 Subfamily",
        3 => "03 Unique",
        4 => "04 Full",
        6 => "06 Postscript",
        16 => "16 Typo Family",
        17 => "17 Typo Subfamily",
        21 => "21 WWS Family",
        22 => "22 WWS Subfamily",
        _ => "",
    }
}

/// A longer explanation, shown as a tooltip. The original offers no such hint; the
/// nameID numbers alone are opaque unless you already know the `name` table.
pub fn row_hint(name_id: u16) -> &'static str {
    match name_id {
        1 => "Font Family name. Limited to four styles per family by legacy systems.",
        2 => "Font Subfamily name. Regular, Italic, Bold or Bold Italic.",
        3 => "Unique font identifier.",
        4 => "Full font name, usually Family plus Subfamily.",
        6 => "PostScript name. No spaces, at most 63 characters.",
        16 => "Typographic Family name, when the family has more than four styles.",
        17 => "Typographic Subfamily name, when the family has more than four styles.",
        21 => "WWS Family name, for families that vary beyond weight/width/slope.",
        22 => "WWS Subfamily name, for families that vary beyond weight/width/slope.",
        _ => "",
    }
}

/// The contents of the Name Editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameEdits {
    values: BTreeMap<u16, String>,
}

impl NameEdits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name_id: u16, value: impl Into<String>) -> &mut Self {
        self.values.insert(name_id, value.into());
        self
    }

    /// The editor cell contents, or `None` when the row is blank.
    pub fn get(&self, name_id: u16) -> Option<&str> {
        self.values
            .get(&name_id)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// The editor cell contents, treating a missing row as blank.
    pub fn get_or_empty(&self, name_id: u16) -> &str {
        self.values.get(&name_id).map(String::as_str).unwrap_or("")
    }

    /// Rows in editor order, as `(nameID, text)`.
    pub fn rows(&self) -> impl Iterator<Item = (u16, &str)> {
        NAME_EDITOR_IDS
            .iter()
            .map(move |&id| (id, self.get_or_empty(id)))
    }

    /// The records to write, and the records to delete, when applying these edits.
    ///
    /// Mandatory IDs are always written. Optional IDs are written when the user typed
    /// something and removed when they cleared the field, which is how the original
    /// distinguishes "leave it alone" from "take it out".
    pub fn plan(&self) -> (Vec<(u16, String)>, Vec<u16>) {
        let mut writes = Vec::new();
        let mut deletes = Vec::new();

        for &id in MANDATORY_IDS {
            writes.push((id, self.get_or_empty(id).to_string()));
        }
        for &id in OPTIONAL_IDS {
            match self.get(id) {
                Some(text) => writes.push((id, text.to_string())),
                None => deletes.push(id),
            }
        }
        (writes, deletes)
    }

    /// True when no row has any text in it.
    pub fn is_empty(&self) -> bool {
        NAME_EDITOR_IDS.iter().all(|&id| self.get(id).is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_in_editor_order() {
        let edits = NameEdits::new();
        let ids: Vec<_> = edits.rows().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 6, 16, 17, 21, 22]);
    }

    #[test]
    fn blank_optional_rows_become_deletions() {
        let mut edits = NameEdits::new();
        edits.set(1, "Test Family").set(16, "Typo Family");
        let (writes, deletes) = edits.plan();

        // Every mandatory ID is written, even the ones left blank.
        for &id in MANDATORY_IDS {
            assert!(writes.iter().any(|(w, _)| *w == id), "missing write for {id}");
        }
        assert!(writes.contains(&(16, "Typo Family".to_string())));
        // 17, 21 and 22 were never filled in, so they come out.
        assert_eq!(deletes, vec![17, 21, 22]);
    }

    #[test]
    fn whitespace_only_is_still_text() {
        // The original treats only the empty string as "delete this record", so a space
        // is a real (if odd) value. Keep that behaviour rather than trimming silently.
        let mut edits = NameEdits::new();
        edits.set(16, " ");
        let (writes, deletes) = edits.plan();
        assert!(writes.contains(&(16, " ".to_string())));
        assert!(!deletes.contains(&16));
    }
}
