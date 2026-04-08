use leptos::prelude::*;
use leptos_router::components::A;

/// Configuration page section identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSection {
    General,
    Proxy,
    Backends,
    Supervisor,
    Sampling,
}

impl ConfigSection {
    /// Get the section identifier from a string slice
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "general" => Some(ConfigSection::General),
            "proxy" => Some(ConfigSection::Proxy),
            "backends" => Some(ConfigSection::Backends),
            "supervisor" => Some(ConfigSection::Supervisor),
            "sampling" => Some(ConfigSection::Sampling),
            _ => None,
        }
    }

    /// Get the URL path for this section
    pub fn path(&self) -> &'static str {
        match self {
            ConfigSection::General => "/config/general",
            ConfigSection::Proxy => "/config/proxy",
            ConfigSection::Backends => "/config/backends",
            ConfigSection::Supervisor => "/config/supervisor",
            ConfigSection::Sampling => "/config/sampling",
        }
    }

    /// Get the display name for this section
    pub fn name(&self) -> &'static str {
        match self {
            ConfigSection::General => "General",
            ConfigSection::Proxy => "Proxy",
            ConfigSection::Backends => "Backends",
            ConfigSection::Supervisor => "Supervisor",
            ConfigSection::Sampling => "Sampling Templates",
        }
    }

    /// Get the icon for this section
    pub fn icon(&self) -> &'static str {
        match self {
            ConfigSection::General => "⚙️",
            ConfigSection::Proxy => "🌐",
            ConfigSection::Backends => "🖥️",
            ConfigSection::Supervisor => "👀",
            ConfigSection::Sampling => "🎲",
        }
    }
}

/// Configuration page side navigation component
#[component]
#[allow(dead_code)]
pub fn ConfigNav(#[prop(into)] current_section: Option<ConfigSection>) -> impl IntoView {
    let sections = vec![
        ConfigSection::General,
        ConfigSection::Proxy,
        ConfigSection::Backends,
        ConfigSection::Supervisor,
        ConfigSection::Sampling,
    ];

    view! {
        <nav class="w-64 bg-white border-r border-gray-200 min-h-screen p-4">
            <h2 class="text-lg font-semibold text-gray-900 mb-4">"Configuration"</h2>
            <ul class="space-y-1">
                {
                   sections.into_iter().map(|section| {
                        let is_active = current_section == Some(section);
                        let icon = section.icon();
                        let name = section.name();
                        let path = section.path();
                        let base_class = "flex items-center gap-3 px-3 py-2 rounded-lg text-sm font-medium";
                        let active_class = if is_active {
                            "bg-blue-50 text-blue-700"
                        } else {
                            "text-gray-700 hover:bg-gray-50 hover:text-gray-900"
                        };
                        let full_class = format!("{} {}", base_class, active_class);

                       view! {
                            <li>
                                <A
                                    href={path}
                                    attr:class={full_class}
                                >
                                    <span class="text-xl">{icon}</span>
                                    <span>{name}</span>
                                </A>
                            </li>
                        }
                    }).collect::<Vec<_>>()
                }
            </ul>
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_section_from_str() {
        assert_eq!(
            ConfigSection::from_str("general"),
            Some(ConfigSection::General)
        );
        assert_eq!(ConfigSection::from_str("proxy"), Some(ConfigSection::Proxy));
        assert_eq!(
            ConfigSection::from_str("backends"),
            Some(ConfigSection::Backends)
        );
        assert_eq!(
            ConfigSection::from_str("supervisor"),
            Some(ConfigSection::Supervisor)
        );
        assert_eq!(
            ConfigSection::from_str("sampling"),
            Some(ConfigSection::Sampling)
        );
        assert_eq!(ConfigSection::from_str("invalid"), None);
    }

    #[test]
    fn test_config_section_path() {
        assert_eq!(ConfigSection::General.path(), "/config/general");
        assert_eq!(ConfigSection::Proxy.path(), "/config/proxy");
        assert_eq!(ConfigSection::Backends.path(), "/config/backends");
        assert_eq!(ConfigSection::Supervisor.path(), "/config/supervisor");
        assert_eq!(ConfigSection::Sampling.path(), "/config/sampling");
    }

    #[test]
    fn test_config_section_name() {
        assert_eq!(ConfigSection::General.name(), "General");
        assert_eq!(ConfigSection::Proxy.name(), "Proxy");
        assert_eq!(ConfigSection::Backends.name(), "Backends");
        assert_eq!(ConfigSection::Supervisor.name(), "Supervisor");
        assert_eq!(ConfigSection::Sampling.name(), "Sampling Templates");
    }

    #[test]
    fn test_config_section_icon() {
        assert_eq!(ConfigSection::General.icon(), "⚙️");
        assert_eq!(ConfigSection::Proxy.icon(), "🌐");
        assert_eq!(ConfigSection::Backends.icon(), "🖥️");
        assert_eq!(ConfigSection::Supervisor.icon(), "👀");
        assert_eq!(ConfigSection::Sampling.icon(), "🎲");
    }

    #[test]
    fn test_config_section_display_name_order() {
        // Verify sections are in logical order
        assert_eq!(ConfigSection::General.name(), "General");
        assert_eq!(ConfigSection::Proxy.name(), "Proxy");
        assert_eq!(ConfigSection::Backends.name(), "Backends");
        assert_eq!(ConfigSection::Supervisor.name(), "Supervisor");
        assert_eq!(ConfigSection::Sampling.name(), "Sampling Templates");
    }
}
