#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// Main configuration editor page - placeholder for section-based UI
#[component]
pub fn ConfigEditor() -> impl IntoView {
    view! {
        <div class="space-y-6">
            <h1 class="text-3xl font-bold text-gray-900">"Configuration"</h1>
            <p class="text-gray-600">"Configuration editor - section components available in crates/koji-web/src/components/"</p>

            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                <div class="bg-white rounded-lg shadow border border-gray-200 p-4">
                    <h2 class="text-lg font-semibold text-gray-900 mb-2">"General Section"</h2>
                    <p class="text-sm text-gray-500">"Log level, models directory, logs directory"</p>
                </div>
                <div class="bg-white rounded-lg shadow border border-gray-200 p-4">
                    <h2 class="text-lg font-semibold text-gray-900 mb-2">"Proxy Section"</h2>
                    <p class="text-sm text-gray-500">"Proxy configuration"</p>
                </div>
                <div class="bg-white rounded-lg shadow border border-gray-200 p-4">
                    <h2 class="text-lg font-semibold text-gray-900 mb-2">"Backends Section"</h2>
                    <p class="text-sm text-gray-500">"Backend configuration"</p>
                </div>
                <div class="bg-white rounded-lg shadow border border-gray-200 p-4">
                    <h2 class="text-lg font-semibold text-gray-900 mb-2">"Supervisor Section"</h2>
                    <p class="text-sm text-gray-500">"Model supervisor and health monitoring"</p>
                </div>
                <div class="bg-white rounded-lg shadow border border-gray-200 p-4">
                    <h2 class="text-lg font-semibold text-gray-900 mb-2">"Sampling Templates Section"</h2>
                    <p class="text-sm text-gray-500">"Model sampling parameters"</p>
                </div>
            </div>
        </div>
    }
}
