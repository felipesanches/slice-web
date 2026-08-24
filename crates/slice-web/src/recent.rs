//! Fonts the user has opened before, and what they last did to them.
//!
//! The store itself is `crates/slice-web/js/font-store.js`, adapted from TypeRoof's
//! `local-font-storage.mjs` under the Apache License 2.0. It is JavaScript because
//! IndexedDB is a callback API with no `async` surface, and driving it from Rust through
//! `web-sys` means hand-rolling a future for every request; the borrowed module already
//! does that correctly and has been in use in a real application.
//!
//! What is stored is the font's bytes and the settings last used with it, so choosing a
//! font from the list is not "open this file again" but "carry on where I left off".
//! Nothing leaves the machine: IndexedDB is per-origin browser storage, and this page has
//! no server to send it to.

/// How long a cached copy may sit before the interface suggests checking upstream.
pub const STALE_AFTER_DAYS: f64 = 14.0;

use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/font-store.js")]
extern "C" {
    #[wasm_bindgen(js_name = listFonts, catch)]
    async fn list_fonts_js() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = getFont, catch)]
    async fn get_font_js(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = putFont, catch)]
    async fn put_font_js(
        id: &str,
        name: &str,
        family: &str,
        version: &str,
        bytes: &[u8],
        settings: &str,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = forgetFont, catch)]
    async fn forget_font_js(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = forgetAll, catch)]
    async fn forget_all_js() -> Result<JsValue, JsValue>;
}

/// One remembered font, without its bytes.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RecentFont {
    pub id: String,
    /// The file name it was opened as.
    pub name: String,
    /// The family name out of the font, which is usually the more recognisable of the two.
    pub family: String,
    /// The `name` table's version string, nameID 5, as the font reports itself.
    pub version: String,
    /// The settings last used with it, as a query string.
    pub settings: String,
    /// Milliseconds since the epoch, when this copy of the font first arrived.
    pub added: f64,
    pub size: f64,
}

impl RecentFont {
    /// What to show in the list: the family, falling back to the file name.
    pub fn label(&self) -> &str {
        if self.family.is_empty() {
            &self.name
        } else {
            &self.family
        }
    }

    /// How long ago this copy arrived, and whether that is long enough to mention.
    ///
    /// A cached font is a copy of a file that has a life of its own: the foundry ships a
    /// new version, the repository gets a fix, and the copy here goes quietly out of date
    /// with nothing to say so. After a fortnight it is worth a nudge — long enough not to
    /// nag about a font opened last Tuesday, short enough to catch a release.
    pub fn is_stale(&self) -> bool {
        self.age_days() >= STALE_AFTER_DAYS
    }

    pub fn age_days(&self) -> f64 {
        let now = js_sys::Date::now();
        ((now - self.added) / (1000.0 * 60.0 * 60.0 * 24.0)).max(0.0)
    }

    /// When it arrived, in the reader's own locale and time zone.
    pub fn added_label(&self) -> String {
        let date = js_sys::Date::new(&JsValue::from_f64(self.added));
        let day = date.to_locale_date_string("default", &JsValue::UNDEFINED);
        let time = date.to_locale_time_string("default");
        format!("{day} {time}")
    }

    /// "3 days ago", for the tooltip, where the exact stamp is already shown.
    pub fn age_label(&self) -> String {
        let days = self.age_days();
        if days < 1.0 {
            "today".to_string()
        } else if days < 2.0 {
            "yesterday".to_string()
        } else if days < 60.0 {
            format!("{:.0} days ago", days)
        } else {
            format!("{:.0} months ago", days / 30.0)
        }
    }

    pub fn size_label(&self) -> String {
        let kb = self.size / 1024.0;
        if kb >= 1024.0 {
            format!("{:.1} MB", kb / 1024.0)
        } else {
            format!("{kb:.0} kB")
        }
    }
}

/// The identity a font is remembered under.
///
/// The file name alone is a poor key -- half the world's variable fonts are called
/// `font.ttf` at some point -- and the bytes alone would list the same font twice under
/// two names. The pair is stable across reloads and distinguishes two versions of the
/// same family, which is exactly what someone comparing them would want.
pub fn identity(name: &str, family: &str, bytes: &[u8]) -> String {
    // FNV-1a over the bytes. A cryptographic digest would be pointless here: this is a
    // cache key, not a security boundary, and pulling in a hash crate for it is not worth
    // the compile time in a wasm bundle.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{family}\u{1}{name}\u{1}{hash:016x}")
}

pub async fn list() -> Vec<RecentFont> {
    match list_fonts_js().await {
        Ok(value) => serde_wasm_bindgen::from_value(value).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub async fn get(id: &str) -> Option<Vec<u8>> {
    let value = get_font_js(id).await.ok()?;
    if value.is_null() || value.is_undefined() {
        return None;
    }
    Some(js_sys::Uint8Array::new(&value).to_vec())
}

/// Remember a font. Failure is deliberately invisible: the font is already open and
/// working, and a browser that will not persist it is not something to interrupt over.
pub async fn put(id: &str, name: &str, family: &str, version: &str, bytes: &[u8], settings: &str) {
    let _ = put_font_js(id, name, family, version, bytes, settings).await;
}

pub async fn forget(id: &str) {
    let _ = forget_font_js(id).await;
}

pub async fn forget_all() {
    let _ = forget_all_js().await;
}
