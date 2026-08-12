//! Page `/connexion`.
//!
//! Formulaire multi-actions (connexion, inscription, OAuth Google, renvoi de
//! confirmation, mot de passe oublie). Tout le flux est client-side : la session
//! Supabase vit en `localStorage`, absente au SSR. Le SSR rend la coquille ; un
//! effet client lit le hash d'erreur eventuel, redirige si deja connecte, puis
//! cable les handlers sur le shim auth.
//!
//! Heberge aussi les modules partages de la tranche auth, reutilises par
//! reset-password et authorize-mcp :
//! - `auth_errors` : traduction FR des erreurs Supabase.
//! - `browser` : bindings DOM + auth-gap (resend / updateUser / fetch OAuth).

#[cfg(any(feature = "hydrate", test))]
#[path = "auth_errors.rs"]
pub(crate) mod auth_errors;
#[cfg(feature = "hydrate")]
#[path = "browser.rs"]
pub(crate) mod browser;

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

const PASSWORD_HINT: &str =
    "Au moins 8 caractères, dont une minuscule, une majuscule et un chiffre.";

#[component]
pub fn LoginPage() -> impl IntoView {
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let is_sign_up = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let confirm_sent = RwSignal::new(false);
    let reset_sent = RwSignal::new(false);
    let resend_notice = RwSignal::new(None::<String>);
    let show_resend = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    let navigate = leptos_router::hooks::use_navigate();
    #[cfg(feature = "hydrate")]
    let query = leptos_router::hooks::use_query_map();

    #[cfg(feature = "hydrate")]
    {
        let navigate = navigate.clone();
        Effect::new(move |_| {
            let next = next_url(query.with(|q| q.get("next")));
            let navigate = navigate.clone();
            if let Some(hash_err) = browser::read_auth_hash_error() {
                error.set(Some(hash_err.message));
                if hash_err.code == "otp_expired" || hash_err.code == "access_denied" {
                    show_resend.set(true);
                }
                browser::clear_location_hash();
                return;
            }
            leptos::task::spawn_local(async move {
                if browser::has_session().await {
                    navigate(&next, Default::default());
                }
            });
        });
    }

    let on_submit = Callback::new(move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        #[cfg(feature = "hydrate")]
        {
            let next = next_url(query.with_untracked(|q| q.get("next")));
            let navigate = navigate.clone();
            let (em, pw, signup) = (
                email.get_untracked(),
                password.get_untracked(),
                is_sign_up.get_untracked(),
            );
            error.set(None);
            loading.set(true);
            leptos::task::spawn_local(async move {
                if signup {
                    match crate::auth::sign_up(&em, &pw, &signup_redirect(&next)).await {
                        Ok(()) => confirm_sent.set(true),
                        Err(msg) => error.set(Some(msg)),
                    }
                } else {
                    match crate::auth::sign_in_password(&em, &pw).await {
                        Ok(()) => navigate(&next, Default::default()),
                        Err(msg) => {
                            // Le shim a deja traduit : le cas
                            // `email_not_confirmed` se detecte sur le message FR.
                            if msg.starts_with("Email non confirmé") {
                                show_resend.set(true);
                            }
                            error.set(Some(msg));
                        }
                    }
                }
                loading.set(false);
            });
        }
    });

    let on_oauth = Callback::new(move |_: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            let next = next_url(query.with_untracked(|q| q.get("next")));
            error.set(None);
            leptos::task::spawn_local(async move {
                let redirect_to = format!("{}{next}", browser::location_origin());
                if let Err(msg) = crate::auth::sign_in_oauth("google", &redirect_to).await {
                    error.set(Some(msg));
                }
            });
        }
    });

    let on_resend = Callback::new(move |_: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            let em = email.get_untracked();
            if em.is_empty() {
                error.set(Some(
                    "Saisissez votre adresse email pour renvoyer le lien.".to_string(),
                ));
                return;
            }
            error.set(None);
            resend_notice.set(None);
            loading.set(true);
            leptos::task::spawn_local(async move {
                match browser::resend_signup(&em).await {
                    Ok(()) => {
                        resend_notice.set(Some(
                            "Email de confirmation renvoyé. Vérifiez votre boîte (et le dossier indésirable)."
                                .to_string(),
                        ));
                        show_resend.set(false);
                    }
                    Err(msg) => error.set(Some(msg)),
                }
                loading.set(false);
            });
        }
    });

    let on_reset = Callback::new(move |_: leptos::ev::MouseEvent| {
        #[cfg(feature = "hydrate")]
        {
            let em = email.get_untracked();
            if em.is_empty() {
                error.set(Some(
                    "Saisissez votre adresse email pour recevoir le lien de réinitialisation."
                        .to_string(),
                ));
                return;
            }
            error.set(None);
            loading.set(true);
            leptos::task::spawn_local(async move {
                let redirect_to =
                    format!("{}/reinitialiser-mot-de-passe", browser::location_origin());
                match crate::auth::reset_password(&em, &redirect_to).await {
                    Ok(()) => reset_sent.set(true),
                    Err(msg) => error.set(Some(msg)),
                }
                loading.set(false);
            });
        }
    });

    view! {
        <Title text="Connexion - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <Show
            when=move || confirm_sent.get() || reset_sent.get()
            fallback=move || {
                view! {
                    <LoginForm
                        email=email
                        password=password
                        is_sign_up=is_sign_up
                        error=error
                        loading=loading
                        resend_notice=resend_notice
                        show_resend=show_resend
                        on_submit=on_submit
                        on_oauth=on_oauth
                        on_resend=on_resend
                        on_reset=on_reset
                    />
                }
            }
        >
            <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Email envoyé"</h1>
                <p class="text-[var(--color-ink-muted)]">
                    {move || {
                        if reset_sent.get() {
                            "Si un compte existe pour cette adresse, un lien de réinitialisation vous a été envoyé. Ouvrez-le sur ce navigateur."
                        } else {
                            "Vérifiez votre boîte mail et cliquez sur le lien de confirmation pour activer votre compte."
                        }
                    }}
                </p>
            </div>
        </Show>
    }
}

/// Formulaire de connexion / inscription (le contenu hors ecran « Email envoyé »).
#[component]
fn LoginForm(
    email: RwSignal<String>,
    password: RwSignal<String>,
    is_sign_up: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    loading: RwSignal<bool>,
    resend_notice: RwSignal<Option<String>>,
    show_resend: RwSignal<bool>,
    on_submit: Callback<leptos::ev::SubmitEvent>,
    on_oauth: Callback<leptos::ev::MouseEvent>,
    on_resend: Callback<leptos::ev::MouseEvent>,
    on_reset: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <div class="flex flex-col gap-2">
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">
                    {move || if is_sign_up.get() { "Créer un compte" } else { "Connexion" }}
                </h1>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    "Accédez à l'ensemble des fonctionnalités LibreJustice."
                </p>
            </div>

            <div class="flex flex-col gap-3">
                <button
                    type="button"
                    disabled=move || loading.get()
                    on:click=move |ev| on_oauth.run(ev)
                    class="flex items-center justify-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-4 py-2.5 text-sm text-[var(--color-ink)] transition-colors hover:bg-[var(--color-rule)] disabled:opacity-50"
                >
                    "Continuer avec Google"
                </button>
            </div>

            <div class="flex items-center gap-3">
                <div class="h-px flex-1 bg-[var(--color-rule)]"></div>
                <span class="text-xs text-[var(--color-ink-subtle)]">"ou"</span>
                <div class="h-px flex-1 bg-[var(--color-rule)]"></div>
            </div>

            <form on:submit=move |ev| on_submit.run(ev) class="flex flex-col gap-4">
                <div class="flex flex-col gap-1.5">
                    <label for="email" class="text-sm text-[var(--color-ink-muted)]">"Email"</label>
                    <input
                        id="email"
                        type="email"
                        autocomplete="email"
                        required
                        prop:value=move || email.get()
                        on:input=move |ev| email.set(event_target_value(&ev))
                        class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                    />
                </div>

                <div class="flex flex-col gap-1.5">
                    <label for="password" class="text-sm text-[var(--color-ink-muted)]">
                        "Mot de passe"
                    </label>
                    <input
                        id="password"
                        type="password"
                        autocomplete=move || {
                            if is_sign_up.get() { "new-password" } else { "current-password" }
                        }
                        required
                        prop:value=move || password.get()
                        on:input=move |ev| password.set(event_target_value(&ev))
                        class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                    />
                    <Show when=move || is_sign_up.get()>
                        <p class="text-xs text-[var(--color-ink-subtle)]">{PASSWORD_HINT}</p>
                    </Show>
                    <Show when=move || !is_sign_up.get()>
                        <button
                            type="button"
                            disabled=move || loading.get()
                            on:click=move |ev| on_reset.run(ev)
                            class="self-end text-xs text-[var(--color-ink-muted)] hover:text-[var(--color-ink)] disabled:opacity-50"
                        >
                            "Mot de passe oublié ?"
                        </button>
                    </Show>
                </div>

                <Show when=move || error.get().is_some()>
                    <div class="flex flex-col gap-2">
                        <p class="text-sm text-red-600">{move || error.get().unwrap_or_default()}</p>
                        <Show when=move || show_resend.get()>
                            <button
                                type="button"
                                disabled=move || loading.get()
                                on:click=move |ev| on_resend.run(ev)
                                class="self-start text-sm text-[var(--color-accent)] underline hover:opacity-80 disabled:opacity-50"
                            >
                                "Renvoyer l'email de confirmation"
                            </button>
                        </Show>
                    </div>
                </Show>
                <Show when=move || resend_notice.get().is_some()>
                    <p class="text-sm text-[var(--color-ink-muted)]">
                        {move || resend_notice.get().unwrap_or_default()}
                    </p>
                </Show>

                <button
                    type="submit"
                    disabled=move || loading.get()
                    class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                >
                    {move || {
                        if loading.get() {
                            "…"
                        } else if is_sign_up.get() {
                            "Créer le compte"
                        } else {
                            "Se connecter"
                        }
                    }}
                </button>
            </form>

            <div class="flex flex-col gap-3 text-center text-sm text-[var(--color-ink-muted)]">
                <button
                    type="button"
                    on:click=move |_| is_sign_up.update(|v| *v = !*v)
                    class="hover:text-[var(--color-ink)]"
                >
                    {move || {
                        if is_sign_up.get() {
                            "Déjà un compte ? Se connecter"
                        } else {
                            "Pas encore de compte ? Créer"
                        }
                    }}
                </button>
            </div>
        </div>
    }
}

/// Resout la destination de `?next=` : chemin interne relatif (`/` non suivi de
/// `/`), sinon l'accueil. Les points d'entrée de la connexion posent tous
/// `?next=` (page courante ou page protégée demandée) : le défaut ne sert que
/// quand la destination est inconnue.
#[cfg(feature = "hydrate")]
pub(crate) fn next_url(raw: Option<String>) -> String {
    let raw = raw.unwrap_or_else(|| "/".to_string());
    let bytes = raw.as_bytes();
    let internal = bytes.first() == Some(&b'/') && bytes.get(1) != Some(&b'/');
    if internal {
        raw
    } else {
        "/".to_string()
    }
}

/// Lien de redirection du mail de confirmation d'inscription : origine + la
/// destination `?next=` (l'accueil par défaut, cf. `next_url`).
///
/// Le lien doit ramener là où l'inscription a commencé, paramètres OAuth de
/// `/authorize-mcp` compris.
#[cfg(feature = "hydrate")]
fn signup_redirect(next: &str) -> String {
    format!("{}{next}", browser::location_origin())
}
