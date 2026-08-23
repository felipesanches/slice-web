//! Loading a font and reporting what the three editors should display.

use read_fonts::{FontRef, TableProvider};
use write_fonts::types::{NameId, Tag};

use crate::axes::AxisSpec;
use crate::bits::BitFlags;
use crate::names::{NameEdits, NAME_EDITOR_IDS};
use crate::SliceError;

/// The Windows / Unicode BMP / English (US) name record triple. The original Slice reads
/// and writes only this one, and so do we.
pub const WIN_PLATFORM: u16 = 3;
pub const WIN_ENCODING: u16 = 1;
pub const WIN_LANGUAGE: u16 = 1033;

/// A font held in memory, with the accessors the UI needs.
///
/// The bytes are always plain sfnt: web font containers are unwrapped by
/// [`crate::font::decode_container`] before they get here.
#[derive(Clone)]
pub struct SliceFont {
    data: Vec<u8>,
}

impl SliceFont {
    /// Read a font from `data`, which may be a bare sfnt or a WOFF/WOFF2 container.
    pub fn load(data: Vec<u8>) -> Result<Self, SliceError> {
        let data = decode_container(data)?;
        // Parse once up front so that a bad file is rejected at load time, the way the
        // original's `load_font` does, rather than at Slice time.
        FontRef::new(&data).map_err(|e| SliceError::Read(e.to_string()))?;
        Ok(Self { data })
    }

    /// The raw sfnt bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn font_ref(&self) -> Result<FontRef<'_>, SliceError> {
        FontRef::new(&self.data).map_err(|e| SliceError::Read(e.to_string()))
    }

    /// True when the font has an `fvar` table.
    ///
    /// This is exactly the original's `FontModel.is_variable_font` check.
    pub fn is_variable(&self) -> bool {
        self.font_ref().map(|f| f.fvar().is_ok()).unwrap_or(false)
    }

    /// True when outlines live in `glyf` (as opposed to `CFF `/`CFF2`).
    pub fn is_truetype(&self) -> bool {
        self.font_ref()
            .map(|f| f.table_data(Tag::new(b"glyf")).is_some())
            .unwrap_or(false)
    }

    /// The rows of the Axis Editor, in the font's own `fvar` order.
    pub fn axes(&self) -> Result<Vec<AxisSpec>, SliceError> {
        let font = self.font_ref()?;
        let fvar = font.fvar().map_err(|_| SliceError::NotVariable)?;
        let mut out = Vec::new();
        for axis in fvar.axes()?.iter() {
            let tag = axis.axis_tag();
            out.push(AxisSpec {
                tag: tag.to_string(),
                min: axis.min_value().to_f64(),
                default: axis.default_value().to_f64(),
                max: axis.max_value().to_f64(),
                name: self.name_string(axis.axis_name_id()),
                // fvar axis flag bit 0 is HIDDEN_AXIS.
                hidden: axis.flags() & 0x0001 != 0,
            });
        }
        Ok(out)
    }

    /// The rows of the Name Editor, prefilled from the font.
    pub fn name_edits(&self) -> NameEdits {
        let mut edits = NameEdits::default();
        for &id in NAME_EDITOR_IDS {
            if let Some(text) = self.name_string(NameId::new(id)) {
                edits.set(id, text);
            }
        }
        edits
    }

    /// The checkbox state of the Bit Flag Editor, prefilled from the font.
    ///
    /// The original always starts these unchecked, which silently clears whatever the
    /// input font had. Reading the real values means an untouched Bit Flag Editor is a
    /// no-op instead of a destructive edit.
    pub fn bit_flags(&self) -> BitFlags {
        let Ok(font) = self.font_ref() else {
            return BitFlags::default();
        };
        BitFlags {
            fs_selection: font.os2().map(|t| t.fs_selection().bits()).unwrap_or(0),
            mac_style: font.head().map(|t| t.mac_style().bits()).unwrap_or(0),
        }
    }

    /// nameID 1, used in the status bar.
    pub fn family_name(&self) -> Option<String> {
        self.name_string(NameId::new(1))
    }

    /// nameID 5 up to the first `;`, matching the original's `get_version`.
    pub fn version(&self) -> Option<String> {
        self.name_string(NameId::new(5))
            .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
    }

    pub fn units_per_em(&self) -> u16 {
        self.font_ref()
            .and_then(|f| Ok(f.head()?.units_per_em()))
            .unwrap_or(1000)
    }

    pub fn glyph_count(&self) -> u16 {
        self.font_ref()
            .and_then(|f| Ok(f.maxp()?.num_glyphs()))
            .unwrap_or(0)
    }

    /// Look one name record up.
    ///
    /// Windows/Unicode/English is tried first because that is the record Slice edits; a
    /// font that only carries some other Windows language would otherwise show an empty
    /// Name Editor even though it has perfectly good names.
    pub fn name_string(&self, id: NameId) -> Option<String> {
        let font = self.font_ref().ok()?;
        let name = font.name().ok()?;
        let data = name.string_data();
        let records = name.name_record();

        let exact = records.iter().find(|r| {
            r.name_id() == id
                && r.platform_id() == WIN_PLATFORM
                && r.encoding_id() == WIN_ENCODING
                && r.language_id() == WIN_LANGUAGE
        });
        let fallback = || {
            records
                .iter()
                .find(|r| r.name_id() == id && r.platform_id() == WIN_PLATFORM)
                .or_else(|| records.iter().find(|r| r.name_id() == id))
        };

        let record = exact.or_else(fallback)?;
        record.string(data).ok().map(|s| s.chars().collect())
    }
}

/// Unwrap a WOFF or WOFF2 container, or pass plain sfnt bytes through unchanged.
pub fn decode_container(data: Vec<u8>) -> Result<Vec<u8>, SliceError> {
    match data.get(..4) {
        Some(b"wOFF") => crate::font::woff::decode_woff(&data),
        Some(b"wOF2") => Err(SliceError::Unsupported(
            "WOFF2 input is not supported yet. Convert the font to TTF/OTF or WOFF first."
                .into(),
        )),
        // 0x00010000 (TrueType), "true", "OTTO" (CFF), "ttcf" (collection).
        _ => Ok(data),
    }
}

pub mod woff;

#[cfg(test)]
mod tests {
    use super::*;

    fn recursive() -> SliceFont {
        SliceFont::load(crate::testdata::recursive_vf().to_vec()).unwrap()
    }

    #[test]
    fn reads_the_axis_editor_rows_in_fvar_order() {
        let axes = recursive().axes().unwrap();
        let tags: Vec<_> = axes.iter().map(|a| a.tag.as_str()).collect();
        assert_eq!(tags, ["MONO", "CASL", "wght", "slnt", "CRSV"]);

        let wght = &axes[2];
        assert_eq!(wght.min, 300.0);
        assert_eq!(wght.default, 300.0);
        assert_eq!(wght.max, 1000.0);
        assert_eq!(wght.range_label(), "300.0 : 1000.0 [300.0]");

        let crsv = &axes[4];
        assert_eq!((crsv.min, crsv.default, crsv.max), (0.0, 0.5, 1.0));

        let slnt = &axes[3];
        assert_eq!((slnt.min, slnt.default, slnt.max), (-15.0, 0.0, 0.0));
    }

    #[test]
    fn recognises_a_variable_truetype_font() {
        let f = recursive();
        assert!(f.is_variable());
        assert!(f.is_truetype());
    }

    #[test]
    fn a_static_font_is_not_variable() {
        let f = SliceFont::load(crate::testdata::recursive_sliced().to_vec()).unwrap();
        assert!(!f.is_variable());
    }

    #[test]
    fn reads_the_name_editor_rows() {
        let f = recursive();
        let edits = f.name_edits();
        assert_eq!(edits.get(1).unwrap(), "Recursive Sans Linear Light");
        assert!(edits.get(6).is_some(), "postscript name should be present");
        assert_eq!(f.family_name().unwrap(), "Recursive Sans Linear Light");
        assert!(f.version().is_some());
    }

    #[test]
    fn woff_input_is_unwrapped_to_the_same_font() {
        let ttf = recursive();
        let woff = SliceFont::load(crate::testdata::recursive_vf_woff().to_vec()).unwrap();
        assert_eq!(
            ttf.axes().unwrap(),
            woff.axes().unwrap(),
            "WOFF and TTF inputs should describe the same design space"
        );
        assert_eq!(ttf.glyph_count(), woff.glyph_count());
    }

    #[test]
    fn garbage_is_rejected_at_load_time() {
        assert!(SliceFont::load(b"this is not a font".to_vec()).is_err());
    }
}
