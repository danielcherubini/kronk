/// A card component for selecting a Docker template.
use leptos::prelude::*;

use tama_core::backends::docker::{available_templates, Template};

/// A card component for selecting a Docker template.
#[component]
pub fn DockerTemplateCard(template: Template, on_select: Callback<String>) -> impl IntoView {
    let on_click = move |_| {
        on_select.run(template.compose_yaml.to_string());
    };

    view! {
        <div
            class="docker-template-card"
            on:click=on_click
            style="cursor: pointer; border: 1px solid #e0e0e0; border-radius: 8px; padding: 16px; margin: 8px; transition: box-shadow 0.2s;"
        >
            <h3 style="margin: 0 0 8px 0; font-size: 1.1em;">{template.name}</h3>
            <p style="margin: 0 0 8px 0; color: #666; font-size: 0.9em;">{template.description}</p>
            <span style="font-size: 0.85em; color: #888;">"Default port: " {template.default_port}</span>
        </div>
    }
}

#[component]
pub fn DockerTemplateGrid(on_select: Callback<String>) -> impl IntoView {
    let templates = available_templates();

    view! {
        <div class="docker-template-grid" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(250px, 1fr)); gap: 12px;">
            {templates.iter().map(|t| {
                let sel = on_select.clone();
                let tmpl = t.clone();
                view! {
                    <DockerTemplateCard template=tmpl on_select=sel />
                }.into_view()
            }).collect_view()}
        </div>
    }
}
