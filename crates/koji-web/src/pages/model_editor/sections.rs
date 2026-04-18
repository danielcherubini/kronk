// ── Section Navigation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Sampling,
    QuantsVision,
    ExtraArgs,
}

impl Section {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Sampling => "Sampling",
            Self::QuantsVision => "Quants & Vision",
            Self::ExtraArgs => "Extra Args",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::General => "⚙️",
            Self::Sampling => "🎲",
            Self::QuantsVision => "📊 👁️",
            Self::ExtraArgs => "📝",
        }
    }
}
