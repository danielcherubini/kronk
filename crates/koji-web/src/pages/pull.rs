use crate::components::pull_quant_wizard::PullQuantWizard;
use leptos::prelude::*;

#[component]
pub fn Pull() -> impl IntoView {
    view! {
        <div class="page-header">
            <h1>"Pull Model"</h1>
        </div>
        <div class="form-card card">
            <PullQuantWizard
                initial_repo=Signal::derive(String::new)
            />
        </div>
    }
}
