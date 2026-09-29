//! `FilterDropdown` — coquille générique des dropdowns de la barre de filtres :
//! bouton `Libellé ▾` + panneau flottant multi-sélection (pas de fermeture au
//! choix).
//!
//! Panneau en `position: fixed` (coordonnées mesurées à l'ouverture) : il
//! échappe à l'`overflow-x-auto` de la barre, qui rognerait un panneau
//! `absolute`. En contrepartie un panneau fixe ne suit pas le scroll de page :
//! on le ferme au scroll fenêtre (le scroll interne des listes, capé +
//! anti-chaining, ne remonte pas à la fenêtre).
//!
//! Un seul dropdown ouvert à la fois : contexte `OpenDropdown` fourni par la
//! barre (id du dropdown ouvert, `None` = tous fermés).

use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use crate::components::ui::panel::{dismiss, Dismiss};
use crate::helpers::cn;

/// Id du dropdown ouvert dans la barre (`None` = aucun). Fourni par la barre de
/// filtres ; chaque `FilterDropdown` s'y compare par son `id`.
#[derive(Clone, Copy)]
pub struct OpenDropdown(pub RwSignal<Option<String>>);

#[component]
pub fn FilterDropdown(
    /// Identifiant unique dans la barre (clé du contexte « un seul ouvert »).
    #[prop(into)]
    id: String,
    #[prop(into)] label: String,
    /// Nombre de sélections actives : badge + style accent sur le bouton.
    #[prop(into)]
    active_count: Signal<usize>,
    children: ChildrenFn,
) -> impl IntoView {
    let open_id = use_context::<OpenDropdown>()
        .map(|c| c.0)
        .unwrap_or_else(|| RwSignal::new(None));
    // `id` est possédé (les axes du pot Annonces sont déclarés par le serveur,
    // pas des littéraux) : on le stocke pour que les clôtures le comparent sans
    // le déplacer.
    let id = StoredValue::new(id);
    let is_open =
        Signal::derive(move || id.with_value(|id| open_id.get().as_deref() == Some(id.as_str())));
    // Lazy-mount : le contenu (arbres à centaines de lignes) n'est construit
    // qu'à la première ouverture, puis gardé monté — l'état interne (saisie,
    // déplis) vit dans les enfants et doit survivre à une fermeture.
    let has_opened = RwSignal::new(false);

    let container_ref = NodeRef::<leptos::html::Div>::new();
    let button_ref = NodeRef::<leptos::html::Button>::new();
    // Coordonnées viewport du panneau `(top, left, max_height)` mesurées à
    // l'ouverture. `None` au SSR / avant mesure.
    let coords = RwSignal::new(None::<(f64, f64, f64)>);

    #[cfg(feature = "hydrate")]
    dismiss(
        container_ref,
        move || is_open.get_untracked(),
        move || open_id.set(None),
        Dismiss {
            escape: true,
            scroll: true,
        },
    );

    // Ouverture : mesure du bouton, ancrage bas-gauche, largeur fixe `w-80`
    // clampée au viewport, hauteur capée à l'espace sous le bouton.
    let on_toggle = move |_| {
        if is_open.get_untracked() {
            open_id.set(None);
            return;
        }
        #[cfg(feature = "hydrate")]
        if let Some(btn) = button_ref.get_untracked() {
            const PANEL_W: f64 = 320.0;
            let rect = btn.get_bounding_client_rect();
            let win_h = window()
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let win_w = window()
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let top = rect.bottom() + 4.0;
            let left = rect.left().min(win_w - PANEL_W - 8.0).max(8.0);
            let max_h = (win_h - top - 16.0).clamp(160.0, 480.0);
            coords.set(Some((top, left, max_h)));
        }
        has_opened.set(true);
        open_id.set(Some(id.get_value()));
    };

    let button_class = move || {
        cn([
            "flex shrink-0 items-center gap-1.5 rounded border px-2.5 py-1 text-xs transition-colors",
            if active_count.get() > 0 {
                "border-[var(--color-accent)] text-[var(--color-accent)]"
            } else {
                "border-[var(--color-rule)] text-[var(--color-ink)] hover:border-[var(--color-ink)]"
            },
        ])
    };

    // Masquage par `.lj-panel` (visibility), pas `display:none` : le panneau
    // reste dans le flux de rendu, donc sa fermeture s'anime.
    let panel_style = move || {
        coords
            .get()
            .map(|(top, left, max_h)| {
                format!("position:fixed; top:{top}px; left:{left}px; max-height:{max_h}px;")
            })
            // Avant toute mesure (SSR, première frame) : hors du flux, pour ne
            // pas élargir le conteneur `inline-flex` de la barre.
            .unwrap_or_else(|| "position:fixed; top:0; left:0;".to_string())
    };

    let children = StoredValue::new(children);

    view! {
        <div node_ref=container_ref class="relative inline-flex">
            <button
                node_ref=button_ref
                type="button"
                aria-expanded=move || is_open.get().then_some("true")
                on:click=on_toggle
                class=button_class
            >
                <span class="whitespace-nowrap">{label}</span>
                <Show when=move || { active_count.get() > 0 }>
                    <span class="rounded-full bg-[var(--color-accent)] px-1.5 text-[10px] leading-4 text-[var(--color-accent-foreground)]">
                        {move || active_count.get()}
                    </span>
                </Show>
                <svg
                    viewBox="0 0 12 8"
                    class=move || {
                        format!(
                            "h-2 w-2.5 text-[var(--color-ink-subtle)] transition-transform {}",
                            if is_open.get() { "rotate-180" } else { "" },
                        )
                    }
                    fill="none"
                    aria-hidden="true"
                >
                    <path
                        d="M1 1.5l5 5 5-5"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                </svg>
            </button>
            // `overflow-hidden` + flex-col : le panneau est capé par la
            // max-height mesurée, la zone scrollable vit dans les enfants
            // (`min-h-0 flex-1 overflow-y-auto`) — le header (recherche
            // intra-facette) reste fixe au-dessus.
            <div
                style=panel_style
                data-open=move || is_open.get().then_some("true")
                class="lj-panel z-50 flex w-80 flex-col overflow-hidden rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3 shadow-lg"
            >
                <Show when=move || has_opened.get()>{children.with_value(|c| c())}</Show>
            </div>
        </div>
    }
}
