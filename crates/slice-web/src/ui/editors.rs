//! The three editors, laid out as the original lays them out.

use leptos::prelude::*;
use slice_core::bits::{HEAD_MAC_STYLE, OS2_FS_SELECTION};
use slice_core::names::{row_hint, row_label, NAME_EDITOR_IDS};
use slice_core::BitFlags;

use crate::state::AppState;

/// The Axis Editor: one row per `fvar` axis, the original extent on the left and the
/// user's entry on the right.
#[component]
pub fn AxisEditor(state: AppState) -> impl IntoView {
    view! {
        <section class="group">
            <h2>"Axis Editor"</h2>
            <Show
                when=move || !state.axes.get().is_empty()
                fallback=|| view! { <p class="empty">"Open a variable font to see its axes."</p> }
            >
                <table class="editor axis-editor">
                    <thead>
                        <tr>
                            <th scope="col" class="axis-tag">"Axis"</th>
                            <th scope="col">"Min : Max [Default]"</th>
                            <th scope="col">"Edit Values"</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || {
                                state.axes.get().into_iter().enumerate().collect::<Vec<_>>()
                            }
                            key=|(index, axis)| (*index, axis.tag.clone())
                            let:entry
                        >
                            {
                                let (index, axis) = entry;
                                let tooltip = axis.display_name().unwrap_or_default();
                                let hidden = axis.hidden;
                                let label = axis.range_label();
                                let problem = move || state.axis_entry_problem(index);
                                view! {
                                    <tr>
                                        <th scope="row" class="axis-tag" title=tooltip.clone()>
                                            {axis.tag.clone()}
                                            <Show when=move || hidden>
                                                <span class="badge" title="This axis is flagged hidden in fvar">
                                                    "hidden"
                                                </span>
                                            </Show>
                                        </th>
                                        <td class="numeric range">{label}</td>
                                        <td>
                                            <input
                                                type="text"
                                                class="numeric"
                                                class:invalid=move || problem().is_some()
                                                placeholder="whole range"
                                                aria-label=format!("{} axis value", axis.tag)
                                                prop:value=move || {
                                                    state
                                                        .axis_text
                                                        .get()
                                                        .get(index)
                                                        .cloned()
                                                        .unwrap_or_default()
                                                }
                                                value=move || {
                                                    state
                                                        .axis_text
                                                        .get()
                                                        .get(index)
                                                        .cloned()
                                                        .unwrap_or_default()
                                                }
                                                on:input=move |ev| {
                                                    let value = event_target_value(&ev);
                                                    state
                                                        .axis_text
                                                        .update(|entries| {
                                                            if let Some(slot) = entries.get_mut(index) {
                                                                *slot = value;
                                                            }
                                                        });
                                                }
                                            />
                                            <Show when=move || problem().is_some()>
                                                <p class="field-error">{move || problem().unwrap_or_default()}</p>
                                            </Show>
                                        </td>
                                    </tr>
                                }
                            }
                        </For>
                    </tbody>
                </table>
                <p class="hint">
                    "Leave a row blank to keep the whole axis, type a number to pin it "
                    "(" <code>"400"</code> "), or a range to restrict it ("
                    <code>"200:700"</code> ")."
                </p>
            </Show>
        </section>
    }
}

/// The Name Editor: the nine `name` records the original exposes.
#[component]
pub fn NameEditor(state: AppState) -> impl IntoView {
    view! {
        <section class="group">
            <h2>"Name Editor"</h2>
            <table class="editor name-editor">
                <thead>
                    <tr>
                        <th scope="col">"Record"</th>
                        <th scope="col">"Edit Values"</th>
                    </tr>
                </thead>
                <tbody>
                    // Keyed by nameID, because the set of rows never changes. That means
                    // a row is never rebuilt, so the input's value has to be a reactive
                    // closure rather than a snapshot -- otherwise opening a font fills
                    // the model but leaves every field on screen empty.
                    <For
                        each=move || { NAME_EDITOR_IDS.to_vec() }
                        key=|id| *id
                        let:id
                    >
                        <tr>
                            <th scope="row" title=row_hint(id)>{row_label(id)}</th>
                            <td>
                                <input
                                    type="text"
                                    aria-label=row_label(id)
                                    // `prop:value` is what the browser actually
                                    // displays; the attribute is set alongside it so
                                    // that a serialised DOM shows the real contents,
                                    // which is what makes this inspectable from the
                                    // outside and testable without a driver.
                                    prop:value=move || {
                                        state.names.get().get_or_empty(id).to_string()
                                    }
                                    value=move || {
                                        state.names.get().get_or_empty(id).to_string()
                                    }
                                    on:input=move |ev| {
                                        let text = event_target_value(&ev);
                                        state.names.update(|names| { names.set(id, text); });
                                    }
                                />
                            </td>
                        </tr>
                    </For>
                </tbody>
            </table>
            <p class="hint">
                "Records 1, 2, 3, 4 and 6 are always written. Clearing one of the "
                "optional rows removes that record from the font."
            </p>
        </section>
    }
}

/// The Bit Flag Editor: `OS/2.fsSelection` and `head.macStyle`.
#[component]
pub fn BitFlagEditor(state: AppState) -> impl IntoView {
    view! {
        <section class="group">
            <h2>"Bit Flag Editor"</h2>
            <div class="bit-groups">
                <fieldset>
                    <legend>"OS/2.fsSelection"</legend>
                    <div class="checkboxes">
                        {OS2_FS_SELECTION
                            .iter()
                            .map(|def| {
                                let offset = def.offset;
                                view! {
                                    <label title=def.hint>
                                        <input
                                            type="checkbox"
                                            prop:checked=move || state.bits.get().fs_selection_bit(offset)
                                            on:change=move |ev| {
                                                let on = event_target_checked(&ev);
                                                state
                                                    .bits
                                                    .update(|bits| bits.set_fs_selection_bit(offset, on));
                                            }
                                        />
                                        <span>{def.label}</span>
                                    </label>
                                }
                            })
                            .collect_view()}
                    </div>
                    <p class="binary" title="The whole 16-bit field, most significant bit first">
                        {move || BitFlags::binary(state.bits.get().fs_selection)}
                    </p>
                </fieldset>

                <fieldset>
                    <legend>"head.macStyle"</legend>
                    <div class="checkboxes">
                        {HEAD_MAC_STYLE
                            .iter()
                            .map(|def| {
                                let offset = def.offset;
                                view! {
                                    <label title=def.hint>
                                        <input
                                            type="checkbox"
                                            prop:checked=move || state.bits.get().mac_style_bit(offset)
                                            on:change=move |ev| {
                                                let on = event_target_checked(&ev);
                                                state
                                                    .bits
                                                    .update(|bits| bits.set_mac_style_bit(offset, on));
                                            }
                                        />
                                        <span>{def.label}</span>
                                    </label>
                                }
                            })
                            .collect_view()}
                    </div>
                    <p class="binary" title="The whole 16-bit field, most significant bit first">
                        {move || BitFlags::binary(state.bits.get().mac_style)}
                    </p>
                </fieldset>
            </div>

            <Show when=move || !state.bits.get().warnings().is_empty()>
                <ul class="warnings">
                    <For
                        each=move || state.bits.get().warnings()
                        key=|w| w.clone()
                        let:warning
                    >
                        <li>{warning}</li>
                    </For>
                </ul>
            </Show>

            <p class="hint">
                "Bits are read from the font when it is opened, so leaving this alone "
                "changes nothing. Bits not shown here are preserved."
            </p>
        </section>
    }
}
