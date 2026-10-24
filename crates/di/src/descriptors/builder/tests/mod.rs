//! Shared `Greeter` provider fixtures for the descriptor builder tests.

use std::sync::Arc;

use upwell_core::{ConditionDescriptor, ConfigFactId};

use super::*;

mod catalog;
mod descriptors;

trait Greeter: Send + Sync {
    fn greet(&self) -> &'static str;
}

/// Declares a unit [`Greeter`] component with a fixed stable ID and greeting.
macro_rules! greeter {
    ($ty:ident, $id:literal, $greeting:literal) => {
        struct $ty;

        impl Greeter for $ty {
            fn greet(&self) -> &'static str {
                $greeting
            }
        }

        impl Component for $ty {
            type Handle = Arc<Self>;

            const ID: &'static str = $id;
            const NAME: &'static str = stringify!($ty);

            fn into_handle(self) -> Self::Handle {
                Arc::new(self)
            }
        }

        impl ProviderOf<dyn Greeter> for $ty {
            fn upcast(this: Arc<Self>) -> Arc<dyn Greeter> {
                this
            }
        }
    };
}

greeter!(English, "english", "hello");
greeter!(Swedish, "swedish", "hej");

async fn english() -> English {
    English
}

async fn swedish() -> Swedish {
    Swedish
}

async fn english_from(_swedish: Arc<Swedish>) -> Result<English, &'static str> {
    Ok(English)
}

const ENABLED: ConfigFactId = ConfigFactId::new("GreeterConfig", "greeter", "enabled");

static ENABLED_CONDITION: ConditionDescriptor =
    ConditionDescriptor::config_bool("greeter-enabled", ENABLED);

static ENGLISH: ComponentDescriptor = ComponentDescriptor::of::<English>()
    .with_factories(factories![english])
    .with_condition(&ENABLED_CONDITION);

static ENGLISH_RECIPES: ComponentDescriptor = ComponentDescriptor::of::<English>()
    .with_factories(factories![default "plain" => english, "from-swedish" => english_from]);
