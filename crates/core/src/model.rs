//! Locked-model policy (DESIGN.md §5).
//!
//! - Never auto-pick the newest id from a live list.
//! - A saved/locked model is kept as-is.
//! - If unset, the default is `x-ai/grok-4.7`.
//! - One-time migration: exactly the old stock default `openai/gpt-4o-mini`
//!   migrates to the new default. A model the user picked themselves is never
//!   touched, including a deliberate re-pick of the old default.

pub const DEFAULT_MODEL: &str = "x-ai/grok-4.7";
pub const LEGACY_DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

#[derive(Debug, Clone, PartialEq)]
pub struct ModelResolution {
    /// The model to use.
    pub model: String,
    /// The value the one-time-migration flag should have after this call.
    pub migrated: bool,
    /// True when this call performed the legacy migration.
    pub migrated_now: bool,
}

/// Resolve the effective model from storage state.
pub fn resolve_model(saved: Option<&str>, migrated: bool) -> ModelResolution {
    let saved_clean = saved.map(str::trim).filter(|s| !s.is_empty());
    match saved_clean {
        None => ModelResolution {
            model: DEFAULT_MODEL.to_string(),
            migrated,
            migrated_now: false,
        },
        Some(m) if m == LEGACY_DEFAULT_MODEL && !migrated => ModelResolution {
            model: DEFAULT_MODEL.to_string(),
            migrated: true,
            migrated_now: true,
        },
        Some(m) => ModelResolution {
            model: m.to_string(),
            migrated,
            migrated_now: false,
        },
    }
}

/// Validate free-text model input from the picker: non-empty after trimming,
/// reasonable length. The locked value is whatever the user saved here.
pub fn normalize_model_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 200 {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_profile_gets_grok_default() {
        let r = resolve_model(None, false);
        assert_eq!(r.model, "x-ai/grok-4.7");
        assert!(!r.migrated_now);
    }

    #[test]
    fn legacy_stock_default_migrates_once() {
        let r = resolve_model(Some("openai/gpt-4o-mini"), false);
        assert_eq!(r.model, "x-ai/grok-4.7");
        assert!(r.migrated_now);
        assert!(r.migrated);

        // Second load: flag set, no re-migration.
        let r2 = resolve_model(Some("openai/gpt-4o-mini"), true);
        assert_eq!(r2.model, "openai/gpt-4o-mini");
        assert!(!r2.migrated_now);
    }

    #[test]
    fn saved_user_choice_is_kept_verbatim() {
        for saved in ["anthropic/claude-sonnet-4.5", "x-ai/grok-4.7", "meta-llama/llama-4-maverick"] {
            let r = resolve_model(Some(saved), false);
            assert_eq!(r.model, saved);
            assert!(!r.migrated_now);
            // A live model list can never clobber it: resolve only reads the
            // saved value, and the picker writes only on explicit user save.
        }
    }

    #[test]
    fn whitespace_and_empty_treated_as_unset() {
        assert_eq!(resolve_model(Some("   "), false).model, "x-ai/grok-4.7");
        assert_eq!(resolve_model(Some(""), false).model, "x-ai/grok-4.7");
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert_eq!(normalize_model_input("  x-ai/grok-4.7 "), Some("x-ai/grok-4.7".into()));
        assert_eq!(normalize_model_input("   "), None);
        assert_eq!(normalize_model_input(&"x".repeat(201)), None);
    }
}
