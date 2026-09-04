//! Selecteur de theme (Systeme / Clair / Sombre).
//!
//! Pont localStorage `lj-theme` + `matchMedia`. L'anti-FOUC inline d'`app.rs`
//! applique deja `.dark` avant le premier paint ; ce composant gere le choix
//! explicite et le suivi de `prefers-color-scheme` en mode Systeme.

use leptos::prelude::*;

/// Theme choisi.
#[derive(Clone, Copy, PartialEq)]
enum Theme {
    System,
    Light,
    Dark,
}

// Conversions chaine <-> Theme : utilisees seulement par le pont localStorage
// (hydrate) ; au SSR le theme initial est `System` en dur.
#[cfg(feature = "hydrate")]
impl Theme {
    /// Valeur persistee (`"light"`/`"dark"`/`"system"`).
    fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_str(s: &str) -> Theme {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

#[cfg(feature = "hydrate")]
mod bridge {
    use wasm_bindgen::prelude::*;

    const STORAGE_KEY: &str = "lj-theme";
    const AUTH_FLAG: &str = "lj-auth";
    const PREFERS_DARK: &str = "(prefers-color-scheme: dark)";

    /// `localStorage`, ou `None` quand il est refuse (mode prive strict, cookies
    /// bloques) : le theme retombe alors sur `System` a chaque chargement.
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    fn prefers_dark() -> bool {
        web_sys::window()
            .and_then(|w| w.match_media(PREFERS_DARK).ok().flatten())
            .is_some_and(|mq| mq.matches())
    }

    /// Theme persiste : `"light"` | `"dark"` | `"system"` (defaut).
    pub fn read_theme() -> String {
        let stocke = storage().and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
        match stocke.as_deref() {
            Some(v @ ("light" | "dark")) => v.to_string(),
            _ => "system".to_string(),
        }
    }

    /// Persiste le theme. `"system"` est l'absence de choix, donc un retrait.
    pub fn persist_theme(theme: &str) {
        let Some(storage) = storage() else {
            return;
        };
        let _ = if theme == "system" {
            storage.remove_item(STORAGE_KEY)
        } else {
            storage.set_item(STORAGE_KEY, theme)
        };
    }

    fn basculer_dark(sombre: bool) {
        if let Some(racine) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = racine.class_list().toggle_with_force("dark", sombre);
        }
    }

    /// Applique le theme : `.dark` selon `dark` | (`system` && prefers-dark).
    pub fn apply_theme(theme: &str) {
        basculer_dark(theme == "dark" || (theme == "system" && prefers_dark()));
    }

    /// Synchronise le flag d'auth `lj-auth` puis (re)applique le theme.
    /// Semantique (cf. anti-FOUC d'`app.rs`) : le sombre est reserve aux
    /// connectes — anonyme => flag retire + theme clair force.
    pub fn sync_auth_theme(authenticated: bool) {
        if let Some(storage) = storage() {
            let _ = if authenticated {
                storage.set_item(AUTH_FLAG, "1")
            } else {
                storage.remove_item(AUTH_FLAG)
            };
        }
        if authenticated {
            apply_theme(&read_theme());
        } else {
            basculer_dark(false);
        }
    }

    /// Abonne au changement de `prefers-color-scheme`. `Drop` desabonne.
    pub fn subscribe_prefers_dark(cb: impl FnMut() + 'static) -> PrefersDarkSubscription {
        let closure = Closure::<dyn FnMut()>::new(cb);
        let media = web_sys::window().and_then(|w| w.match_media(PREFERS_DARK).ok().flatten());
        if let Some(mq) = &media {
            let _ = mq.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
        }
        PrefersDarkSubscription { closure, media }
    }

    pub struct PrefersDarkSubscription {
        closure: Closure<dyn FnMut()>,
        media: Option<web_sys::MediaQueryList>,
    }

    impl Drop for PrefersDarkSubscription {
        fn drop(&mut self) {
            if let Some(mq) = &self.media {
                let _ = mq.remove_event_listener_with_callback(
                    "change",
                    self.closure.as_ref().unchecked_ref(),
                );
            }
        }
    }
}

/// Bridge thème ↔ auth (appelé depuis `App` à l'hydratation) : pose/retire le
/// flag `lj-auth` et (ré)applique le thème selon la session. Sombre réservé aux
/// connectés (cf. anti-FOUC d'`app.rs`).
#[cfg(feature = "hydrate")]
pub fn sync_auth_theme(authenticated: bool) {
    bridge::sync_auth_theme(authenticated);
}

#[component]
pub fn ThemeSettings() -> impl IntoView {
    // Valeur initiale : lue du localStorage cote client, `System` au SSR.
    #[cfg(feature = "hydrate")]
    let initial = Theme::from_str(&bridge::read_theme());
    #[cfg(not(feature = "hydrate"))]
    let initial = Theme::System;

    let theme = RwSignal::new(initial);

    #[cfg(feature = "hydrate")]
    {
        // Persistance + application a chaque changement.
        Effect::new(move |_| {
            let t = theme.get();
            bridge::persist_theme(t.as_str());
            bridge::apply_theme(t.as_str());
        });
        // En mode Systeme : suivre `prefers-color-scheme`. L'abonnement vit dans un
        // `StoredValue` local (non-`Send`, normal cote wasm) : chaque passage remplace
        // l'abonnement precedent — le `Drop` de l'ancien desabonne.
        let sub_slot: StoredValue<
            Option<bridge::PrefersDarkSubscription>,
            leptos::reactive::owner::LocalStorage,
        > = StoredValue::new_local(None);
        Effect::new(move |_| {
            if theme.get() != Theme::System {
                sub_slot.set_value(None);
                return;
            }
            let subscription = bridge::subscribe_prefers_dark(move || {
                bridge::apply_theme("system");
            });
            sub_slot.set_value(Some(subscription));
        });
    }

    let options = [
        (Theme::System, "Système"),
        (Theme::Light, "Clair"),
        (Theme::Dark, "Sombre"),
    ];

    view! {
        <fieldset class="flex flex-col gap-2">
            <legend class="sr-only">"Thème de l'interface"</legend>
            <div class="flex gap-2">
                {options
                    .into_iter()
                    .map(|(value, label)| {
                        let active = move || theme.get() == value;
                        view! {
                            <button
                                type="button"
                                on:click=move |_| theme.set(value)
                                aria-pressed=move || active().to_string()
                                class=move || {
                                    crate::helpers::cn([
                                        "flex flex-1 items-center justify-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors",
                                        if active() {
                                            "border-[var(--color-ink)] bg-[var(--color-vellum)] text-[var(--color-ink)]"
                                        } else {
                                            "border-[var(--color-rule)] text-[var(--color-ink-muted)] hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                                        },
                                    ])
                                }
                            >
                                <ThemeIcon theme=value />
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <p class="text-xs text-[var(--color-ink-subtle)]">
                "Préférence enregistrée localement sur cet appareil."
            </p>
        </fieldset>
    }
}

/// Icone SVG inline d'une option de theme.
#[component]
fn ThemeIcon(theme: Theme) -> impl IntoView {
    match theme {
        Theme::Light => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                aria-hidden="true"
            >
                <circle cx="10" cy="10" r="3.5"></circle>
                <path d="M10 2v2M10 16v2M18 10h-2M4 10H2M15.66 4.34l-1.41 1.41M5.75 14.25l-1.41 1.41M15.66 15.66l-1.41-1.41M5.75 5.75 4.34 4.34"></path>
            </svg>
        }
        .into_any(),
        Theme::Dark => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linejoin="round"
                aria-hidden="true"
            >
                <path d="M16.5 12.5A6.5 6.5 0 0 1 7.5 3.5a6.5 6.5 0 1 0 9 9Z"></path>
            </svg>
        }
        .into_any(),
        Theme::System => view! {
            <svg
                width="16"
                height="16"
                viewBox="0 0 20 20"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                aria-hidden="true"
            >
                <rect x="2.5" y="3.5" width="15" height="11" rx="1.5"></rect>
                <path d="M7 17h6M10 14.5v2.5"></path>
            </svg>
        }
        .into_any(),
    }
}
