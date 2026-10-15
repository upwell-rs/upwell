/// Implements [`Component`](upwell_di::Component) for a test type stored as `Arc<Self>`,
/// with stable ID `$id` and the type's own name, plus a
/// [`ProviderOf`](upwell_di::ProviderOf) upcast for each trait object after `provides`.
macro_rules! test_component {
    ($ty:ident, $id:literal $(, provides $($provided:ty),+)?) => {
        impl ::upwell_di::Component for $ty {
            type Handle = ::std::sync::Arc<Self>;

            const ID: &'static str = $id;
            const NAME: &'static str = stringify!($ty);

            fn into_handle(self) -> Self::Handle {
                ::std::sync::Arc::new(self)
            }
        }

        $($(
            impl ::upwell_di::ProviderOf<$provided> for $ty {
                fn upcast(this: ::std::sync::Arc<Self>) -> ::std::sync::Arc<$provided> {
                    this
                }
            }
        )+)?
    };
}

pub(crate) use test_component;

/// Declares the one never-driven recipe `$id` declaring `$dependencies`, as the
/// `fn() -> &'static [..]` a descriptor stores.
macro_rules! unbuilt_factories {
    ($id:literal, $dependencies:expr) => {
        || {
            const {
                &[::upwell_di::ComponentFactoryDescriptor {
                    id: $id,
                    construct: $crate::test_support::unbuilt,
                    dependencies: $dependencies,
                    default: false,
                }]
            }
        }
    };
}

pub(crate) use unbuilt_factories;

/// The factory callback of a recipe that planning tests register but never drive.
pub(crate) fn unbuilt(
    _: &mut upwell_di::ComponentConstructionContext,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = upwell_di::Result<upwell_di::BoxedComponent>> + Send + '_>,
> {
    panic!("a planning test constructed a component")
}
