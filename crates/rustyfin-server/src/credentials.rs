//! The credentials the server reads from the environment at startup.

/// The trimmed `value`, or `None` when unset or blank. `warning` is logged when it is blank.
fn trimmed_or_warn(value: Option<String>, warning: &'static str) -> Option<String> {
    let trimmed = value?.trim().to_string();
    if trimmed.is_empty() {
        tracing::warn!("{warning}");
        return None;
    }
    Some(trimmed)
}

/// The API key from `RUSTYFIN_API_KEY`. `None` turns API key access off.
pub fn api_key(value: Option<String>) -> Option<String> {
    trimmed_or_warn(
        value,
        "RUSTYFIN_API_KEY is set but empty, so API key access is off",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // B8: API key

    #[test]
    fn api_key_is_trimmed() {
        assert_eq!(
            api_key(Some("b8-key".to_string())).as_deref(),
            Some("b8-key")
        );
        assert_eq!(
            api_key(Some(" \tb8-key \n".to_string())).as_deref(),
            Some("b8-key")
        );
    }

    #[test]
    fn unset_empty_or_blank_api_key_is_none() {
        assert_eq!(api_key(None), None);
        assert_eq!(api_key(Some(String::new())), None);
        assert_eq!(api_key(Some(" \t\n ".to_string())), None);
    }
}
