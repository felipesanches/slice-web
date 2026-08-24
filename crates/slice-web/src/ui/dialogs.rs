//! The modal dialogs: About, the error report, and the progress indicator.

use leptos::prelude::*;

use crate::state::AppState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The error dialog: one sentence, with the technical detail behind a disclosure.
///
/// This is the shape the original uses, and it is the right one: the sentence is for the
/// person, and the detail is for whoever they forward it to.
#[component]
pub fn ErrorDialog(state: AppState) -> impl IntoView {
    view! {
        <Show when=move || state.error.get().is_some()>
            {move || {
                let message = state.error.get().expect("guarded by Show");
                view! {
                    <div class="modal-backdrop" on:click=move |_| state.clear_error()>
                        <div
                            class="modal error"
                            role="alertdialog"
                            aria-modal="true"
                            aria-labelledby="error-title"
                            on:click=|ev| ev.stop_propagation()
                        >
                            <h2 id="error-title">"Error"</h2>
                            <p>{message.summary.clone()}</p>
                            {message
                                .details
                                .clone()
                                .map(|details| {
                                    view! {
                                        <details>
                                            <summary>"Details"</summary>
                                            <pre>{details}</pre>
                                        </details>
                                    }
                                })}
                            <div class="buttons">
                                <button class="primary" on:click=move |_| state.clear_error()>
                                    "OK"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            }}
        </Show>
    }
}

/// Shown while a slice runs.
///
/// The engine runs on the main thread, so this is painted before the work starts and
/// stays put until it finishes; it cannot animate meaningfully in between. It is
/// deliberately honest about that rather than showing a bar that pretends to move.
#[component]
pub fn ProgressDialog(state: AppState) -> impl IntoView {
    view! {
        <Show when=move || state.busy.get()>
            <div class="modal-backdrop">
                <div class="modal progress" role="status" aria-live="polite">
                    <h2>"Slicing…"</h2>
                    <div class="indeterminate"><div class="bar"></div></div>
                    <p class="hint">"The page will be unresponsive until this finishes."</p>
                </div>
            </div>
        </Show>
    }
}

#[component]
pub fn AboutDialog(state: AppState) -> impl IntoView {
    view! {
        <Show when=move || state.about_open.get()>
            <div class="modal-backdrop" on:click=move |_| state.about_open.set(false)>
                <div
                    class="modal about"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="about-title"
                    on:click=|ev| ev.stop_propagation()
                >
                    <div class="about-head">
                        <SliceLogo/>
                        <h2 id="about-title">"Slice"</h2>
                    </div>
                    <p>"Version " {VERSION}</p>
                    <p class="about-lead">
                        "Builds custom design sub-spaces from variable fonts, in the "
                        "browser. Fonts are read and written locally; nothing is uploaded."
                    </p>
                    <p>
                        "A reimplementation of "
                        <a href="https://github.com/source-foundry/Slice" target="_blank" rel="noreferrer">
                            "Slice"
                        </a>
                        " by Source Foundry (Christopher Simpkins), which was a PyQt5 "
                        "desktop application built on fontTools. This version keeps its "
                        "interface, moves the engine to Rust and WebAssembly, and adds "
                        "overlap removal."
                    </p>
                    <h3>"Built with"</h3>
                    <ul class="credits">
                        <li>
                            <a href="https://github.com/googlefonts/fontations" target="_blank" rel="noreferrer">
                                "fontations"
                            </a>
                            " — read-fonts, write-fonts and skrifa"
                        </li>
                        <li>
                            <a href="https://github.com/Logicalshift/flo_curves" target="_blank" rel="noreferrer">
                                "flo_curves"
                            </a>
                            " — Bézier path arithmetic, for overlap removal"
                        </li>
                        <li>
                            <a href="https://github.com/linebender/kurbo" target="_blank" rel="noreferrer">
                                "kurbo"
                            </a>
                            " — curve geometry"
                        </li>
                        <li>
                            <a href="https://leptos.dev" target="_blank" rel="noreferrer">"Leptos"</a>
                            " — the interface"
                        </li>
                        <li>
                            "The sub-space solver is a port of the one in "
                            <a href="https://github.com/fonttools/fonttools" target="_blank" rel="noreferrer">
                                "fontTools"
                            </a>
                        </li>
                        // The application ships the icon, so its attribution has to be
                        // reachable from inside the application, not only from the
                        // repository's thirdparty/ directory.
                        <li>
                            "The icon is the original Slice project's, itself a derivative "
                            "of a "
                            <a
                                href="https://www.flaticon.com/free-icon/cheesecake_3400263"
                                target="_blank"
                                rel="noreferrer"
                            >
                                "cheesecake icon"
                            </a>
                            " by flaticon.com, used under the Flaticon license"
                        </li>
                    </ul>
                    <p class="licence">
                        "GNU General Public License v3 or later, as the original is."
                    </p>
                    <div class="buttons">
                        <button class="primary" on:click=move |_| state.about_open.set(false)>
                            "OK"
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// The application mark, shared with the documentation site.
#[component]
pub fn SliceLogo() -> impl IntoView {
    // The original Slice project's icon, so this is recognisable as the same tool. It is
    // one file shared with the documentation site rather than inline markup: `build.sh`
    // copies `docs/assets/slice-icon.svg` into the build, so the app and the website
    // cannot drift into showing different logos.
    //
    // It is decorative here -- every place it appears sits beside the word "Slice" -- so
    // it carries an empty alt and is hidden from assistive technology rather than making
    // a screen reader announce the name twice.
    view! {
        <img class="logo" src="./slice-icon.svg" alt="" aria-hidden="true"/>
    }
}
