//! Page d'accueil. Herite du title/description racine.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::CorpusStatsResponse;

use crate::api::ApiClient;
use crate::components::search::HeroSearch;
use crate::components::ui::Skeleton;
use crate::pages::law_page::data::{sendable, PageError};
use crate::seo::SITE_JSONLD;

#[component]
pub fn Landing() -> impl IntoView {
    crate::pages::cache_public_5_min();
    view! {
        <leptos_meta::Script type_="application/ld+json">{SITE_JSONLD}</leptos_meta::Script>
        <div class="relative flex flex-1 flex-col">
            <HeroSearch />
            <McpCta />
            <div class="mt-auto">
                <Stats />
            </div>
        </div>
    }
}

#[component]
fn McpCta() -> impl IntoView {
    view! {
        <section class="mx-auto w-full max-w-3xl px-4 py-6 sm:px-6">
            <div class="flex flex-col gap-4 rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)] px-6 py-5 sm:flex-row sm:items-center sm:justify-between">
                <div class="flex flex-col gap-1">
                    <p class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                        "Connecteur MCP"
                    </p>
                    <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">
                        "Connectez votre IA (Claude, ChatGPT, Perplexity, Mistral…) au droit français."
                    </p>
                </div>
                <A
                    href="/mcp-guide"
                    attr:class="shrink-0 rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90"
                >
                    "Configurer"
                </A>
            </div>
        </section>
    }
}

#[component]
fn Stats() -> impl IntoView {
    let stats = Resource::new(|| (), |_| sendable(fetch_stats()));
    view! {
        <Suspense fallback=StatsSkeleton>
            {move || Suspend::new(async move {
                stats.await.ok().map(|s| view! { <StatsGrid stats=s /> })
            })}
        </Suspense>
    }
}

/// Charge les compteurs corpus (`GET /internal/corpus-stats`).
async fn fetch_stats() -> Result<CorpusStatsResponse, PageError> {
    ApiClient::from_context()
        .fetch_corpus_stats()
        .await
        .map_err(PageError::from)
}

#[component]
fn StatsGrid(stats: CorpusStatsResponse) -> impl IntoView {
    view! {
        <section class="mx-auto grid w-full max-w-5xl grid-cols-1 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] sm:grid-cols-2">
            <Stat label="Décisions" value=format_thousands(stats.decisions_count) href="/juridictions" />
            <Stat label="Textes normatifs" value=format_thousands(stats.texts_count) href="/normes" />
        </section>
    }
}

#[component]
fn StatsSkeleton() -> impl IntoView {
    view! {
        <section class="mx-auto w-full max-w-5xl">
            <div class="grid w-full grid-cols-1 gap-px overflow-hidden rounded-lg border border-[var(--color-rule)] bg-[var(--color-rule)] sm:grid-cols-2">
                {(0..2)
                    .map(|_| {
                        view! {
                            <div class="flex flex-col gap-2 bg-[var(--color-parchment)] px-6 py-5">
                                <Skeleton class="h-3 w-24" />
                                <Skeleton class="h-6 w-16" />
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
        </section>
    }
}

#[component]
fn Stat(label: &'static str, value: String, href: &'static str) -> impl IntoView {
    view! {
        <A
            href=href
            attr:class="group flex flex-col gap-1 bg-[var(--color-parchment)] px-6 py-5 no-underline transition-colors hover:bg-[var(--color-vellum)]"
        >
            <span class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)] transition-colors group-hover:text-[var(--color-ink)]">
                {label}
            </span>
            <span class="font-sans text-2xl tabular-nums text-[var(--color-ink)]">{value}</span>
        </A>
    }
}

/// Groupe les milliers avec l'espace fine insécable (séparateur français).
fn format_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}
