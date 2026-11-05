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

/// The JWT signing secret from `RUSTYFIN_SECRET`. A random one is used when unset or blank.
pub fn jwt_secret(value: Option<String>) -> String {
    trimmed_or_warn(
        value,
        "RUSTYFIN_SECRET is set but empty, so a random secret is used and sign-ins end when the server restarts",
    )
    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // B8: API key

    fn admin() -> rustyfin_model::user::User {
        rustyfin_model::user::User {
            id: uuid::Uuid::new_v4(),
            name: "b8-forged".to_string(),
            password_hash: String::new(),
            is_administrator: true,
            created_at: Default::default(),
            last_login_date: None,
            configuration: serde_json::Value::Null,
        }
    }

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

    #[test]
    fn jwt_secret_is_trimmed() {
        assert_eq!(jwt_secret(Some("b8-secret".to_string())), "b8-secret");
        assert_eq!(jwt_secret(Some(" \tb8-secret \n".to_string())), "b8-secret");
    }

    #[test]
    fn unset_empty_or_blank_jwt_secret_is_random_per_start() {
        for value in [None, Some(String::new()), Some(" \t\n ".to_string())] {
            let secret = jwt_secret(value.clone());
            assert!(!secret.trim().is_empty(), "{value:?} gave a blank secret");
            assert_ne!(
                secret,
                jwt_secret(value.clone()),
                "{value:?} gave a fixed secret"
            );
        }
    }

    #[test]
    fn token_signed_with_an_empty_or_blank_secret_is_rejected() {
        for value in ["", "   "] {
            let forged = rustyfin_auth::create_token(&admin(), value).unwrap();
            let secret = jwt_secret(Some(value.to_string()));
            assert!(
                rustyfin_auth::validate_token(&forged, &secret).is_err(),
                "a token signed with {value:?} was accepted"
            );
        }
    }
}
