use leptos::prelude::*;

use super::types::ModelForm;
use crate::utils::target_value;

#[component]
pub fn ModelEditorExtraArgsForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    view! {
        <label class="form-label" for="field-args">"Extra args"</label>
        <textarea
            id="field-args"
            class="form-textarea"
            rows="6"
            placeholder="One flag per line, e.g.:\n-fa 1\n-b 4096\n--mlock"
            prop:value=move || form.get().as_ref().map(|f| f.args.clone()).unwrap_or_default()
            on:input=move |e| {
                form.update(|f| {
                    if let Some(form) = f {
                        form.args = target_value(&e);
                    }
                });
            }
        />
        <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces: -m \"path with space/m.gguf\""</span>
    }
}
