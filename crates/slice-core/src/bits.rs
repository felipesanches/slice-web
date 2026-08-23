//! The Bit Flag Editor: the `OS/2.fsSelection` and `head.macStyle` bits Slice exposes.

/// One checkbox in the Bit Flag Editor.
pub struct BitDef {
    /// Bit offset within the 16-bit field.
    pub offset: u8,
    /// The label on the checkbox, e.g. `bit 0 (ITALIC)`.
    pub label: &'static str,
    /// What setting the bit actually means.
    pub hint: &'static str,
}

/// The `OS/2.fsSelection` checkboxes, in the order the original lays them out.
pub const OS2_FS_SELECTION: &[BitDef] = &[
    BitDef {
        offset: 0,
        label: "bit 0 (ITALIC)",
        hint: "Glyphs are slanted. Set together with head.macStyle bit 1.",
    },
    BitDef {
        offset: 5,
        label: "bit 5 (BOLD)",
        hint: "Glyphs are emboldened. Set together with head.macStyle bit 0.",
    },
    BitDef {
        offset: 6,
        label: "bit 6 (REGULAR)",
        hint: "Glyphs are the regular style. Mutually exclusive with ITALIC and BOLD.",
    },
    BitDef {
        offset: 8,
        label: "bit 8 (WWS)",
        hint: "Family name differs from other family members only in weight/width/slope.",
    },
];

/// The `head.macStyle` checkboxes.
pub const HEAD_MAC_STYLE: &[BitDef] = &[
    BitDef {
        offset: 0,
        label: "bit 0 (BOLD)",
        hint: "Bold. Should agree with OS/2.fsSelection bit 5.",
    },
    BitDef {
        offset: 1,
        label: "bit 1 (ITALIC)",
        hint: "Italic. Should agree with OS/2.fsSelection bit 0.",
    },
];

/// The state of the Bit Flag Editor, held as the two raw 16-bit fields.
///
/// Holding whole fields rather than a set of booleans matters: bits the editor does not
/// expose (`fsSelection` bits 1-4, 7, 9…) have to survive untouched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BitFlags {
    pub fs_selection: u16,
    pub mac_style: u16,
}

impl BitFlags {
    pub fn fs_selection_bit(&self, offset: u8) -> bool {
        self.fs_selection & (1 << offset) != 0
    }

    pub fn mac_style_bit(&self, offset: u8) -> bool {
        self.mac_style & (1 << offset) != 0
    }

    pub fn set_fs_selection_bit(&mut self, offset: u8, on: bool) {
        self.fs_selection = set_bit(self.fs_selection, offset, on);
    }

    pub fn set_mac_style_bit(&mut self, offset: u8, on: bool) {
        self.mac_style = set_bit(self.mac_style, offset, on);
    }

    /// Bits the editor exposes that contradict each other.
    ///
    /// The OpenType specification requires REGULAR to be mutually exclusive with BOLD and
    /// ITALIC, and requires the two BOLD bits and the two ITALIC bits to agree. The
    /// original Slice lets you ship any combination silently; reporting it costs nothing
    /// and these are exactly the mistakes the editor makes easy.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let italic = self.fs_selection_bit(0);
        let bold = self.fs_selection_bit(5);
        let regular = self.fs_selection_bit(6);
        let mac_bold = self.mac_style_bit(0);
        let mac_italic = self.mac_style_bit(1);

        if regular && (bold || italic) {
            out.push(
                "OS/2.fsSelection REGULAR (bit 6) must not be set together with BOLD \
                 (bit 5) or ITALIC (bit 0)."
                    .into(),
            );
        }
        if bold != mac_bold {
            out.push(format!(
                "OS/2.fsSelection BOLD (bit 5) is {} but head.macStyle BOLD (bit 0) is \
                 {}; these should agree.",
                on_off(bold),
                on_off(mac_bold)
            ));
        }
        if italic != mac_italic {
            out.push(format!(
                "OS/2.fsSelection ITALIC (bit 0) is {} but head.macStyle ITALIC (bit 1) \
                 is {}; these should agree.",
                on_off(italic),
                on_off(mac_italic)
            ));
        }
        out
    }

    /// Render a field as a 16-character binary string, most significant bit first.
    ///
    /// The original prints exactly this to stdout for debugging; the web UI shows it
    /// under the checkboxes instead, where it is actually visible.
    pub fn binary(value: u16) -> String {
        (0..16)
            .rev()
            .map(|b| if value & (1 << b) != 0 { '1' } else { '0' })
            .collect()
    }
}

fn set_bit(field: u16, offset: u8, on: bool) -> u16 {
    if on {
        field | (1 << offset)
    } else {
        field & !(1 << offset)
    }
}

fn on_off(v: bool) -> &'static str {
    if v {
        "set"
    } else {
        "clear"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_and_clearing_leaves_other_bits_alone() {
        let mut f = BitFlags {
            fs_selection: 0b0000_0001_1010_0010,
            mac_style: 0,
        };
        let before = f.fs_selection;
        f.set_fs_selection_bit(6, true);
        assert_eq!(f.fs_selection, before | (1 << 6));
        f.set_fs_selection_bit(6, false);
        assert_eq!(f.fs_selection, before, "unexposed bits must survive");
    }

    #[test]
    fn binary_rendering_matches_the_originals_debug_output() {
        assert_eq!(BitFlags::binary(0), "0000000000000000");
        assert_eq!(BitFlags::binary(1 << 6), "0000000001000000");
        assert_eq!(BitFlags::binary(0xFFFF), "1111111111111111");
    }

    #[test]
    fn contradictory_bits_are_reported() {
        let f = BitFlags {
            fs_selection: (1 << 5) | (1 << 6), // BOLD and REGULAR together
            mac_style: 1 << 0,
        };
        let w = f.warnings();
        assert!(w.iter().any(|m| m.contains("REGULAR")), "{w:?}");

        // Bold set in OS/2 but not in head.
        let f = BitFlags {
            fs_selection: 1 << 5,
            mac_style: 0,
        };
        assert!(f.warnings().iter().any(|m| m.contains("BOLD")));
    }

    #[test]
    fn a_consistent_regular_font_produces_no_warnings() {
        let f = BitFlags {
            fs_selection: 1 << 6,
            mac_style: 0,
        };
        assert!(f.warnings().is_empty(), "{:?}", f.warnings());
    }
}
