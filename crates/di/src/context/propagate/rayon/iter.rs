//! [`ParallelIteratorExt::current`]: a parallel iterator whose closures run inside a captured
//! invocation context.
//!
//! rayon hands a nested parallel iterator, such as the one `flat_map` or `flatten` starts per
//! item, a consumer of its own, so no wrapper around a consumer can follow every job such an
//! adapter spawns. [`WithContext`] therefore binds the closures themselves: it is not a
//! [`ParallelIterator`], and it offers only the adapters and consumers whose every closure call
//! it can run inside the context, wherever rayon schedules it. Nested parallel work started
//! inside one of those closures carries the context again by calling
//! [`current`](ParallelIteratorExt::current) or a `context::rayon` function there, since the
//! closure itself runs inside the context.

use ::rayon::iter::{FromParallelIterator, ParallelIterator};

use super::CapturedContext;

/// Carries the invocation context into the closures of a parallel iterator.
pub trait ParallelIteratorExt: ParallelIterator {
    /// This iterator, with every closure passed to the returned [`WithContext`]'s methods run
    /// inside the invocation context and span active here, whichever worker runs it.
    ///
    /// Closures of the adapters chained before it run outside the context.
    fn current(self) -> WithContext<Self> {
        WithContext {
            base: self,
            context: CapturedContext::active(),
        }
    }
}

impl<I: ParallelIterator> ParallelIteratorExt for I {}

/// A parallel iterator whose closures run inside a captured invocation context. See
/// [`ParallelIteratorExt::current`] and the [module documentation](self).
#[must_use = "a parallel iterator does nothing until it is consumed"]
#[derive(Clone, Debug)]
pub struct WithContext<I> {
    base: I,
    context: CapturedContext,
}

impl<I: ParallelIterator> WithContext<I> {
    /// [`ParallelIterator::map`], calling `map` inside the context.
    pub fn map<F, R>(self, map: F) -> WithContext<impl ParallelIterator<Item = R>>
    where
        F: Fn(I::Item) -> R + Sync + Send,
        R: Send,
    {
        let context = self.context.clone();

        self.adapt(|base| base.map(move |item| context.run_in(|| map(item))))
    }

    /// [`ParallelIterator::filter`], calling `predicate` inside the context.
    pub fn filter<P>(self, predicate: P) -> WithContext<impl ParallelIterator<Item = I::Item>>
    where
        P: Fn(&I::Item) -> bool + Sync + Send,
    {
        let context = self.context.clone();

        self.adapt(|base| base.filter(move |item| context.run_in(|| predicate(item))))
    }

    /// [`ParallelIterator::filter_map`], calling `filter_map` inside the context.
    pub fn filter_map<F, R>(self, filter_map: F) -> WithContext<impl ParallelIterator<Item = R>>
    where
        F: Fn(I::Item) -> Option<R> + Sync + Send,
        R: Send,
    {
        let context = self.context.clone();

        self.adapt(|base| base.filter_map(move |item| context.run_in(|| filter_map(item))))
    }

    /// [`ParallelIterator::inspect`], calling `inspect` inside the context.
    pub fn inspect<F>(self, inspect: F) -> WithContext<impl ParallelIterator<Item = I::Item>>
    where
        F: Fn(&I::Item) + Sync + Send,
    {
        let context = self.context.clone();

        self.adapt(|base| base.inspect(move |item| context.run_in(|| inspect(item))))
    }

    /// [`ParallelIterator::fold`], calling `identity` and `fold` inside the context.
    pub fn fold<T, ID, F>(
        self,
        identity: ID,
        fold: F,
    ) -> WithContext<impl ParallelIterator<Item = T>>
    where
        ID: Fn() -> T + Sync + Send,
        F: Fn(T, I::Item) -> T + Sync + Send,
        T: Send,
    {
        let identity_context = self.context.clone();
        let fold_context = self.context.clone();

        self.adapt(|base| {
            base.fold(
                move || identity_context.run_in(&identity),
                move |accumulator, item| fold_context.run_in(|| fold(accumulator, item)),
            )
        })
    }

    /// [`ParallelIterator::for_each`], calling `body` inside the context.
    pub fn for_each<F>(self, body: F)
    where
        F: Fn(I::Item) + Sync + Send,
    {
        let Self { base, context } = self;

        base.for_each(move |item| context.run_in(|| body(item)));
    }

    /// [`ParallelIterator::reduce`], calling `identity` and `reduce` inside the context.
    pub fn reduce<ID, F>(self, identity: ID, reduce: F) -> I::Item
    where
        ID: Fn() -> I::Item + Sync + Send,
        F: Fn(I::Item, I::Item) -> I::Item + Sync + Send,
    {
        let Self { base, context } = self;
        let identity_context = context.clone();

        base.reduce(
            move || identity_context.run_in(&identity),
            move |left, right| context.run_in(|| reduce(left, right)),
        )
    }

    /// [`ParallelIterator::all`], calling `predicate` inside the context.
    pub fn all<P>(self, predicate: P) -> bool
    where
        P: Fn(I::Item) -> bool + Sync + Send,
    {
        let Self { base, context } = self;

        base.all(move |item| context.run_in(|| predicate(item)))
    }

    /// [`ParallelIterator::any`], calling `predicate` inside the context.
    pub fn any<P>(self, predicate: P) -> bool
    where
        P: Fn(I::Item) -> bool + Sync + Send,
    {
        let Self { base, context } = self;

        base.any(move |item| context.run_in(|| predicate(item)))
    }

    /// [`ParallelIterator::count`]. It runs no closure.
    pub fn count(self) -> usize {
        self.base.count()
    }

    /// [`ParallelIterator::collect`]. It runs no closure of its own.
    pub fn collect<C>(self) -> C
    where
        C: FromParallelIterator<I::Item>,
    {
        self.base.collect()
    }

    /// `adapt(base)`, keeping this iterator's context.
    fn adapt<J>(self, adapt: impl FnOnce(I) -> J) -> WithContext<J> {
        let Self { base, context } = self;

        WithContext {
            base: adapt(base),
            context,
        }
    }
}
