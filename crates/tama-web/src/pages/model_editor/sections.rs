// ── Section Navigation ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Sampling,
    SpecDecoding,
    QuantsVision,
    ExtraArgs,
}

impl Section {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Sampling => "Sampling",
            Self::SpecDecoding => "Spec Decoding",
            Self::QuantsVision => "Quants & Vision",
            Self::ExtraArgs => "Extra Args",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::General => "⚙️",
            Self::Sampling => "🎲",
            Self::SpecDecoding => "⚡",
            Self::QuantsVision => "📊 👁️",
            Self::ExtraArgs => "📝",
        }
    }
}
