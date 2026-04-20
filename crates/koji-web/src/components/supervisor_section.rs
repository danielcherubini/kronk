use leptos::prelude::*;

/// Supervisor configuration section component — controlled form.
#[component]
pub fn SupervisorSection(
    /// Initial config values for the form fields.
    initial: ReadSignal<SupervisorConfig>,
    /// Called when the user submits the form with updated values.
    on_submit: Callback<SupervisorConfig>,
) -> impl IntoView {
    // Local form state — initialised from the parent's signal.
    let (restart_policy, set_restart_policy) = signal(String::new());
    let (max_restarts, set_max_restarts) = signal(u32::MAX.to_string());
    let (restart_delay_ms, set_restart_delay_ms) = signal(String::new());
    let (health_check_interval_ms, set_health_check_interval_ms) = signal(String::new());
    let (health_check_timeout_ms, set_health_check_timeout_ms) = signal(String::new());
    let (health_check_retries, set_health_check_retries) = signal(u32::MAX.to_string());
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);

    // Populate form from initial config once at setup.
    Effect::new(move |_| {
        let sup = initial.get();
        set_restart_policy.set(sup.restart_policy);
        set_max_restarts.set(sup.max_restarts.to_string());
        set_restart_delay_ms.set(sup.restart_delay_ms.to_string());
        set_health_check_interval_ms.set(sup.health_check_interval_ms.to_string());
        set_health_check_timeout_ms.set(sup.health_check_timeout_ms.to_string());
        set_health_check_retries.set(sup.health_check_retries.to_string());
    });

    let on_submit_clone = on_submit;
    let submit_handler = move |_| {
        // Validate numeric fields before submission — reject empty or invalid values.
        let max_restarts_val: u32 = match max_restarts.get().parse() {
            Ok(v) => v,
            Err(_) => {
                set_error_msg.set(Some("Max Restarts must be a valid number.".to_string()));
                return;
            }
        };
        let restart_delay_ms_val: u64 = match restart_delay_ms.get().parse() {
            Ok(v) => v,
            Err(_) => {
                set_error_msg.set(Some("Restart Delay must be a valid number.".to_string()));
                return;
            }
        };
        let health_check_interval_ms_val: u64 = match health_check_interval_ms.get().parse() {
            Ok(v) => v,
            Err(_) => {
                set_error_msg.set(Some(
                    "Health Check Interval must be a valid number.".to_string(),
                ));
                return;
            }
        };
        let health_check_timeout_ms_val: u64 = match health_check_timeout_ms.get().parse() {
            Ok(v) => v,
            Err(_) => {
                set_error_msg.set(Some(
                    "Health Check Timeout must be a valid number.".to_string(),
                ));
                return;
            }
        };
        let health_check_retries_val: u32 = match health_check_retries.get().parse() {
            Ok(v) => v,
            Err(_) => {
                set_error_msg.set(Some(
                    "Health Check Retries must be a valid number.".to_string(),
                ));
                return;
            }
        };

        // Clear any previous error before submitting.
        set_error_msg.set(None);

        let cfg = SupervisorConfig {
            restart_policy: restart_policy.get(),
            max_restarts: max_restarts_val,
            restart_delay_ms: restart_delay_ms_val,
            health_check_interval_ms: health_check_interval_ms_val,
            health_check_timeout_ms: health_check_timeout_ms_val,
            health_check_retries: health_check_retries_val,
        };
        on_submit_clone.run(cfg);
    };

    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"Supervisor Settings"</h2>
            <p class="text-gray-600">"Configure model supervisor and health monitoring."</p>

            <form on:submit=move |e| {
                e.prevent_default();
                submit_handler(e);
            }>
            {move || error_msg.get().map(|e| {
                view! { <div class="alert alert-error">{e.to_string()}</div> }
            })}

            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Restart Policy */}
                <div>
                    <label for="restart_policy" class="block text-sm font-medium text-gray-700 mb-1">
                        "Restart Policy"
                    </label>
                    <select
                        id="restart_policy"
                        prop:value=restart_policy
                        on:change=move |e| {
                            set_restart_policy.set(event_target_value(&e));
                        }
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    >
                        <option value="never">"Never"</option>
                        <option value="on-failure">"On Failure"</option>
                        <option value="always">"Always"</option>
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
                        prop:value=max_restarts
                        on:change=move |e| {
                            set_max_restarts.set(event_target_value(&e));
                        }
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
                        prop:value=restart_delay_ms
                        on:change=move |e| {
                            set_restart_delay_ms.set(event_target_value(&e));
                        }
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
                        prop:value=health_check_interval_ms
                        on:change=move |e| {
                            set_health_check_interval_ms.set(event_target_value(&e));
                        }
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
                        prop:value=health_check_timeout_ms
                        on:change=move |e| {
                            set_health_check_timeout_ms.set(event_target_value(&e));
                        }
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
                        prop:value=health_check_retries
                        on:change=move |e| {
                            set_health_check_retries.set(event_target_value(&e));
                        }
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Number of retry attempts before marking as unhealthy."</p>
                </div>
            </div>

            <button type="submit" class="btn btn-primary">"Save"</button>
            </form>
        </section>
    }
}

/// Minimal supervisor config struct for the controlled form component.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    pub restart_policy: String,
    #[allow(dead_code)]
    pub max_restarts: u32,
    #[allow(dead_code)]
    pub restart_delay_ms: u64,
    #[allow(dead_code)]
    pub health_check_interval_ms: u64,
    #[allow(dead_code)]
    pub health_check_timeout_ms: u64,
    #[allow(dead_code)]
    pub health_check_retries: u32,
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
