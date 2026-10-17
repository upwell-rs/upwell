/// Implements [`Component`](crate::Component) for a test type stored as `Arc<Self>`, with
/// stable ID `$id` and the type's own name, plus a [`ProviderOf`](crate::ProviderOf)
/// upcast for each trait object after `provides`.
macro_rules! test_component {
    ($ty:ident, $id:literal $(, provides $($provided:ty),+)?) => {
        impl $crate::Component for $ty {
            type Handle = ::std::sync::Arc<Self>;

            const ID: &'static str = $id;
            const NAME: &'static str = stringify!($ty);

            fn into_handle(self) -> Self::Handle {
                ::std::sync::Arc::new(self)
            }
        }

        $($(
            impl $crate::ProviderOf<$provided> for $ty {
                fn upcast(this: ::std::sync::Arc<Self>) -> ::std::sync::Arc<$provided> {
                    this
                }
            }
        )+)?
    };
}

pub(crate) use test_component;
