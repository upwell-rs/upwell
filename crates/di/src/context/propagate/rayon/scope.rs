//! [`scope`]: `rayon::scope` whose spawned jobs carry the invocation context.

use super::CapturedContext;

/// [`rayon::scope`](::rayon::scope), running `body` and every job its [`Scope`] spawns inside
/// the invocation context and span active where it is called.
///
/// # Panics
///
/// When `body` or a spawned job panics, after every job ended, as `rayon::scope`; and when
/// rayon's global pool cannot start.
pub fn scope<'scope, F, R>(body: F) -> R
where
    F: for<'inner> FnOnce(&Scope<'inner, 'scope>) -> R + Send,
    R: Send,
{
    let context = CapturedContext::active();

    ::rayon::scope(move |inner| {
        let scope = Scope { inner, context };

        scope.context.run_in(|| body(&scope))
    })
}

/// A [`rayon::Scope`](::rayon::Scope) whose jobs carry a captured invocation context.
#[derive(Debug)]
pub struct Scope<'inner, 'scope> {
    inner: &'inner ::rayon::Scope<'scope>,
    context: CapturedContext,
}

impl<'scope> Scope<'_, 'scope> {
    /// [`rayon::Scope::spawn`](::rayon::Scope::spawn), running `body` inside the captured
    /// context and span. The job receives this scope's counterpart, so the jobs it spawns
    /// carry the context too.
    ///
    /// # Panics
    ///
    /// When `body` panics, the enclosing [`scope`] panics after every job ended.
    pub fn spawn<F>(&self, body: F)
    where
        F: for<'inner> FnOnce(&Scope<'inner, 'scope>) + Send + 'scope,
    {
        let context = self.context.clone();

        self.inner.spawn(move |inner| {
            let scope = Scope { inner, context };

            scope.context.run_in(|| body(&scope));
        });
    }
}
