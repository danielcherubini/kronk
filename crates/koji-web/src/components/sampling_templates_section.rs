#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// Sampling templates configuration section component
#[component]
#[allow(dead_code)]
pub fn SamplingTemplatesSection(
    sampling_params: ReadSignal<crate::types::config::SamplingParams>,
) -> impl IntoView {
    let params = sampling_params.get();

    view! {
        <section class="space-y-6">
            <h2 class="text-2xl font-bold text-gray-900">"Sampling Templates"</h2>
            <p class="text-gray-600">"Configure model sampling parameters and templates."</p>

            <div class="bg-white rounded-lg shadow border border-gray-200 p-6 space-y-4">
                {/* Temperature */}
                <div>
                    <label for="temperature" class="block text-sm font-medium text-gray-700 mb-1">
                        "Temperature"
                    </label>
                    <input
                        type="number"
                        id="temperature"
                        value={params.temperature.map_or(String::new(), |v| v.to_string())}
                        step="0.1"
                        min="0.0"
                        max="2.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Controls randomness. Higher values make output more random."</p>
                </div>

                {/* Top K */}
                <div>
                    <label for="top_k" class="block text-sm font-medium text-gray-700 mb-1">
                        "Top K"
                    </label>
                    <input
                        type="number"
                        id="top_k"
                        value={params.top_k.map_or(String::new(), |v| v.to_string())}
                        min="0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Limits sampling to top K tokens. 0 = disabled."</p>
                </div>

                {/* Top P */}
                <div>
                    <label for="top_p" class="block text-sm font-medium text-gray-700 mb-1">
                        "Top P"
                    </label>
                    <input
                        type="number"
                        id="top_p"
                        value={params.top_p.map_or(String::new(), |v| v.to_string())}
                        step="0.01"
                        min="0.0"
                        max="1.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Nucleus sampling. Limits to tokens with cumulative probability P."</p>
                </div>

                {/* Min P */}
                <div>
                    <label for="min_p" class="block text-sm font-medium text-gray-700 mb-1">
                        "Min P"
                    </label>
                    <input
                        type="number"
                        id="min_p"
                        value={params.min_p.map_or(String::new(), |v| v.to_string())}
                        step="0.01"
                        min="0.0"
                        max="1.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Minimum probability threshold for token selection."</p>
                </div>

                {/* Presence Penalty */}
                <div>
                    <label for="presence_penalty" class="block text-sm font-medium text-gray-700 mb-1">
                        "Presence Penalty"
                    </label>
                    <input
                        type="number"
                        id="presence_penalty"
                        value={params.presence_penalty.map_or(String::new(), |v| v.to_string())}
                        step="0.1"
                        min="-2.0"
                        max="2.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Penalize repetition of tokens regardless of frequency."</p>
                </div>

                {/* Frequency Penalty */}
                <div>
                    <label for="frequency_penalty" class="block text-sm font-medium text-gray-700 mb-1">
                        "Frequency Penalty"
                    </label>
                    <input
                        type="number"
                        id="frequency_penalty"
                        value={params.frequency_penalty.map_or(String::new(), |v| v.to_string())}
                        step="0.1"
                        min="-2.0"
                        max="2.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Penalize tokens based on their frequency in the output."</p>
                </div>

                {/* Repeat Penalty */}
                <div>
                    <label for="repeat_penalty" class="block text-sm font-medium text-gray-700 mb-1">
                        "Repeat Penalty"
                    </label>
                    <input
                        type="number"
                        id="repeat_penalty"
                        value={params.repeat_penalty.map_or(String::new(), |v| v.to_string())}
                        step="0.1"
                        min="0.5"
                        max="2.0"
                        class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                    />
                    <p class="mt-1 text-sm text-gray-500">"Penalty for repeating tokens in the output."</p>
                </div>
            </div>
        </section>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    #[test]
    fn test_sampling_params_defaults() {
        let params = crate::types::config::SamplingParams::default();
        assert!(params.temperature.is_none());
        assert!(params.top_k.is_none());
        assert!(params.top_p.is_none());
        assert!(params.min_p.is_none());
        assert!(params.presence_penalty.is_none());
        assert!(params.frequency_penalty.is_none());
        assert!(params.repeat_penalty.is_none());
    }
}
