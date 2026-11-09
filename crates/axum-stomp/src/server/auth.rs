//! STOMP CONNECT authentication and the principal it establishes for message handlers.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use upwell_axum::{
    Component, DependencyDescriptor, DiError, FromContainer, Injectable, ScopeContainer, Seeds,
    WebsocketUpgradeMeta,
};

use super::StompHeaders;

/// The parsed authentication-relevant contents of a STOMP `CONNECT` frame, with the head of the
/// HTTP request that upgraded the connection.
///
/// The custom headers are preserved in wire order. `Debug` deliberately redacts the passcode and
/// lists only the names of the upgrade's headers.
#[derive(Clone)]
pub struct StompConnect {
    host: String,
    login: Option<String>,
    passcode: Option<String>,
    headers: StompHeaders,
    upgrade: Arc<WebsocketUpgradeMeta>,
}

impl StompConnect {
    pub(crate) fn new(
        host: String,
        login: Option<String>,
        passcode: Option<String>,
        headers: Vec<(String, String)>,
        upgrade: Arc<WebsocketUpgradeMeta>,
    ) -> Self {
        Self {
            host,
            login,
            passcode,
            headers: StompHeaders::new(headers),
            upgrade,
        }
    }

    /// The head of the HTTP request that upgraded the connection: its URI, headers and
    /// cookies, for authentication schemes that read them, such as a session cookie.
    pub fn upgrade(&self) -> &WebsocketUpgradeMeta {
        &self.upgrade
    }

    /// The virtual host requested by the client.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The standard STOMP `login` credential, if supplied.
    pub fn login(&self) -> Option<&str> {
        self.login.as_deref()
    }

    /// The standard STOMP `passcode` credential, if supplied.
    pub fn passcode(&self) -> Option<&str> {
        self.passcode.as_deref()
    }

    /// Custom CONNECT headers, for token or application-specific authentication schemes.
    pub fn headers(&self) -> &StompHeaders {
        &self.headers
    }
}

impl fmt::Debug for StompConnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name).collect();
        let upgrade_header_names: Vec<&str> = self
            .upgrade
            .headers
            .keys()
            .map(|name| name.as_str())
            .collect();

        f.debug_struct("StompConnect")
            .field("host", &self.host)
            .field("login", &self.login)
            .field("passcode", &self.passcode.as_ref().map(|_| "[REDACTED]"))
            .field("header_names", &header_names)
            .field("upgrade_uri", &self.upgrade.uri)
            .field("upgrade_header_names", &upgrade_header_names)
            .finish()
    }
}

/// The authenticated identity of a STOMP connection, a seed of its connection scope, so every
/// connection- and message-scoped component and handler can inject it.
///
/// An endpoint without an authenticator receives [`anonymous`](Self::anonymous) principals. An
/// authenticator returns [`authenticated`](Self::authenticated) with an application-defined
/// subject and may attach string attributes for authorization decisions in message handlers.
#[derive(Clone, Debug, Default)]
pub struct StompPrincipal {
    subject: Option<Arc<str>>,
    attributes: Arc<HashMap<String, String>>,
}

impl StompPrincipal {
    /// An unauthenticated principal used when an endpoint has no authenticator.
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// An authenticated principal identified by `subject`.
    pub fn authenticated(subject: impl Into<Arc<str>>) -> Self {
        Self {
            subject: Some(subject.into()),
            attributes: Arc::default(),
        }
    }

    /// Adds or replaces one authorization attribute.
    pub fn with_attribute(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.attributes).insert(name.into(), value.into());

        self
    }

    /// Whether an authenticator established this identity.
    pub fn is_authenticated(&self) -> bool {
        self.subject.is_some()
    }

    /// The authenticated subject, or `None` for an anonymous connection.
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    /// One application-defined authorization attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    /// Every application-defined authorization attribute.
    pub fn attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attributes
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

impl Injectable for StompPrincipal {
    type Target = StompPrincipal;
    type Stored = Self;

    fn into_stored(self) -> Self {
        self
    }

    fn from_stored(stored: &Self) -> Self {
        stored.clone()
    }
}

/// What a successful [`StompAuthenticator`] produces: the connection's [`StompPrincipal`] and any
/// application seeds, such as an app-defined identity, for the connection scope.
///
/// The connection scope opens only after authentication, with the principal and these seeds,
/// so no connection-scoped component observes the connection before it is accepted. Each seed
/// must fill a factory-less slot of the connection scope, or the connection is closed.
#[derive(Debug)]
pub struct StompAuthenticated {
    principal: StompPrincipal,
    seeds: Seeds,
}

impl StompAuthenticated {
    /// An accepted connection identified by `principal`, with no other seed.
    pub fn new(principal: StompPrincipal) -> Self {
        Self {
            principal,
            seeds: Seeds::new(),
        }
    }

    /// Adds `value` as the connection scope's seed of `T`.
    #[must_use]
    pub fn seed<T: std::any::Any + Send + Sync>(mut self, value: T) -> Self {
        self.seeds.insert(value);

        self
    }

    /// The accepted connection's principal.
    pub fn principal(&self) -> &StompPrincipal {
        &self.principal
    }

    pub(crate) fn into_parts(self) -> (StompPrincipal, Seeds) {
        (self.principal, self.seeds)
    }
}

impl From<StompPrincipal> for StompAuthenticated {
    fn from(principal: StompPrincipal) -> Self {
        Self::new(principal)
    }
}

/// A rejected STOMP connection.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StompAuthenticationError {
    /// The authenticator rejected the credentials; the message is sent to the client.
    #[error("{0}")]
    Rejected(String),

    /// An argument of the authenticator could not be resolved from the application root. An
    /// authenticator runs before the connection scope opens, so it can inject singletons and
    /// configuration only. The client sees only that the authenticator is misconfigured.
    #[error("authenticator misconfigured")]
    Unresolved {
        /// The type of the argument that could not be resolved.
        dependency: &'static str,
        /// Why it could not be resolved.
        #[source]
        source: DiError,
    },
}

impl StompAuthenticationError {
    /// Builds a rejection with a client-visible error message.
    pub fn new(message: impl Into<String>) -> Self {
        Self::Rejected(message.into())
    }
}

/// The boxed authentication future returned by [`StompAuthenticator`].
pub type StompAuthFuture = Pin<
    Box<dyn Future<Output = Result<StompAuthenticated, StompAuthenticationError>> + Send + 'static>,
>;

/// Authenticates one parsed STOMP `CONNECT` frame before the broker sends `CONNECTED`.
///
/// It runs before the connection scope opens, so the scope can be seeded with what it
/// produces. The constant input is the [`StompConnect`] frame the authenticator must inspect,
/// with the upgrade's head; the application root's [`ScopeContainer`] is handed in too, so an
/// implementation can resolve singletons and configuration from DI. Connection-scoped
/// components do not exist yet: [`Stomp`](crate::Stomp) rejects an authenticator whose declared
/// [`dependencies`](Self::dependencies) only a scope provides when the endpoint is built, and
/// any other unresolvable argument rejects the connection with
/// [`StompAuthenticationError::Unresolved`]. Where an implementation gets its dependencies is its
/// own choice:
///
/// - **From parameters** — an async closure/fn whose arguments after `StompConnect` are injected
///   from the root (anything a constructor parameter can be — `Arc<T>`, `Dep<T>`, a by-value
///   injectable, `Cfg<T>`, `Option`/`Vec`/`HashMap` of providers). It returns a
///   [`StompPrincipal`] or a [`StompAuthenticated`] carrying seeds. Async closures and functions
///   implement this trait directly, so most applications need no named type:
///
///   ```ignore
///   StompConfig::default().with_authenticator(
///       |connect: StompConnect, users: Arc<UserStore>| async move {
///           let user = users.validate(connect.login(), connect.passcode()).await?;
///           Ok(StompAuthenticated::new(StompPrincipal::authenticated(&user.id)).seed(Identity::from(user)))
///       },
///   )
///   ```
///
/// - **From `self`** — a component that hand-implements this trait and holds its own injected
///   fields. Install a built instance directly, or resolve one from the root at CONNECT time
///   with the [`Injected`] adapter: `with_authenticator(Injected::<TokenAuth>::new())`.
///
/// Dependencies resolved from parameters or by [`Injected`] are resolved at CONNECT time and are
/// **not** covered by `di-check`; a missing provider rejects the connection with a logged error —
/// the same runtime-resolution semantics as a handler's `Inject<T>`.
pub trait StompAuthenticator: Send + Sync + 'static {
    /// Resolves to the accepted connection's principal and seeds, or rejects the connection
    /// before `CONNECTED`. `root` is the application root an implementation may resolve
    /// singletons and configuration from.
    fn authenticate(
        self: Arc<Self>,
        connect: StompConnect,
        root: Arc<ScopeContainer>,
    ) -> StompAuthFuture;

    /// The dependency edges the authenticator resolves from the root, checked when the endpoint
    /// is built. None by default.
    fn dependencies(&self) -> Vec<DependencyDescriptor> {
        Vec::new()
    }
}

/// Maps a DI resolution failure during authentication into a connection rejection. Logged because a
/// missing provider is a wiring bug the client cannot act on.
fn authenticator_dependency_error(
    dependency: &'static str,
    source: DiError,
) -> StompAuthenticationError {
    tracing::error!(
        target: "upwell::axum",
        dependency,
        error = %source,
        "STOMP authenticator dependency could not be resolved from the application root",
    );

    StompAuthenticationError::Unresolved { dependency, source }
}

/// Converts a value accepted by [`with_authenticator`](super::StompConfig::with_authenticator) into
/// a shared [`StompAuthenticator`]. `Marker` disambiguates the two blanket forms — a value that is
/// already a `StompAuthenticator` (a component or the [`Injected`] adapter) versus an async function
/// whose arguments after [`StompConnect`] are DI-injected — so both install through the one builder
/// method without the blanket impls overlapping.
pub trait IntoAuthenticator<Marker> {
    /// Erases the value into a shared authenticator.
    fn into_authenticator(self) -> Arc<dyn StompAuthenticator>;
}

/// The [`IntoAuthenticator`] marker for a value that already implements [`StompAuthenticator`].
pub struct Direct;

impl<A> IntoAuthenticator<Direct> for A
where
    A: StompAuthenticator,
{
    fn into_authenticator(self) -> Arc<dyn StompAuthenticator> {
        Arc::new(self)
    }
}

/// Adapts an authenticator function into a [`StompAuthenticator`], resolving the function's injected
/// arguments from the application root on each CONNECT. `Args` records the argument tuple so the
/// parameter type variables stay constrained (mirroring the DI crate's `Factory<Args>`).
pub struct FnAuthenticator<F, Args> {
    function: F,
    _args: PhantomData<fn() -> Args>,
}

/// Implements the function-authenticator forms for one argument arity: the [`IntoAuthenticator`]
/// blanket that boxes the function into a [`FnAuthenticator`], and the [`StompAuthenticator`] impl
/// that resolves each injected argument and calls it. Arity zero is the plain `Fn(StompConnect)`.
macro_rules! impl_authenticator_fn {
    ( $($ty:ident),* ) => {
        impl<F, Fut, R, $($ty,)*> IntoAuthenticator<fn($($ty,)*) -> R> for F
        where
            F: Fn(StompConnect, $($ty,)*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, StompAuthenticationError>> + Send + 'static,
            R: Into<StompAuthenticated> + 'static,
            $( $ty: FromContainer + Send + 'static, )*
        {
            fn into_authenticator(self) -> Arc<dyn StompAuthenticator> {
                Arc::new(FnAuthenticator::<F, ($($ty,)*)> {
                    function: self,
                    _args: PhantomData,
                })
            }
        }

        impl<F, Fut, R, $($ty,)*> StompAuthenticator for FnAuthenticator<F, ($($ty,)*)>
        where
            F: Fn(StompConnect, $($ty,)*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, StompAuthenticationError>> + Send + 'static,
            R: Into<StompAuthenticated> + 'static,
            $( $ty: FromContainer + Send + 'static, )*
        {
            #[allow(non_snake_case, unused_variables)]
            fn authenticate(
                self: Arc<Self>,
                connect: StompConnect,
                root: Arc<ScopeContainer>,
            ) -> StompAuthFuture {
                Box::pin(async move {
                    $(
                        let $ty = root
                            .extract::<$ty>()
                            .await
                            .map_err(|error| {
                                authenticator_dependency_error(::std::any::type_name::<$ty>(), error)
                            })?;
                    )*

                    (self.function)(connect, $($ty,)*).await.map(Into::into)
                })
            }

            fn dependencies(&self) -> Vec<DependencyDescriptor> {
                vec![$(<$ty as FromContainer>::dependency(),)*]
            }
        }
    };
}

impl_authenticator_fn!();
impl_authenticator_fn!(P1);
impl_authenticator_fn!(P1, P2);
impl_authenticator_fn!(P1, P2, P3);
impl_authenticator_fn!(P1, P2, P3, P4);
impl_authenticator_fn!(P1, P2, P3, P4, P5);

/// A resolved component handle that can serve as a [`StompAuthenticator`]: an `Arc<T>` handle for a
/// reference-counted component, or a by-value component that is itself the authenticator. The
/// `Target = Self` bound on the by-value impl excludes `Arc<T>` (`Target = T`), so the two do not
/// overlap — the same disjointness the DI crate's `FromContainer` relies on.
pub trait ResolvedAuthenticator {
    /// Erases the concrete handle into a shared authenticator, wrapping a by-value component in an
    /// `Arc` so it can be called through the `self: Arc<Self>` receiver.
    fn into_authenticator(self) -> Arc<dyn StompAuthenticator>;
}

impl<T> ResolvedAuthenticator for Arc<T>
where
    T: StompAuthenticator,
{
    fn into_authenticator(self) -> Arc<dyn StompAuthenticator> {
        self
    }
}

impl<T> ResolvedAuthenticator for T
where
    T: StompAuthenticator + Injectable<Target = T>,
{
    fn into_authenticator(self) -> Arc<dyn StompAuthenticator> {
        Arc::new(self)
    }
}

/// Installs a DI-resolved component as the [`StompAuthenticator`].
///
/// The component `T` is resolved from the application root at CONNECT time (so its own injected
/// fields are already wired) and its [`authenticate`](StompAuthenticator::authenticate) runs. Works
/// for both a reference-counted `#[component]` (resolved as `Arc<T>`) and a by-value component
/// (resolved by value). Use it through the single builder:
/// `with_authenticator(Injected::<TokenAuth>::new())`.
pub struct Injected<T>(PhantomData<fn() -> T>);

impl<T> Injected<T> {
    /// Builds the adapter that resolves component `T` from the application root.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for Injected<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> StompAuthenticator for Injected<T>
where
    T: Component,
    T::Handle: ResolvedAuthenticator + FromContainer + Send + 'static,
{
    fn authenticate(
        self: Arc<Self>,
        connect: StompConnect,
        root: Arc<ScopeContainer>,
    ) -> StompAuthFuture {
        Box::pin(async move {
            let handle = root.extract::<T::Handle>().await.map_err(|error| {
                authenticator_dependency_error(::std::any::type_name::<T>(), error)
            })?;

            handle
                .into_authenticator()
                .authenticate(connect, root)
                .await
        })
    }

    fn dependencies(&self) -> Vec<DependencyDescriptor> {
        vec![<T::Handle as FromContainer>::dependency()]
    }
}

/// Rejects an authenticator argument whose type every registered component provides only in a
/// scope, which a root resolution can never reach. Trait and configuration arguments are left to
/// the CONNECT-time resolution.
pub(super) fn check_root_dependencies(
    authenticator: &dyn StompAuthenticator,
    runtime: &upwell_axum::AppRuntime,
) -> Result<(), super::StompBuildError> {
    let components = runtime.resolved_components();
    let topology = runtime.scope_topology();

    for dependency in authenticator.dependencies() {
        let placements: Vec<_> = components
            .iter()
            .filter(|component| component.ty.type_id == dependency.ty.type_id)
            .collect();
        let scoped_only = placements
            .iter()
            .all(|component| topology.boundary(&component.scope.id()).is_some());

        if let Some(first) = placements.first()
            && scoped_only
            && !dependency.config
        {
            return Err(super::StompBuildError::ScopedAuthenticatorDependency {
                dependency: (dependency.ty.type_name)(),
                scope: first.scope.name(),
            });
        }
    }

    Ok(())
}

#[cfg(feature = "di-check")]
impl upwell_axum::Provided<StompPrincipal> for upwell_axum::Wiring {}

#[cfg(test)]
#[path = "auth/tests.rs"]
mod tests;
