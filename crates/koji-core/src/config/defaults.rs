// Profile resolution is now handled via Config.sampling_templates
// and ModelCard.sampling directly. See resolve.rs.

use crate::config::types::General;

pub fn default_log_level() -> String {
    "info".to_string()
}

pub fn default_update_check_interval() -> u32 {
    12
}

impl Default for General {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: default_update_check_interval(),
        }
    }
}
