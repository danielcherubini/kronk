#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// General configuration section component
#[component]
#[allow(dead_code)]
pub fn GeneralSection(general: ReadSignal<crate::types::config::General>) -> impl IntoView {
    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"General Settings"</h2>
            <p class="text-gray-600">"Configure global Koji settings."</p>

            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Log Level */}
                <div>
                    <label for="log_level" class="block text-sm font-medium text-gray-700 mb-1">
                        "Log Level"
                    </label>
                    <select
                        id="log_level"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    >
                        <option value="trace" selected={general.get().log_level == "trace"}>"Trace"</option>
                        <option value="debug" selected={general.get().log_level == "debug"}>"Debug"</option>
                        <option value="info" selected={general.get().log_level == "info"}>"Info"</option>
                        <option value="warn" selected={general.get().log_level == "warn"}>"Warn"</option>
                        <option value="error" selected={general.get().log_level == "error"}>"Error"</option>
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
                        value={general.get().models_dir.clone().unwrap_or_default()}
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
                        value={general.get().logs_dir.clone().unwrap_or_default()}
                        placeholder="/path/to/logs"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Directory where log files are stored."</p>
                </div>
            </div>
        </section>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    #[test]
    fn test_general_section_component_creation() {
        let general = crate::types::config::General {
            log_level: "info".to_string(),
            models_dir: Some("/models".to_string()),
            logs_dir: None,
        };
        assert_eq!(general.log_level, "info");
        assert_eq!(general.models_dir, Some("/models".to_string()));
        assert_eq!(general.logs_dir, None);
    }

    #[test]
    fn test_general_section_default() {
        let general = crate::types::config::General::default();
        assert_eq!(general.log_level, "");
        assert_eq!(general.models_dir, None);
        assert_eq!(general.logs_dir, None);
    }
}
