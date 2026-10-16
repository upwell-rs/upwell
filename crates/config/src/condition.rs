//! Raw config reads for component conditions.
//!
//! A condition may read a config path that no type binds through [`resolve`]. The
//! application resolves every declared raw path of the candidate config into a
//! [`ResolvedConfig`] before evaluation, so callbacks never touch resolvers or ambient
//! process state themselves.

use serde::de::DeserializeOwned;
use upwell_core::condition::scope::{self, ConditionInputRejection};

use crate::resolve::{ResolveCtx, render_str};
use crate::{ConfigStr, ConfigValue, ResolverChain, TemplateError, from_value};

/// A config subtree with every placeholder rendered, as supplied to condition callbacks.
///
/// Rendering keeps templated strings templated and turns literals and full placeholders into
/// literals, which both coerce from their rendered text, so deserializing a resolved subtree
/// coerces exactly like deserializing the source subtree. Values compare structurally,
/// which is how an unchanged raw input is recognised across reloads.
#[derive(Clone, PartialEq)]
pub struct ResolvedConfig(ConfigValue);

impl ResolvedConfig {
    /// Renders the subtree at `path` of `root`, or `None` when the path is absent.
    pub(crate) fn render(
        root: &ConfigValue,
        path: &str,
        resolvers: &ResolverChain,
    ) -> Result<Option<Self>, TemplateError> {
        let Some(subtree) = root.get_path(path) else {
            return Ok(None);
        };
        let mut ctx = ResolveCtx::new(root, resolvers);

        let rendered = render_value(subtree, &mut ctx)?;

        Ok(Some(Self(rendered)))
    }
}

impl std::fmt::Debug for ResolvedConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedConfig(..)")
    }
}

fn render_value(
    value: &ConfigValue,
    ctx: &mut ResolveCtx<'_, '_>,
) -> Result<ConfigValue, TemplateError> {
    let rendered = match value {
        ConfigValue::Str(text) => {
            ConfigValue::Str(ConfigStr::rendered(render_str(text, ctx)?, text.kind))
        }

        ConfigValue::Array(items) => ConfigValue::Array(
            items
                .iter()
                .map(|item| render_value(item, ctx))
                .collect::<Result<_, _>>()?,
        ),

        ConfigValue::Table(entries) => ConfigValue::Table(
            entries
                .iter()
                .map(|(key, item)| Ok((key.clone(), render_value(item, ctx)?)))
                .collect::<Result<_, TemplateError>>()?,
        ),

        ConfigValue::Null | ConfigValue::Bool(_) | ConfigValue::Int(_) | ConfigValue::Float(_) => {
            value.clone()
        }
    };

    Ok(rendered)
}

/// Reads the config value at `path` inside a component condition.
///
/// `path` must be a string literal so the condition declares it statically. A missing key
/// answers `None`. A present value that does not deserialize into `T` fails the condition
/// evaluation, as does reading a path the condition did not declare; both also answer
/// `None` so the expression can finish. The value deserializes with the ordinary config
/// deserializer, so it coerces exactly like a `#[config]` field.
///
/// The deserialization error itself is dropped: it can quote the config value, and
/// condition diagnostics are redacted to the path and target type.
///
/// # Panics
///
/// Panics when called outside a condition evaluation.
pub fn resolve<T: DeserializeOwned>(path: &'static str) -> Option<T> {
    let value = scope::raw_input(path)?;
    let parsed = value
        .downcast_ref::<ResolvedConfig>()
        .and_then(|resolved| from_value::<T>(&resolved.0, &ResolverChain::empty()).ok());

    if parsed.is_none() {
        scope::reject_input(ConditionInputRejection::Invalid {
            path,
            target: std::any::type_name::<T>(),
        });
    }

    parsed
}

#[cfg(test)]
mod tests;
