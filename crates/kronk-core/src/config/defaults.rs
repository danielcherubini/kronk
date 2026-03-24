use super::types::Config;

/// Resolve profile params from sampling_templates.
/// Returns `Option<SamplingParams>` for the given profile name.
pub fn resolve_profile_params(
    config: &Config,
    profile: &Option<crate::profiles::Profile>,
) -> Option<crate::profiles::SamplingParams> {
    match profile {
        Some(profile) => config.sampling_templates.get(profile).cloned(),
        None => None,
    }
}
