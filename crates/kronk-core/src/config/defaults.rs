use super::types::Config;

/// Shared helper to resolve profile params from custom_profiles, profiles.d/, or built-in.
/// Returns Option<SamplingParams> by checking:
/// 1. self.custom_profiles for the profile name
/// 2. profiles.d/ via crate::profiles::load_profiles_d
/// 3. Profile::params() for built-in profiles
pub fn resolve_profile_params(
    config: &Config,
    profile: &Option<crate::profiles::Profile>,
) -> Option<crate::profiles::SamplingParams> {
    match profile {
        Some(crate::profiles::Profile::Custom { name }) => {
            // Look up custom profile in config, then profiles.d/
            config
                .custom_profiles
                .as_ref()
                .and_then(|m| m.get(name))
                .cloned()
                .or_else(|| {
                    config
                        .profiles_dir()
                        .ok()
                        .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                        .and_then(|map| map.get(name).cloned())
                })
        }
        Some(profile) => {
            // Try profiles.d/ first, fall back to built-in
            let from_disk = config
                .profiles_dir()
                .ok()
                .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                .and_then(|map| map.get(&profile.to_string()).cloned());
            from_disk.or_else(|| Some(profile.params()))
        }
        None => None,
    }
}
