//! `ResultError` : recherche qui n'a pas abouti, et la conduite à tenir.

use crate::api::client::SearchFailure;
use leptos::prelude::*;

#[component]
pub fn ResultError(failure: SearchFailure) -> impl IntoView {
    view! {
        <div
            role="alert"
            class="border-t border-[var(--color-accent)] py-12 text-[var(--color-accent)]"
        >
            <p class="text-xs uppercase tracking-[0.2em]">"Erreur"</p>
            <p class="mt-2 font-sans text-xl">{failure.title}</p>
            <p class="mt-1 text-sm text-[var(--color-ink-muted)]">{failure.hint}</p>
        </div>
    }
}
