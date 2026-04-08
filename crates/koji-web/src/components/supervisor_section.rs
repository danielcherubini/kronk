#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// Supervisor configuration section component
#[component]
#[allow(dead_code)]
pub fn SupervisorSection(
    supervisor: ReadSignal<crate::types::config::Supervisor>,
) -> impl IntoView {
    let sup = supervisor.get();

    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"Supervisor Settings"</h2>
            <p class="text-gray-600">"Configure model supervisor and health monitoring."</p>

            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Restart Policy */}
                <div>
                    <label for="restart_policy" class="block text-sm font-medium text-gray-700 mb-1">
                        "Restart Policy"
                    </label>
                    <select
                        id="restart_policy"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    >
                        <option value="never" selected={sup.restart_policy == "never"}>"Never"</option>
                        <option value="on-failure" selected={sup.restart_policy == "on-failure"}>"On Failure"</option>
                        <option value="always" selected={sup.restart_policy == "always"}>"Always"</option>
                    </select>
                    <p class="mt-1 text-sm text-gray-500">"When to restart the model server."</p>
                </div>

                {/* Max Restarts */}
                <div>
                    <label for="max_restarts" class="block text-sm font-medium text-gray-700 mb-1">
                        "Max Restarts"
                    </label>
                    <input
                        type="number"
                        id="max_restarts"
                        value={sup.max_restarts.to_string()}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Maximum number of restart attempts."</p>
                </div>

                {/* Restart Delay */}
                <div>
                    <label for="restart_delay_ms" class="block text-sm font-medium text-gray-700 mb-1">
                        "Restart Delay (ms)"
                    </label>
                    <input
                        type="number"
                        id="restart_delay_ms"
                        value={sup.restart_delay_ms.to_string()}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Delay between restart attempts in milliseconds."</p>
                </div>

                {/* Health Check Interval */}
                <div>
                    <label for="health_check_interval_ms" class="block text-sm font-medium text-gray-700 mb-1">
                        "Health Check Interval (ms)"
                    </label>
                    <input
                        type="number"
                        id="health_check_interval_ms"
                        value={sup.health_check_interval_ms.to_string()}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Interval between health checks in milliseconds."</p>
                </div>

                {/* Health Check Timeout */}
                <div>
                    <label for="health_check_timeout_ms" class="block text-sm font-medium text-gray-700 mb-1">
                        "Health Check Timeout (ms)"
                    </label>
                    <input
                        type="number"
                        id="health_check_timeout_ms"
                        value={sup.health_check_timeout_ms.to_string()}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Timeout for each health check in milliseconds."</p>
                </div>

                {/* Health Check Retries */}
                <div>
                    <label for="health_check_retries" class="block text-sm font-medium text-gray-700 mb-1">
                        "Health Check Retries"
                    </label>
                    <input
                        type="number"
                        id="health_check_retries"
                        value={sup.health_check_retries.to_string()}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Number of retry attempts before marking as unhealthy."</p>
                </div>
            </div>
        </section>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    #[test]
    fn test_supervisor_config_defaults() {
        let supervisor = crate::types::config::Supervisor::default();
        // All fields should have default values (unsigned types are always >= 0)
        assert_eq!(supervisor.max_restarts, 0);
        assert_eq!(supervisor.restart_delay_ms, 0);
        assert_eq!(supervisor.health_check_interval_ms, 0);
        assert_eq!(supervisor.health_check_timeout_ms, 0);
        assert_eq!(supervisor.health_check_retries, 0);
    }
}
