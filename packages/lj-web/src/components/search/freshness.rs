//! A results page stays interactive during a refetch, so anything read off the
//! last response is a claim about a list that is no longer the one being
//! served. Whatever asserts reads through [`answered`] and gets `None` for the
//! duration; whatever merely holds the page's shape does not, a card view
//! flipping on every refetch being worse than a stale default.

use leptos::prelude::*;

/// `None` while `value` answers a request other than the one on screen.
///
/// The staleness horizon is chosen at the call site through `answers`: a count
/// key survives a page change, a full key does not.
pub fn answered<T, K>(
    value: Signal<Option<T>>,
    answers: Signal<Option<K>>,
    asked: Signal<K>,
) -> Signal<Option<T>>
where
    T: Clone + Send + Sync + 'static,
    K: PartialEq + Clone + Send + Sync + 'static,
{
    Signal::derive(move || value.get().filter(|_| answers.get() == Some(asked.get())))
}

/// The same question, for a caller with only a boolean to draw from it.
pub fn is_stale<K>(answers: Signal<Option<K>>, asked: Signal<K>) -> Signal<bool>
where
    K: PartialEq + Clone + Send + Sync + 'static,
{
    Signal::derive(move || answers.get() != Some(asked.get()))
}
