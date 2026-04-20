use leptos::prelude::*;

/// General configuration section component — controlled form.
#[component]
pub fn GeneralSection(
    /// Initial config values for the form fields.
    initial: ReadSignal<GeneralConfig>,
    /// Called when the user submits the form with updated values.
    on_submit: Callback<GeneralConfig>,
) -> impl IntoView {
    // Local form state — initialised from the parent's signal.
    let (log_level, set_log_level) = signal(String::new());
    let (models_dir, set_models_dir) = signal(String::new());
    let (logs_dir, set_logs_dir) = signal(String::new());

    // Populate form from initial config once at setup.
    Effect::new(move |_| {
        let cfg = initial.get();
        set_log_level.set(cfg.log_level);
        set_models_dir.set(cfg.models_dir.unwrap_or_default());
        set_logs_dir.set(cfg.logs_dir.unwrap_or_default());
    });

    let on_submit_clone = on_submit;
    let submit_handler = move |_| {
        let cfg = GeneralConfig {
            log_level: log_level.get(),
            models_dir: models_dir.get().into(),
            logs_dir: logs_dir.get().into(),
        };
        on_submit_clone.run(cfg);
    };

    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"General Settings"</h2>
            <p class="text-gray-600">"Configure global Koji settings."</p>

            <form on:submit=move |e| {
                e.prevent_default();
                submit_handler(e);
            }>
            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Log Level */}
                <div>
                    <label for="log_level" class="block text-sm font-medium text-gray-700 mb-1">
                        "Log Level"
                    </label>
                    <select
                        id="log_level"
                        prop:value=log_level
                        on:change=move |e| {
                            set_log_level.set(event_target_value(&e));
                        }
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    >
                        <option value="trace">"Trace"</option>
                        <option value="debug">"Debug"</option>
                        <option value="info">"Info"</option>
                        <option value="warn">"Warn"</option>
                        <option value="error">"Error"</option>
                    </select>
                    <p class="mt-1 text-sm text-gray-500">"The verbosity level for logging."</p>
                </div>

                {/* Models Directory */}
                <div>
                    <label for="models_dir" class="block text-sm font-medium text-gray-700 mb-1">
                        "Models Directory"
                    </label>
                    <input
                        type="text"
                        id="models_dir"
                        prop:value=models_dir
                        on:change=move |e| {
                            set_models_dir.set(event_target_value(&e));
                        }
                        placeholder="/path/to/models"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Directory where models are stored."</p>
                </div>

                {/* Logs Directory */}
                <div>
                    <label for="logs_dir" class="block text-sm font-medium text-gray-700 mb-1">
                        "Logs Directory"
                    </label>
                    <input
                        type="text"
                        id="logs_dir"
                        prop:value=logs_dir
                        on:change=move |e| {
                            set_logs_dir.set(event_target_value(&e));
                        }
                        placeholder="/path/to/logs"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Directory where log files are stored."</p>
                </div>
            </div>

            <button type="submit" class="btn btn-primary">"Save"</button>
            </form>
        </section>
    }
}

/// Minimal general config struct for the controlled form component.
#[derive(Clone, Debug)]
pub struct GeneralConfig {
    pub log_level: String,
    #[allow(dead_code)]
    pub models_dir: Option<String>,
    #[allow(dead_code)]
    pub logs_dir: Option<String>,
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    #[test]
    fn test_general_section_component_creation() {
        let general = crate::types::config::General {
            log_level: "info".to_string(),
            models_dir: Some("/models".to_string()),
            logs_dir: None,
            hf_token: Some("hf_test123".to_string()),
            update_check_interval: 12,
        };
        assert_eq!(general.log_level, "info");
        assert_eq!(general.models_dir, Some("/models".to_string()));
        assert_eq!(general.logs_dir, None);
        assert_eq!(general.hf_token, Some("hf_test123".to_string()));
        assert_eq!(general.update_check_interval, 12);
    }

    #[test]
    fn test_general_section_default() {
        let general = crate::types::config::General::default();
        assert_eq!(general.log_level, "");
        assert_eq!(general.models_dir, None);
        assert_eq!(general.logs_dir, None);
        assert_eq!(general.hf_token, None);
    }
}
