use syn::parse_quote;

use super::*;

macro_rules! assert_purity {
    ($pure:expr => $($expr:expr),+ $(,)?) => {
        $(
            assert_eq!(
                is_pure(&parse_quote!($expr)),
                $pure,
                "purity of `{}`",
                stringify!($expr),
            );
        )+
    };
}

#[test]
fn closed_subset_expressions_are_pure() {
    assert_purity!(true =>
        AuthConfig.enabled,
        AuthConfig.mode == AuthMode::Custom,
        AuthConfig.mode != crate::auth::AuthMode::Custom,
        AuthConfig.limits.max == -1,
        AuthConfig.name == "custom" && !Flags.beta,
        (Flags.beta || Flags.alpha) && Audit.eligible(),
        resolve("feature.mode") == Some("on"),
        resolve("feature.mode") != None,
        AuthConfig.mode == MODE,
        true,
    );
}

#[test]
fn calls_macros_and_other_operators_are_trusted() {
    assert_purity!(false =>
        AuthConfig.is_custom(),
        AuthConfig.mode.is_custom(),
        helper(AuthConfig.mode),
        matches!(AuthConfig.mode, AuthMode::Custom),
        AuthConfig.mode == AuthMode::parse("custom"),
        AuthConfig.limit > 3,
        AuthConfig.name == "custom".to_string(),
        Audit.eligible_now(),
        Audit.eligible::<()>(),
        Some(AuthConfig.mode).is_some(),
        resolve(PATH) == Some("on"),
        upwell::condition::resolve("feature.mode") == Some("on"),
        evil::resolve("feature.mode") == Some("on"),
        resolve("feature.mode") == mine::Some("on"),
        AuthConfig.mode == { AuthMode::Custom },
        flag,
    );
}
