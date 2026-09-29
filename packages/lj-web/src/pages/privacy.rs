//! Politique de confidentialité. `<Title>` propre, hérite de la description
//! racine.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::components::A;

#[component]
pub fn Privacy() -> impl IntoView {
    crate::pages::cache_public_5_min();
    view! {
        <Title text="Confidentialité - LibreJustice" />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                "Politique de confidentialité"
            </h1>
            <p class="mt-2 text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "En vigueur au 28 septembre 2026"
            </p>

            <Section title="Responsable de traitement">
                <p>
                    "Iris Sainte Fare Garnot (entrepreneur individuel, SIRET 922 108 600 00029), éditeur de "
                    <strong>"librejustice.fr"</strong>
                    ". Contact : "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                    "."
                </p>
            </Section>

            <Section title="Données collectées">
                <p>
                    <strong>"Compte utilisateur."</strong>
                    " Votre adresse e-mail, votre nom si vous l'avez indiqué, et votre identifiant Google ou Microsoft si vous vous connectez par l'un d'eux. Nous notons aussi la langue de votre navigateur, votre pays, la date de votre inscription et celle de votre dernière visite. Si vous connectez un assistant IA à votre compte, nous lui transmettons votre adresse e-mail. Tout est effacé quand vous supprimez votre compte. Nous gardons aussi la trace des e-mails que nous vous envoyons et de votre éventuel désabonnement."
                </p>
                <p class="mt-3">
                    <strong>"Signets."</strong>
                    " Les décisions que vous avez mises de côté. Vous seul les voyez, et vous pouvez les retirer à tout moment."
                </p>
                <p class="mt-3">
                    <strong>"Journaux techniques."</strong>
                    " Le fonctionnement du service repose sur des journaux applicatifs et des traces de performance, qui servent à mesurer les temps de réponse et à diagnostiquer les erreurs. Ils peuvent contenir les paramètres des requêtes, dont les termes de recherche, ainsi que le type de client utilisé (navigateur, API ou assistant IA). Ces données sont pseudonymisées : aucun nom ni adresse e-mail n'y figure, l'utilisateur n'y apparaît que sous un identifiant technique dérivé. Elles sont supprimées automatiquement au bout de 30 jours."
                </p>
                <p class="mt-3">
                    <strong>"Mesure d'audience."</strong>
                    " Pour suivre l'utilisation du service et orienter ses évolutions, nous produisons des indicateurs de fréquentation consolidés à la journée : le volume des recherches et des consultations, et leur répartition par canal d'accès. Ces indicateurs agrégés reposent sur de simples décomptes, sans conserver les paramètres ou le contenu des requêtes individuelles. Ils ne font appel à aucun cookie ni traceur, et aucune adresse IP n'est enregistrée."
                </p>
                <p class="mt-3">
                    <strong>"Signalement d'un problème."</strong>
                    " Le texte de votre message, l'adresse de la page concernée (y compris les termes de recherche lorsqu'elle en porte), le fait que vous étiez connecté ou non, et votre adresse e-mail si vous la donnez ou si vous êtes connecté. Ces éléments arrivent dans notre boîte de support, où ils sont conservés pour le suivi de votre demande."
                </p>
            </Section>

            <Section title="Cookies et stockage local">
                <p>
                    "Nous n'utilisons aucun cookie publicitaire ni aucun traceur soumis à consentement. Si vous êtes connecté, votre navigateur garde seulement un jeton de session, qui s'efface à la déconnexion."
                </p>
            </Section>

            <Section title="Sous-traitants">
                <ul class="list-disc space-y-2 pl-5">
                    <li>
                        <strong>"OVH"</strong>
                        " (Roubaix) : hébergement, base de données, télémétrie, messagerie."
                    </li>
                    <li>
                        <strong>"Cloudflare Lda."</strong>
                        " (Lisbonne) : CDN, protection, mesure d'audience."
                    </li>
                    <li>
                        <strong>"Grafana Cloud"</strong>
                        " (New York, données à Francfort) : logs, traces et mesure d'audience."
                    </li>
                    <li>
                        <strong>"Mistral AI"</strong>
                        " (Paris) : résumés des décisions et reclassement des résultats de recherche."
                    </li>
                    <li>
                        <strong>"Resend"</strong>
                        " (San Francisco, envoi via AWS Dublin) : e-mails de service (confirmation, réinitialisation) et e-mails d'information."
                    </li>
                </ul>
            </Section>

            <Section title="Personnes citées dans les données publiées">
                <p>
                    "Les décisions indexées sont publiées par l'État, pseudonymisées à la source (Judilibre, Open Data du Conseil d'État). Les fiches de l'annuaire des acteurs du contentieux (avocats, entités) sont dérivées de ces décisions publiques."
                </p>
                <p class="mt-3">
                    "Si vous êtes cité dans une décision ou une fiche (anonymisation défaillante, donnée inexacte, ou simple volonté de ne plus apparaître), vous pouvez exercer votre droit d'opposition ou demander une correction par courriel à "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                    ", en indiquant l'adresse de la page concernée. Réponse sous un mois au plus."
                </p>
            </Section>

            <Section title="Vos droits">
                <p>
                    "Vous disposez d'un droit d'accès, de rectification, d'effacement, de limitation, d'opposition et de portabilité (RGPD, loi Informatique et Libertés)."
                </p>
                <p class="mt-3">
                    "Le bouton « Supprimer mon compte » sur votre "
                    <A
                        href="/profil"
                        attr:class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                    >
                        "page profil"
                    </A>
                    " efface immédiatement votre compte et vos signets."
                </p>
                <p class="mt-3">
                    "Pour toute autre demande, écrivez à "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="mailto:contact@librejustice.fr"
                    >
                        "contact@librejustice.fr"
                    </a>
                    " ou saisissez la "
                    <a
                        class="text-[var(--color-accent)] underline-offset-4 hover:underline"
                        href="https://www.cnil.fr/fr/plaintes"
                        rel="noreferrer"
                        target="_blank"
                    >
                        "CNIL"
                    </a>
                    "."
                </p>
            </Section>

            <div class="mt-auto pt-12">
                <div class="border-t border-[var(--color-rule)] pt-6">
                    <A
                        href="/"
                        attr:class="inline-flex items-center gap-1.5 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        <span aria-hidden="true">"←"</span>
                        " Retour à l'accueil"
                    </A>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Section(title: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="mt-10">
            <h2 class="font-sans text-xl text-[var(--color-ink)]">{title}</h2>
            <div class="mt-3 space-y-2 text-[var(--color-ink-muted)] [&_strong]:text-[var(--color-ink)] [&_code]:rounded [&_code]:bg-[var(--color-parchment)] [&_code]:px-1 [&_code]:py-0.5 [&_code]:text-xs">
                {children()}
            </div>
        </section>
    }
}
