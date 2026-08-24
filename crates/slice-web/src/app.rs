//! The application shell: layout, the font drop zone, and the Slice button.

use leptos::prelude::*;
use leptos::task::spawn_local;
use slice_core::OutputFormat;
use wasm_bindgen::JsCast;
use web_sys::{HtmlInputElement, HtmlSelectElement};

use crate::files;
use crate::state::AppState;
use crate::ui::dialogs::{AboutDialog, ErrorDialog, ProgressDialog, SliceLogo, VERSION};
use crate::ui::editors::{AxisEditor, BitFlagEditor, NameEditor};
use crate::ui::menubar::MenuBar;

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();
    let file_input: NodeRef<leptos::html::Input> = NodeRef::new();

    // Opening a file, from the button, the menu, or a drop.
    let open_dialog = Callback::new(move |_: ()| {
        if let Some(input) = file_input.get() {
            input.click();
        }
    });

    let accept_file = move |file: web_sys::File| {
        spawn_local(async move {
            match files::read_file(file).await {
                Ok((name, bytes)) => state.load_font(name, bytes),
                Err(message) => state.report("The file could not be read.", Some(message)),
            }
        });
    };

    let on_file_chosen = move |ev: web_sys::Event| {
        let input: HtmlInputElement = ev.target().unwrap().unchecked_into();
        if let Some(files) = input.files() {
            if let Some(file) = files.get(0) {
                accept_file(file);
            }
        }
        // Clear the value so choosing the same file twice still fires a change event.
        input.set_value("");
    };

    let load_sample = Callback::new(move |_: ()| {
        spawn_local(async move {
            match files::fetch_same_origin(files::SAMPLE_PATH).await {
                Ok(bytes) => state.load_font(files::SAMPLE_NAME.to_string(), bytes),
                Err(message) => state.report("The sample font could not be loaded.", Some(message)),
            }
        });
    });

    // Settings the page was opened with, applied as soon as there is a font to apply them
    // to. Held rather than consumed, so that opening a second font restores them again --
    // a bookmarked weight is a thing you want for the next font too, and the address bar
    // still says so.
    let pending = StoredValue::new(crate::settings::from_location());
    Effect::new(move |_| {
        // Reruns whenever a font is loaded, because `axes` is read here.
        let axes = state.axes.get();
        if axes.is_empty() {
            return;
        }
        pending.with_value(|settings| {
            if !settings.is_empty() {
                state.apply_settings(settings);
            }
        });
    });

    // `?sample` in the URL opens the bundled font on start, so a link can show the tool
    // already working. It is also what the browser smoke test drives.
    if files::wants_sample() {
        load_sample.run(());
    }

    // The File menu advertises Ctrl+O, so it has to work. Bound on the document rather
    // than the app element, because the shortcut should fire wherever focus happens to
    // be -- except inside a text field, where Ctrl+O is the browser's to interpret.
    {
        use wasm_bindgen::closure::Closure;
        if let Some(document) = web_sys::window().and_then(|w| w.document()) {
            let handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |event: web_sys::KeyboardEvent| {
                    if (event.ctrl_key() || event.meta_key())
                        && !event.alt_key()
                        && event.key().eq_ignore_ascii_case("o")
                    {
                        event.prevent_default();
                        open_dialog.run(());
                    }
                },
            );
            let _ = document
                .add_event_listener_with_callback("keydown", handler.as_ref().unchecked_ref());
            // The listener outlives this scope; the document owns it for the page's life.
            handler.forget();
        }
    }

    let dragging = RwSignal::new(false);

    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        dragging.set(false);
        if let Some(transfer) = ev.data_transfer() {
            if let Some(files) = transfer.files() {
                if let Some(file) = files.get(0) {
                    accept_file(file);
                }
            }
        }
    };

    view! {
        <div
            class="app"
            class:dragging=move || dragging.get()
            on:dragover=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                dragging.set(true);
            }
            on:dragleave=move |_| dragging.set(false)
            on:drop=on_drop
        >
            <MenuBar state=state on_open=open_dialog/>

            <header class="title">
                <SliceLogo/>
                <h1>"Slice"</h1>
                <p class="tagline">"Custom design sub-spaces from variable fonts"</p>
            </header>

            <main>
                <FontPathRow state=state open_dialog=open_dialog load_sample=load_sample dragging=dragging/>
                <AxisEditor state=state/>
                <NameEditor state=state/>
                <BitFlagEditor state=state/>
                <OutlineOptions state=state/>
                <SliceButton state=state/>
                <ResultNotes state=state/>
            </main>

            <footer class="statusbar">
                <span class="status">{move || state.status.get()}</span>
                <span class="version">"v" {VERSION}</span>
            </footer>

            <input
                node_ref=file_input
                type="file"
                accept=files::ACCEPTED_EXTENSIONS
                class="hidden-file-input"
                on:change=on_file_chosen
            />

            <ErrorDialog state=state/>
            <AboutDialog state=state/>
            <ProgressDialog state=state/>
        </div>
    }
}

/// The font path row: a drop target and an Open button.
#[component]
fn FontPathRow(
    state: AppState,
    open_dialog: Callback<()>,
    load_sample: Callback<()>,
    dragging: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <section class="group font-path">
            <div class="dropzone" class:active=move || dragging.get()>
                <Show
                    when=move || state.font.with(Option::is_some)
                    fallback=move || {
                        view! {
                            <p class="placeholder">
                                "Drop a variable font here, or click Open."
                                <button class="linklike" on:click=move |_| load_sample.run(())>
                                    "Try the sample font"
                                </button>
                            </p>
                        }
                    }
                >
                    <div class="loaded">
                        <strong>{move || state.file_name.get()}</strong>
                        <span class="detail">
                            {move || {
                                state
                                    .font
                                    .with(|font| {
                                        font.as_ref()
                                            .map(|font| {
                                                format!(
                                                    "{} glyphs · {} units per em · {}",
                                                    font.glyph_count(),
                                                    font.units_per_em(),
                                                    if font.is_truetype() {
                                                        "TrueType outlines"
                                                    } else {
                                                        "CFF outlines"
                                                    },
                                                )
                                            })
                                            .unwrap_or_default()
                                    })
                            }}
                        </span>
                    </div>
                </Show>
            </div>
            <button class="secondary" on:click=move |_| open_dialog.run(())>
                "Open"
            </button>
        </section>
    }
}

/// The overlap-removal option and the output format.
///
/// This group has no counterpart in the original, which never removed overlaps and
/// inferred the output format from the file name in a save dialog the browser does not
/// give us.
#[component]
fn OutlineOptions(state: AppState) -> impl IntoView {
    view! {
        <section class="group">
            <h2>"Outlines and Output"</h2>
            <label class="option">
                <input
                    type="checkbox"
                    prop:checked=move || state.remove_overlaps.get()
                    on:change=move |ev| state.remove_overlaps.set(event_target_checked(&ev))
                />
                <span>
                    <strong>"Remove overlapping contours"</strong>
                    <span class="detail">
                        "Merges each glyph's contours into one non-overlapping outline. "
                        "Needs every axis pinned, and drops hinting. Worth it because "
                        "design applications still handle overlaps badly."
                    </span>
                </span>
            </label>

            <label class="option format">
                <span><strong>"Output format"</strong></span>
                <select on:change=move |ev| {
                    let select: HtmlSelectElement = ev.target().unwrap().unchecked_into();
                    state
                        .format
                        .set(match select.value().as_str() {
                            "woff" => OutputFormat::Woff,
                            "woff2" => OutputFormat::Woff2,
                            _ => OutputFormat::Sfnt,
                        });
                }>
                    <option value="sfnt" selected=move || state.format.get() == OutputFormat::Sfnt>
                        "TrueType / OpenType (.ttf, .otf)"
                    </option>
                    <option value="woff" selected=move || state.format.get() == OutputFormat::Woff>
                        "WOFF (.woff)"
                    </option>
                    <option
                        value="woff2"
                        selected=move || state.format.get() == OutputFormat::Woff2
                    >
                        "WOFF2 (.woff2)"
                    </option>
                </select>
            </label>

            <p class="hint">
                "Saves as "
                <code>{move || state.suggested_output_name()}</code>
            </p>
        </section>
    }
}

/// The Slice button, and the work it starts.
#[component]
fn SliceButton(state: AppState) -> impl IntoView {
    let run = move |_| {
        if state.font.with(Option::is_none) {
            state.status.set("Requires a font path".to_string());
            return;
        }

        state.clear_error();
        state.busy.set(true);
        state.status.set("Slicing…".to_string());

        // Yield to the browser so the progress dialog is actually painted before the
        // engine takes over the main thread. Without this the dialog would only appear
        // after the work it is meant to cover had already finished.
        spawn_local(async move {
            next_tick().await;
            perform(state);
            state.busy.set(false);
        });
    };

    view! {
        <div class="actions">
            <button
                class="primary slice"
                disabled=move || state.busy.get() || state.font.with(Option::is_none)
                on:click=run
            >
                "Slice"
            </button>
        </div>
    }
}

/// Run the job and hand the result to the browser.
fn perform(state: AppState) {
    let job = match state.build_job() {
        Ok(job) => job,
        Err(e) => {
            state.report(&e.to_string(), None);
            return;
        }
    };

    let output = state.font.with(|font| {
        let font = font.as_ref().expect("guarded by the caller");
        job.run(font)
    });

    match output {
        Ok(output) => {
            let name = state.suggested_output_name();
            match files::download(&output.bytes, &name, job.format.mime_type()) {
                Ok(()) => {
                    state.status.set(format!("Saved {name}"));

                    // The address bar becomes a description of the slice that just ran,
                    // so the browser's own bookmark button saves the settings. Done here
                    // rather than on every keystroke: a URL that changes while you type
                    // is one you cannot copy.
                    let settings = state.capture_settings();
                    crate::settings::write_to_location(&settings);

                    let mut notes = output.notes;
                    if !settings.is_empty() {
                        notes.push(
                            "These settings are now in the address bar — bookmark the page \
                             to keep them."
                                .to_string(),
                        );
                    }
                    state.last_result.set(notes);
                }
                Err(message) => state.report("The font could not be saved.", Some(message)),
            }
        }
        Err(e) => {
            // Errors that name an axis or explain a rule read better as the summary;
            // there is no extra detail to hide behind a disclosure.
            state.report(
                "Font processing failed. See details below.",
                Some(e.to_string()),
            );
        }
    }
}

/// What the last successful slice did.
#[component]
fn ResultNotes(state: AppState) -> impl IntoView {
    view! {
        <Show when=move || !state.last_result.get().is_empty()>
            <section class="group notes">
                <h2>"Last slice"</h2>
                <ul>
                    <For
                        each=move || state.last_result.get()
                        key=|note| note.clone()
                        let:note
                    >
                        <li>{note}</li>
                    </For>
                </ul>
            </section>
        </Show>
    }
}

/// Resolve after the browser has had a chance to paint.
async fn next_tick() {
    use wasm_bindgen::prelude::*;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("a window");
        // Two frames: one to paint the dialog, one to be sure it is on screen.
        let inner = Closure::once_into_js(move || {
            let window = web_sys::window().expect("a window");
            window
                .request_animation_frame(resolve.unchecked_ref())
                .expect("request_animation_frame");
        });
        window
            .request_animation_frame(inner.unchecked_ref())
            .expect("request_animation_frame");
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
