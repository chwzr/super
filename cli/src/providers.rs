pub struct ProviderEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    pub haiku: &'static str,
    pub sonnet: &'static str,
    pub opus: &'static str,
}

pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "anthropic",
        display_name: "Anthropic",
        haiku:  "anthropic/claude-haiku-4-5-20251001",
        sonnet: "anthropic/claude-sonnet-4-6",
        opus:   "anthropic/claude-opus-4-7",
    },
    ProviderEntry {
        id: "z-ai",
        display_name: "Z.ai",
        haiku:  "z-ai/glm-4.7-flash",
        sonnet: "z-ai/glm-5-turbo",
        opus:   "z-ai/glm-5.1",
    },
    ProviderEntry {
        id: "moonshot",
        display_name: "Moonshot",
        haiku:  "moonshotai/kimi-k2.5",
        sonnet: "moonshotai/kimi-k2.5",
        opus:   "moonshotai/kimi-k2.6",
    },
    ProviderEntry {
        id: "deepseek",
        display_name: "Deepseek",
        haiku:  "deepseek/deepseek-v4-flash",
        sonnet: "deepseek/deepseek-v4-flash",
        opus:   "deepseek/deepseek-v4-pro",
    },
    ProviderEntry {
        id: "free",
        display_name: "Free",
        haiku:  "openrouter/free",
        sonnet: "openrouter/free",
        opus:   "openrouter/free",
    },
];

/// Returns the OpenRouter slug for the given provider id + class ("haiku"/"sonnet"/"opus").
/// Falls back to Anthropic sonnet if the provider or class is unrecognised.
pub fn resolve_slug(provider: &str, class: &str) -> &'static str {
    let entry = PROVIDERS.iter().find(|p| p.id == provider)
        .unwrap_or(&PROVIDERS[0]); // default: Anthropic
    match class {
        "haiku"  => entry.haiku,
        "opus"   => entry.opus,
        _        => entry.sonnet, // default class: sonnet
    }
}

/// Returns the human-readable provider name for display.
/// Falls back to the first provider's display_name if unrecognised.
pub fn provider_display_name(provider: &str) -> &'static str {
    PROVIDERS.iter()
        .find(|p| p.id == provider)
        .map(|p| p.display_name)
        .unwrap_or(PROVIDERS[0].display_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_slug_anthropic_all_classes() {
        assert_eq!(resolve_slug("anthropic", "haiku"),  "anthropic/claude-haiku-4-5-20251001");
        assert_eq!(resolve_slug("anthropic", "sonnet"), "anthropic/claude-sonnet-4-6");
        assert_eq!(resolve_slug("anthropic", "opus"),   "anthropic/claude-opus-4-7");
    }

    #[test]
    fn resolve_slug_z_ai() {
        assert_eq!(resolve_slug("z-ai", "haiku"),  "z-ai/glm-4.7-flash");
        assert_eq!(resolve_slug("z-ai", "sonnet"), "z-ai/glm-5-turbo");
        assert_eq!(resolve_slug("z-ai", "opus"),   "z-ai/glm-5.1");
    }

    #[test]
    fn resolve_slug_moonshot_haiku_and_sonnet_same() {
        assert_eq!(resolve_slug("moonshot", "haiku"),  "moonshotai/kimi-k2.5");
        assert_eq!(resolve_slug("moonshot", "sonnet"), "moonshotai/kimi-k2.5");
        assert_eq!(resolve_slug("moonshot", "opus"),   "moonshotai/kimi-k2.6");
    }

    #[test]
    fn resolve_slug_unknown_provider_falls_back_to_anthropic() {
        assert_eq!(resolve_slug("unknown", "sonnet"), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn resolve_slug_unknown_class_defaults_to_sonnet() {
        assert_eq!(resolve_slug("anthropic", "turbo"), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn provider_display_name_known() {
        assert_eq!(provider_display_name("anthropic"), "Anthropic");
        assert_eq!(provider_display_name("z-ai"),      "Z.ai");
        assert_eq!(provider_display_name("moonshot"),  "Moonshot");
        assert_eq!(provider_display_name("deepseek"),  "Deepseek");
        assert_eq!(provider_display_name("free"),      "Free");
    }

    #[test]
    fn provider_display_name_unknown_returns_id() {
        assert_eq!(resolve_slug("custom", "sonnet"), "anthropic/claude-sonnet-4-6");
    }
}
