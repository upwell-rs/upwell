//! [`ParallelIteratorExt::current`]: a parallel iterator whose folds run inside a captured
//! invocation context.

use ::rayon::iter::ParallelIterator;
use ::rayon::iter::plumbing::{Consumer, Folder, Reducer, UnindexedConsumer};

use super::CapturedContext;

/// Carries the invocation context into the closures of a parallel iterator.
pub trait ParallelIteratorExt: ParallelIterator {
    /// This iterator, with every closure of the adapters and the consumer chained after it run
    /// inside the invocation context and span active here, whichever worker runs them.
    ///
    /// The result is an unindexed [`ParallelIterator`], so apply indexed adapters such as
    /// `zip` or `enumerate` before it. Closures of the adapters chained before it run outside
    /// the context.
    fn current(self) -> WithContext<Self> {
        WithContext {
            base: self,
            context: CapturedContext::active(),
        }
    }
}

impl<I: ParallelIterator> ParallelIteratorExt for I {}

/// A parallel iterator whose downstream work runs inside a captured invocation context. See
/// [`ParallelIteratorExt::current`].
#[must_use = "a parallel iterator does nothing until it is consumed"]
#[derive(Clone, Debug)]
pub struct WithContext<I> {
    base: I,
    context: CapturedContext,
}

impl<I: ParallelIterator> ParallelIterator for WithContext<I> {
    type Item = I::Item;

    fn drive_unindexed<C>(self, consumer: C) -> C::Result
    where
        C: UnindexedConsumer<Self::Item>,
    {
        self.base.drive_unindexed(ContextConsumer {
            base: consumer,
            context: self.context,
        })
    }

    fn opt_len(&self) -> Option<usize> {
        self.base.opt_len()
    }
}

/// A consumer whose folders and reducers run inside a captured context.
struct ContextConsumer<C> {
    base: C,
    context: CapturedContext,
}

impl<T, C: Consumer<T>> Consumer<T> for ContextConsumer<C> {
    type Folder = ContextFolder<C::Folder>;
    type Reducer = ContextReducer<C::Reducer>;
    type Result = C::Result;

    fn split_at(self, index: usize) -> (Self, Self, Self::Reducer) {
        let (left, right, reducer) = self.base.split_at(index);
        let reducer = ContextReducer {
            base: reducer,
            context: self.context.clone(),
        };
        let left = ContextConsumer {
            base: left,
            context: self.context.clone(),
        };
        let right = ContextConsumer {
            base: right,
            context: self.context,
        };

        (left, right, reducer)
    }

    fn into_folder(self) -> Self::Folder {
        ContextFolder {
            base: self.base.into_folder(),
            context: self.context,
        }
    }

    fn full(&self) -> bool {
        self.base.full()
    }
}

impl<T, C: UnindexedConsumer<T>> UnindexedConsumer<T> for ContextConsumer<C> {
    fn split_off_left(&self) -> Self {
        ContextConsumer {
            base: self.base.split_off_left(),
            context: self.context.clone(),
        }
    }

    fn to_reducer(&self) -> Self::Reducer {
        ContextReducer {
            base: self.base.to_reducer(),
            context: self.context.clone(),
        }
    }
}

/// A folder that consumes inside a captured context.
struct ContextFolder<F> {
    base: F,
    context: CapturedContext,
}

impl<T, F: Folder<T>> Folder<T> for ContextFolder<F> {
    type Result = F::Result;

    fn consume(self, item: T) -> Self {
        let Self { base, context } = self;
        let base = context.run_in(|| base.consume(item));

        Self { base, context }
    }

    fn consume_iter<I>(self, iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let Self { base, context } = self;
        let base = context.run_in(|| base.consume_iter(iter));

        Self { base, context }
    }

    fn complete(self) -> Self::Result {
        let Self { base, context } = self;

        context.run(|| base.complete())
    }

    fn full(&self) -> bool {
        self.base.full()
    }
}

/// A reducer that combines inside a captured context.
struct ContextReducer<R> {
    base: R,
    context: CapturedContext,
}

impl<T, R: Reducer<T>> Reducer<T> for ContextReducer<R> {
    fn reduce(self, left: T, right: T) -> T {
        let Self { base, context } = self;

        context.run(|| base.reduce(left, right))
    }
}
