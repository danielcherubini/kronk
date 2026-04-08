#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// Backends configuration section component
#[component]
#[allow(dead_code)]
pub fn BackendsSection(backends: ReadSignal<crate::types::config::BackendConfig>) -> impl IntoView {
    let backend = backends.get();
    let path = backend.path.clone().unwrap_or_default();
    let health_check_url = backend.health_check_url.clone().unwrap_or_default();
    let version = backend.version.clone().unwrap_or_default();
    let default_args = backend.default_args.join(", ");

    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"Backend Settings"</h2>
            <p class="text-gray-600">"Configure model backend settings."</p>

            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Backend Path */}
                <div>
                    <label for="backend_path" class="block text-sm font-medium text-gray-700 mb-1">
                        "Backend Path"
                    </label>
                    <input
                        type="text"
                        id="backend_path"
                        value={path}
                        placeholder="/path/to/backend"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Path to the backend executable or service."</p>
                </div>

                {/* Health Check URL */}
                <div>
                    <label for="health_check_url" class="block text-sm font-medium text-gray-700 mb-1">
                        "Health Check URL"
                    </label>
                    <input
                        type="text"
                        id="health_check_url"
                        value={health_check_url}
                        placeholder="http://localhost:8080/health"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"URL for health check endpoint."</p>
                </div>

                {/* Version */}
                <div>
                    <label for="version" class="block text-sm font-medium text-gray-700 mb-1">
                        "Version"
                    </label>
                    <input
                        type="text"
                        id="version"
                        value={version}
                        placeholder="v1.0.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Optional version pin for the backend."</p>
                </div>

                {/* Default Args */}
                <div>
                    <label for="default_args" class="block text-sm font-medium text-gray-700 mb-1">
                        "Default Arguments"
                    </label>
                    <input
                        type="text"
                        id="default_args"
                        value={default_args}
                        placeholder="--gpu-layers 35 --threads 4"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Space-separated default arguments for the backend."</p>
                </div>
            </div>

            {/* Models List */}
            <div class="bg-white rounded-lg shadow border border-gray-200 p-6">
                <h3 class="text-lg font-semibold text-gray-900 mb-4">"Configured Models"</h3>
                <p class="text-sm text-gray-500">"No models configured yet."</p>
            </div>
        </section>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    #[test]
    fn test_backend_config_defaults() {
        let backend = crate::types::config::BackendConfig::default();
        assert!(backend.path.is_none());
        assert!(backend.default_args.is_empty());
        assert!(backend.health_check_url.is_none());
        assert!(backend.version.is_none());
    }
}
