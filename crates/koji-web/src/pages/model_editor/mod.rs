mod api;
mod extra_args_form;
mod general_form;
mod quants_vision_form;
mod sampling_form;
mod sections;
mod types;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};

use self::api::*;
use self::extra_args_form::ModelEditorExtraArgsForm;
use self::general_form::ModelEditorGeneralForm;
use self::quants_vision_form::ModelEditorQuantsVisionForm;
use self::sampling_form::ModelEditorSamplingForm;
use self::types::*;

// Helper to convert RwSignal to Signal for Modal
fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}

// ── Component ─────────────────────────────────────────────────────────────────

use self::sections::Section;

#[component]
pub fn ModelEditor() -> impl IntoView {
    let params = use_params_map();
    let model_id = move || params.get().get("id").unwrap_or_default();
    let is_new = move || model_id() == "new";

    let detail: LocalResource<Option<ModelDetail>> = LocalResource::new(move || {
        let id = model_id();
        async move { fetch_model(id).await }
    });

    // Use LocalResource for templates
    let templates: LocalResource<Option<std::collections::HashMap<String, serde_json::Value>>> =
        LocalResource::new(|| async move { fetch_sampling_templates().await });

    // Consolidated form signal
    let form = RwSignal::new(Option::<ModelForm>::None);

    // UI-only signals (not part of form)
    let backends = RwSignal::new(Vec::<String>::new());
    let original_id = RwSignal::new(String::new());
    let pull_modal_open_signal = RwSignal::new(false);

    // Status
    let save_status = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);

    // Repo-level DB metadata (from Phase 3 API enrichment)
    let repo_commit_sha = RwSignal::new(Option::<String>::None);
    let repo_pulled_at = RwSignal::new(Option::<String>::None);

    // Status for refresh / verify actions (busy flag + last message)
    let refresh_busy = RwSignal::new(false);
    let verify_busy = RwSignal::new(false);
    let refresh_status = RwSignal::new(Option::<(bool, String)>::None);
    let verify_status = RwSignal::new(Option::<(bool, String)>::None);

    // Active navigation section
    let active_section = RwSignal::new(Section::General);

    // Tracks whether the form has been populated from the loaded model detail.
    // Used to gate the layout render without depending on form.get() (which changes on every keystroke).
    let form_ready = RwSignal::new(false);

    // Populate signals when resource loads
    Effect::new(move |_| {
        if let Some(guard) = detail.get() {
            if let Some(d) = guard.take() {
                backends.set(d.backends.clone());
                original_id.set(d.id.to_string());

                // Build consolidated form
                let mut sampling_fields = std::collections::HashMap::new();
                if let Some(sampling_json) = &d.sampling {
                    if let Some(obj) = sampling_json.as_object() {
                        if let Some(temp) = obj.get("temperature") {
                            if let Some(val) = temp.as_f64() {
                                sampling_fields.insert(
                                    "temperature".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_k) = obj.get("top_k") {
                            if let Some(val) = top_k.as_u64() {
                                sampling_fields.insert(
                                    "top_k".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_p) = obj.get("top_p") {
                            if let Some(val) = top_p.as_f64() {
                                sampling_fields.insert(
                                    "top_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(min_p) = obj.get("min_p") {
                            if let Some(val) = min_p.as_f64() {
                                sampling_fields.insert(
                                    "min_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(presence) = obj.get("presence_penalty") {
                            if let Some(val) = presence.as_f64() {
                                sampling_fields.insert(
                                    "presence_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(frequency) = obj.get("frequency_penalty") {
                            if let Some(val) = frequency.as_f64() {
                                sampling_fields.insert(
                                    "frequency_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(repeat_pen) = obj.get("repeat_penalty") {
                            if let Some(val) = repeat_pen.as_f64() {
                                sampling_fields.insert(
                                    "repeat_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                    }
                }

                // Initialize modalities if absent so checkboxes have stable structure
                let mut modalities = d.modalities.clone();
                if modalities.is_none() {
                    modalities = Some(ModelModalities {
                        input: Vec::new(),
                        output: Vec::new(),
                    });
                }
                form.set(Some(ModelForm {
                    id: d.id.to_string(),
                    backend: d.backend.clone(),
                    model: d.model,
                    quant: d.quant,
                    mmproj: d.mmproj,
                    args: d.args.join("\n"),
                    sampling: sampling_fields,
                    enabled: d.enabled,
                    context_length: d.context_length,
                    port: d.port,
                    api_name: d.api_name.clone(),
                    display_name: d.display_name.clone(),
                    gpu_layers: d.gpu_layers,
                    quants: d.quants.clone(),
                    modalities,
                }));

                repo_commit_sha.set(d.repo_commit_sha.clone());
                repo_pulled_at.set(d.repo_pulled_at.clone());
                form_ready.set(true);
            }
        }
    });

    let load_preset_action: Action<String, (), LocalStorage> =
        Action::new_unsync(move |preset_name: &String| {
            let preset_name_clone = preset_name.clone();
            async move {
                let templates_map: Option<std::collections::HashMap<String, serde_json::Value>> =
                    templates.get().and_then(|g| (*g).clone());
                if let Some(templates_map) = templates_map {
                    if let Some(preset) = templates_map.get(&preset_name_clone) {
                        if let Some(obj) = preset.as_object() {
                            form.update(|f| {
                                if let Some(form) = f {
                                    if let Some(temp) = obj.get("temperature") {
                                        if let Some(val) = temp.as_f64() {
                                            form.sampling
                                                .entry("temperature".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(top_k) = obj.get("top_k") {
                                        if let Some(val) = top_k.as_u64() {
                                            form.sampling
                                                .entry("top_k".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(top_p) = obj.get("top_p") {
                                        if let Some(val) = top_p.as_f64() {
                                            form.sampling
                                                .entry("top_p".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(min_p) = obj.get("min_p") {
                                        if let Some(val) = min_p.as_f64() {
                                            form.sampling
                                                .entry("min_p".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(presence) = obj.get("presence_penalty") {
                                        if let Some(val) = presence.as_f64() {
                                            form.sampling
                                                .entry("presence_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(frequency) = obj.get("frequency_penalty") {
                                        if let Some(val) = frequency.as_f64() {
                                            form.sampling
                                                .entry("frequency_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                    if let Some(repeat_pen) = obj.get("repeat_penalty") {
                                        if let Some(val) = repeat_pen.as_f64() {
                                            form.sampling
                                                .entry("repeat_penalty".to_string())
                                                .and_modify(|field| {
                                                    field.enabled = true;
                                                    field.value = val.to_string();
                                                })
                                                .or_insert(SamplingField {
                                                    enabled: true,
                                                    value: val.to_string(),
                                                });
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
        });

    // Actions
    let save_action: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| {
        let form_val = form.get();
        let original_id_val = original_id.get();
        let is_new_val = is_new();

        async move {
            let Some(initial_form) = form_val else {
                save_status.set(Some((false, "Form not loaded.".into())));
                return;
            };

            // Ensure form_id is set to original_id if empty (prevents creating new models)
            let save_id = if initial_form.id.trim().is_empty() {
                original_id_val.clone()
            } else {
                initial_form.id.clone()
            };

            let args: Vec<String> = initial_form
                .args
                .lines()
                .map(|l: &str| l.trim().to_string())
                .filter(|l: &String| !l.is_empty())
                .collect();

            let form_data = ModelForm {
                id: save_id,
                backend: initial_form.backend.clone(),
                model: initial_form.model.clone(),
                quant: initial_form.quant.clone(),
                mmproj: initial_form.mmproj.clone(),
                args: initial_form.args.clone(),
                sampling: initial_form.sampling.clone(),
                enabled: initial_form.enabled,
                context_length: initial_form.context_length,
                port: initial_form.port,
                api_name: initial_form.api_name.clone(),
                display_name: initial_form.display_name.clone(),
                gpu_layers: initial_form.gpu_layers,
                quants: initial_form.quants.clone(),
                modalities: initial_form.modalities.clone(),
            };

            let new_id = form_data.id.clone();
            let old_id = original_id_val;

            if old_id != new_id && !old_id.is_empty() {
                match rename_model(&old_id, &new_id).await {
                    Ok(()) => (),
                    Err(e) => {
                        save_status.set(Some((false, format!("Rename failed: {}", e))));
                        return;
                    }
                }
            }

            let form_id = form_data.id.clone();
            match save_model(args, form_data, is_new_val).await {
                Ok(()) => {
                    original_id.set(form_id);
                    save_status.set(Some((true, "Saved.".into())));
                }
                Err(e) => {
                    if old_id != new_id && !old_id.is_empty() {
                        match rename_model(&new_id, &old_id).await {
                            Ok(()) => {
                                original_id.set(old_id.clone());
                                save_status
                                    .set(Some((false, format!("Save failed, rolled back: {}", e))));
                            }
                            Err(rename_err) => {
                                save_status.set(Some((
                                    false,
                                    format!(
                                        "Save failed ({}), and rollback also failed ({})",
                                        e, rename_err
                                    ),
                                )));
                            }
                        }
                    } else {
                        save_status.set(Some((false, format!("Error: {}", e))));
                    }
                }
            }
        }
    });

    let delete_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            let form_opt = form.get();
            let Some(form) = form_opt else {
                save_status.set(Some((false, "Form not loaded.".into())));
                return;
            };
            match delete_model_api(form.id.clone()).await {
                Ok(()) => deleted.set(true),
                Err(e) => save_status.set(Some((false, format!("Delete failed: {}", e)))),
            }
        });

    let delete_quant_action: Action<(String, String), (), LocalStorage> =
        Action::new_unsync(move |(id, key): &(String, String)| {
            let id = id.clone();
            let key = key.clone();
            async move {
                match delete_quant_api(id.clone(), key.clone()).await {
                    Ok(()) => {
                        // Remove from local state on success
                        form.update(|f| {
                            if let Some(form) = f {
                                form.quants.retain(|k, _| k != &key);
                                // Clear form.quant if matching
                                if form.quant.as_deref() == Some(key.as_str()) {
                                    form.quant = None;
                                }
                                // Clear mmproj if matching
                                if form.mmproj.as_deref() == Some(key.as_str()) {
                                    form.mmproj = None;
                                }
                            }
                        });
                        save_status.set(Some((true, "Quant deleted from disk.".into())));
                    }
                    Err(e) => {
                        save_status.set(Some((false, format!("Delete failed: {}", e))));
                    }
                }
            }
        });

    // Merge a list of DB file records back into the `quants` signal, matching
    // on `QuantInfo.file`. Only updates DB-enrichment fields; TOML fields
    // (name, kind, context_length) are left untouched.
    let merge_file_records = move |files: Vec<FileRecordJson>| {
        form.update(|f| {
            if let Some(form) = f {
                for rec in files {
                    for (_name, q) in form.quants.iter_mut() {
                        if q.file == rec.filename {
                            q.lfs_oid = rec.lfs_oid.clone();
                            q.db_size_bytes = rec.size_bytes;
                            // Authoritative size from HF blob metadata — update
                            // the visible size_bytes too, since the editable input
                            // is now read-only.
                            if rec.size_bytes.is_some() {
                                q.size_bytes = rec.size_bytes;
                            }
                            q.last_verified_at = rec.last_verified_at.clone();
                            q.verified_ok = rec.verified_ok;
                            q.verify_error = rec.verify_error.clone();
                            break;
                        }
                    }
                }
            }
        });
    };

    let refresh_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            refresh_busy.set(true);
            refresh_status.set(None);
            // Use the persisted id, not the editable form_id — otherwise
            // mid-rename edits would cause the backend to look up a model
            // that isn't saved yet.
            let persisted = original_id.get();
            let id = if persisted.is_empty() {
                form.get().map(|f| f.id).unwrap_or_default()
            } else {
                persisted
            };
            match refresh_model_api(id).await {
                Ok(resp) => {
                    repo_commit_sha.set(resp.repo_commit_sha.clone());
                    repo_pulled_at.set(resp.repo_pulled_at.clone());
                    let n = resp.files.len();
                    merge_file_records(resp.files);
                    refresh_status.set(Some((
                        true,
                        format!("Refreshed metadata for {} file(s).", n),
                    )));
                }
                Err(e) => {
                    refresh_status.set(Some((false, format!("Refresh failed: {}", e))));
                }
            }
            refresh_busy.set(false);
        });

    let verify_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            verify_busy.set(true);
            verify_status.set(None);
            // Same reasoning as refresh_action: target the saved id.
            let persisted = original_id.get();
            let id = if persisted.is_empty() {
                form.get().map(|f| f.id).unwrap_or_default()
            } else {
                persisted
            };
            match verify_model_api(id).await {
                Ok(resp) => {
                    let n = resp.files.len();
                    merge_file_records(resp.files);
                    let msg = if resp.ok && !resp.any_unknown {
                        format!("All {} file(s) verified successfully.", n)
                    } else if resp.ok {
                        format!("Verified {} file(s) (some without an upstream hash).", n)
                    } else {
                        "Verification failed for one or more files.".to_string()
                    };
                    verify_status.set(Some((resp.ok, msg)));
                }
                Err(e) => {
                    verify_status.set(Some((false, format!("Verify failed: {}", e))));
                }
            }
            verify_busy.set(false);
        });

    // View
    view! {
        <div class="page-header">
            <h1>
                {move || {
                    if is_new() {
                        "New Model".to_string()
                    } else {
                        // Prefer display_name from form, fall back to model_id (which may be integer or config_key)
                        form.get()
                            .and_then(|f| f.display_name.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(model_id)
                    }
                }}
            </h1>
            <div class="page-header-actions">
                <button
                    type="button"
                    class="btn btn-primary"
                    on:click=move |_| { save_action.dispatch(()); }
                >
                    "Save Model"
                </button>
                {move || (!is_new()).then(|| view! {
                    <button
                        type="button"
                        class="btn btn-danger ml-2"
                        on:click=move |_| {
                            let confirmed = web_sys::window()
                                .and_then(|w| w.confirm_with_message("Delete this model and all its files from disk? This cannot be undone.").ok())
                                .unwrap_or(false);
                            if confirmed { delete_action.dispatch(()); }
                        }
                    >"Delete Model"</button>
                })}
                <A href="/models"><button type="button" class="btn btn-secondary btn-sm ml-2">"← Back to Models"</button></A>
            </div>
        </div>

        {move || deleted.get().then(|| view! {
            <div class="alert alert--success mb-3">
                <span class="alert__icon">"✓"</span>
                <span>"Model deleted. " <A href="/models">"← Back to Models"</A></span>
            </div>
        })}

        <Suspense fallback=|| view! {
            <div class="spinner-container">
                <span class="spinner"></span>
                <span class="text-muted">"Loading model..."</span>
            </div>
        }>
            {move || {
                // Use form_ready as the stability gate, NOT form.get().
                // form.get() changes on every keystroke, which would cause
                // the entire layout to unmount/remount and lose input focus.
                form_ready.get().then(|| {
                    view! {
                        <div class="model-editor-layout">
                            // Side navigation
                            <div class="model-editor-nav">
                                <button
                                    class="nav-btn"
                                    class:nav-btn--active=move || active_section.get() == Section::General
                                    on:click=move |_| {
                                        active_section.set(Section::General);
                                        if let Some(el) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.get_element_by_id("section-general"))
                                        {
                                            el.scroll_into_view_with_bool(true);
                                        }
                                    }
                                >
                                    <span class="nav-btn__icon">{Section::General.icon()}</span>
                                    <span class="nav-btn__text">{Section::General.name()}</span>
                                </button>
                                <button
                                    class="nav-btn"
                                    class:nav-btn--active=move || active_section.get() == Section::Sampling
                                    on:click=move |_| {
                                        active_section.set(Section::Sampling);
                                        if let Some(el) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.get_element_by_id("section-sampling"))
                                        {
                                            el.scroll_into_view_with_bool(true);
                                        }
                                    }
                                >
                                    <span class="nav-btn__icon">{Section::Sampling.icon()}</span>
                                    <span class="nav-btn__text">{Section::Sampling.name()}</span>
                                </button>
                                <button
                                    class="nav-btn"
                                    class:nav-btn--active=move || active_section.get() == Section::QuantsVision
                                    on:click=move |_| {
                                        active_section.set(Section::QuantsVision);
                                        if let Some(el) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.get_element_by_id("section-quants"))
                                        {
                                            el.scroll_into_view_with_bool(true);
                                        }
                                    }
                                >
                                    <span class="nav-btn__icon">{Section::QuantsVision.icon()}</span>
                                    <span class="nav-btn__text">{Section::QuantsVision.name()}</span>
                                </button>
                                <button
                                    class="nav-btn"
                                    class:nav-btn--active=move || active_section.get() == Section::ExtraArgs
                                    on:click=move |_| {
                                        active_section.set(Section::ExtraArgs);
                                        if let Some(el) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.get_element_by_id("section-extra-args"))
                                        {
                                            el.scroll_into_view_with_bool(true);
                                        }
                                    }
                                >
                                    <span class="nav-btn__icon">{Section::ExtraArgs.icon()}</span>
                                    <span class="nav-btn__text">{Section::ExtraArgs.name()}</span>
                                </button>
                            </div>

                            // Main content area — all sections visible, stacked
                            <div class="model-editor-main">
                                <div id="section-general" class="card">
                                    <h2 class="card__title">"General"</h2>
                                    <ModelEditorGeneralForm
                                        form=form
                                        backends=backends
                                    />
                                </div>

                                <div id="section-sampling" class="card mt-2">
                                    <h2 class="card__title">"Sampling"</h2>
                                    <ModelEditorSamplingForm
                                        form=form
                                        templates=templates
                                        load_preset_action=load_preset_action
                                    />
                                </div>

                                <div id="section-quants" class="card mt-2">
                                    <h2 class="card__title">"Quants & Vision"</h2>
                                    <ModelEditorQuantsVisionForm
                                        form=form
                                        repo_commit_sha=repo_commit_sha
                                        repo_pulled_at=repo_pulled_at
                                        refresh_busy=refresh_busy
                                        verify_busy=verify_busy
                                        refresh_status=refresh_status
                                        verify_status=verify_status
                                        pull_modal_open_signal=pull_modal_open_signal
                                        delete_quant_action=delete_quant_action
                                        original_id=original_id
                                        refresh_action=refresh_action
                                        verify_action=verify_action
                                    />
                                </div>

                                <div id="section-extra-args" class="card mt-2">
                                    <h2 class="card__title">"Extra Args"</h2>
                                    <ModelEditorExtraArgsForm form=form />
                                </div>

                                {move || save_status.get().map(|(ok, msg)| {
                                    let cls = if ok { "alert alert--success mt-2" } else { "alert alert--error mt-2" };
                                    let icon = if ok { "✓" } else { "✕" };
                                    view! {
                                        <div class=cls>
                                            <span class="alert__icon">{icon}</span>
                                            <span>{msg}</span>
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                })
            }}
        </Suspense>

        <Modal
            open=rw_signal_to_signal(pull_modal_open_signal)
            on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
            title="Pull Quant from HuggingFace".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(move || form.get().map(|f| f.model.unwrap_or_default()).unwrap_or_default())
                is_open=rw_signal_to_signal(pull_modal_open_signal)
                on_complete=Callback::new(move |completed: Vec<CompletedQuant>| {
                    // Visibility for the silent-failure caveat in spec §8.7: if all
                    // quants in this session failed, log to console so the user has
                    // *some* trace after the modal auto-closes.
                    if completed.is_empty() {
                        web_sys::console::warn_1(
                            &"All pulled quants failed; nothing merged into the editor.".into(),
                        );
                    }
                    form.update(|f| {
                        if let Some(form) = f {
                            for cq in completed {
                                // Detect mmproj files by filename pattern (matches
                                // the backend's QuantKind::from_filename logic).
                                let lower = cq.filename.to_lowercase();
                                let kind = if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
                                    QuantKind::Mmproj
                                } else {
                                    QuantKind::Model
                                };
                                let key = cq.quant.clone().unwrap_or_else(|| {
                                    // Infer quant from filename: try standard patterns first,
                                    // otherwise use last component after splitting by `-` or `_`
                                    let stem = cq.filename.trim_end_matches(".gguf");
                                    let quant_patterns = [
                                        "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S",
                                        "IQ2_M", "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL",
                                        "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
                                        "Q4_K_L", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q2_K_XL", "Q3_K_XL",
                                        "Q4_K_XL", "Q5_K_XL", "Q6_K_XL", "Q8_K_XL", "Q2_K", "Q3_K",
                                        "Q4_K", "Q5_K", "Q6_K", "Q4_0", "Q4_1", "Q5_0", "Q5_1",
                                        "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
                                    ];
                                    let stem_upper = stem.to_uppercase();
                                    let quant = quant_patterns.iter().find(|pattern| {
                                        stem_upper.ends_with(*pattern)
                                            || stem_upper.contains(&format!("-{}", pattern))
                                            || stem_upper.contains(&format!(".{}", pattern))
                                            || stem_upper.contains(&format!("_{}", pattern))
                                    }).map(|s| s.to_string());
                                    quant.unwrap_or_else(|| {
                                        stem.split(|c: char| ['-', '_'].contains(&c))
                                            .next_back()
                                            .unwrap_or("unknown")
                                            .to_string()
                                    })
                                });
                                if let Some(pos) = form.quants.iter().position(|(k, _)| k == &key) {
                                    // Re-pull: overwrite filename and context_length
                                    // (the wizard's values reflect the user's latest intent).
                                    // Only overwrite size_bytes when we have a value —
                                    // never clobber a known size with None.
                                    let row = &mut form.quants.values_mut().nth(pos).unwrap();
                                    row.file = cq.filename;
                                    row.kind = kind;
                                    // Only set context_length for model quants;
                                    // mmprojs don't use it.
                                    if kind == QuantKind::Model {
                                        row.context_length = Some(cq.context_length);
                                    }
                                    if cq.size_bytes.is_some() {
                                        row.size_bytes = cq.size_bytes;
                                    }
                                } else {
                                    // New row.
                                    form.quants.insert(key, QuantInfo {
                                        file: cq.filename,
                                        kind,
                                        size_bytes: cq.size_bytes,
                                        context_length: if kind == QuantKind::Model {
                                            Some(cq.context_length)
                                        } else {
                                            None
                                        },
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    });
                    pull_modal_open_signal.set(false);
                })
                on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
            />
        </Modal>
    }
}
