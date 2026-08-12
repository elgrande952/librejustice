//! Recensement des profils d'acheteurs : à quel moteur appartient un hôte ?
//!
//! Les moteurs multi-tenants (Atexo, Safetender) n'ont pas de point d'entrée
//! national : il faut la liste de leurs instances. On la tire des URLs citées par
//! les avis BOAMP (l'avis pointe le profil qui porte le DCE), puis on classe chaque
//! hôte : liste `HORS_BOAMP` et domaine connu d'abord, empreinte HTML ensuite,
//! sonde API Atexo en dernier. Sortie : la table `profil_acheteur`.

use std::collections::HashMap;
use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use super::send_text_once;
use crate::error::Result;

const HOTES_PARALLELES: usize = 48;

/// Un hôte recensé, avec le moteur identifié et son poids en avis BOAMP.
#[derive(Debug)]
pub struct Profil {
    pub host: String,
    pub moteur: Option<&'static str>,
    pub avis_boamp: i32,
}

/// Moteurs reconnaissables au seul domaine (un point d'entrée national).
const PAR_DOMAINE: [(&str, &str); 15] = [
    ("aws", r"(^|\.)(marches-publics\.info|aws-achat\.info)$"),
    ("achatpublic", r"(^|\.)achatpublic\.com$"),
    ("atline", r"(^|\.)marches-securises\.fr$"),
    ("dematis", r"(^|\.)e-marchespublics\.com$"),
    ("synapse", r"(^|\.)synapse-entreprises\.com$"),
    ("klekoon", r"(^|\.)klekoon\.com$"),
    ("safetender", r"(^|\.)safetender\.com$"),
    ("xmarches", r"(^|\.)xmarches\.fr$"),
    ("aji", r"(^|\.)aji-france\.com$"),
    (
        "modula",
        r"(^|\.)(proxilegales\.fr|achatspublicscorse\.com|marches-demat\.com|marchespublics\.pro|antilles-legales\.fr|laprovencemarchespublics\.com)$",
    ),
    // Moteurs identifiés mais non connectés : recensés pour mesurer le résidu de
    // couverture, pas moissonnés.
    ("agysoft", r"(^|\.)agysoft\.marches-publics\.info$"),
    // Centrale des marchés = un portail Modula-Demat de plus (Medialex), hors de
    // portée : WAF Akamai sur toutes les portes sondées, et `robots.txt` en
    // `User-agent: * / Disallow: /`.
    (
        "centraledesmarches",
        r"(^|\.)(centraledesmarches\.com|medialex\.fr)$",
    ),
    ("telerecours", r"(^|\.)telerecours\.fr$"),
    ("eurolegales", r"(^|\.)marchespublics-eurolegales\.com$"),
    // EU-Supply sert une marque par acheteur (`?B=…`) et le connecteur `synapse` n'en
    // moissonne qu'une (`B=SYNAPSE`) : un hôte EU-Supply nu désigne une des autres.
    ("eu-supply", r"(^|\.)eu-supply\.com$"),
];

/// Portails hors du périmètre BOAMP, avec leur moteur. Le recensement dérive des
/// URLs citées par les avis BOAMP : un territoire dont la commande publique relève
/// d'un autre régime n'y publie pas, et son portail n'entrera jamais dans le
/// comptage. La Nouvelle-Calédonie a le sien (compétence propre du Congrès), et
/// `portail.marchespublics.nc` est une instance Atexo MPE standard. Ces hôtes sont
/// injectés au recensement à poids nul et leur moteur est pris ici plutôt que
/// sondé.
const HORS_BOAMP: [(&str, &str); 1] = [("portail.marchespublics.nc", "atexo")];

/// Empreintes HTML, quand le domaine ne dit rien (marques blanches).
const EMPREINTES: [(&str, &str); 10] = [
    (
        "atexo",
        r"Entreprise\.EntrepriseAdvancedSearch|AtexoMpe|MPE_",
    ),
    ("dematis", r"e-marchespublics|Dematis"),
    // Avant `agysoft` : les portails Modula citent « marco agysoft » dans leur meta
    // keywords (texte SEO), et `find` prend la première empreinte.
    ("modula", r"publisher_portail|Modula D"),
    ("agysoft", r"marcoweb|Agysoft"),
    ("atline", r"marches-securises|Atline"),
    ("safetender", r"safetender|Local Trust"),
    ("klekoon", r"klekoon"),
    ("synapse", r"synapse-entreprises"),
    ("achatpublic", r"achatpublic\.com"),
    ("aws", r"marches-publics\.info|aws-achat"),
];

fn par_domaine(host: &str) -> Option<&'static str> {
    static RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        PAR_DOMAINE
            .iter()
            .map(|(m, p)| (*m, Regex::new(p).expect("motif de domaine valide")))
            .collect()
    });
    RES.iter()
        .find(|(_, re)| re.is_match(host))
        .map(|(m, _)| *m)
}

fn empreinte(html: &str) -> Option<&'static str> {
    static RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        EMPREINTES
            .iter()
            .map(|(m, p)| {
                (
                    *m,
                    Regex::new(&format!("(?i){p}")).expect("motif d'empreinte valide"),
                )
            })
            .collect()
    });
    RES.iter()
        .find(|(_, re)| re.is_match(html))
        .map(|(m, _)| *m)
}

/// Sonde l'API Atexo : `hydra:member` présent ⇒ instance MPE. Dernier recours,
/// après l'empreinte HTML (deux requêtes de plus par hôte).
async fn est_atexo(client: &reqwest::Client, host: &str) -> bool {
    for h in [host.to_string(), format!("www.{host}")] {
        let sonde = send_text_once(
            client
                .get(format!("https://{h}/api/v2/consultations"))
                .header("Accept", "application/ld+json")
                .query(&[("itemsPerPage", "1")]),
        )
        .await;
        if sonde.is_some_and(|(_, body)| body.contains("hydra:member")) {
            return true;
        }
    }
    false
}

async fn classer(client: &reqwest::Client, host: String, avis_boamp: i32) -> Profil {
    let profil = |moteur| Profil {
        host: host.clone(),
        moteur,
        avis_boamp,
    };
    if let Some(m) = HORS_BOAMP.iter().find(|(h, _)| *h == host).map(|(_, m)| *m) {
        return profil(Some(m));
    }
    if let Some(m) = par_domaine(&host) {
        return profil(Some(m));
    }
    // La page d'accueil sert deux verdicts d'un coup : l'URL finale (un hôte
    // qui redirige vers son moteur) et l'empreinte HTML.
    for h in [host.clone(), format!("www.{host}")] {
        let Some((url_finale, html)) = send_text_once(client.get(format!("https://{h}/"))).await
        else {
            continue;
        };
        if let Some(m) = par_domaine(&url_finale).or_else(|| empreinte(&html)) {
            return profil(Some(m));
        }
    }
    if est_atexo(client, &host).await {
        profil(Some("atexo"))
    } else {
        profil(None)
    }
}

/// Hôtes cités qui ne servent pas eux-mêmes le profil qu'ils désignent : portails
/// dont la MPE vit sur un sous-domaine, et coquilles de saisie de l'acheteur. La
/// forme canonique est celle qui répond ; l'hôte cité en hérite le moteur au
/// recensement, et le jumelage — qui canonise le même hôte de dépôt — y retrouve le
/// canal sous lequel la moisson a écrit ses lignes. Correspondance littérale,
/// jamais approchée.
const ALIAS_HOTE: [(&str, &str); 65] = [
    // Portails dont la MPE Atexo vit sous `marches.<hôte>` ; l'avis cite tantôt
    // l'apex, tantôt la MPE.
    ("departement13.fr", "marches.departement13.fr"),
    ("loire-atlantique.fr", "marches.loire-atlantique.fr"),
    ("maximilien.fr", "marches.maximilien.fr"),
    ("megalis.bretagne.bzh", "marches.megalis.bretagne.bzh"),
    ("montpellier3m.fr", "marches.montpellier3m.fr"),
    ("rhone.fr", "marches.rhone.fr"),
    ("ternum-bfc.fr", "marches.ternum-bfc.fr"),
    // Megalis, sous son ancien domaine (mort) et sans le point.
    (
        "marches.megalisbretagne.bzh",
        "marches.megalis.bretagne.bzh",
    ),
    (
        "marches.megalisbretagne.org",
        "marches.megalis.bretagne.bzh",
    ),
    // PLACE, dont l'acronyme sert d'hôte autant que les coquilles du domaine.
    ("marchespublics.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-public.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-publics.gouv", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.f", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.ff", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.fr.l", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.fr.la", "marches-publics.gouv.fr"),
    ("marches-publics.gou.fr", "marches-publics.gouv.fr"),
    ("marches-piblics.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-publis.gouv.fr", "marches-publics.gouv.fr"),
    ("marches.publics.gouv.fr", "marches-publics.gouv.fr"),
    ("marchespublic.gouv.fr", "marches-publics.gouv.fr"),
    ("marches_pubics.gouv.fr", "marches-publics.gouv.fr"),
    ("marhces-publics.gouv.fr", "marches-publics.gouv.fr"),
    ("place.gouv.fr", "marches-publics.gouv.fr"),
    ("ww.marchespublics.gouv.fr", "marches-publics.gouv.fr"),
    ("wwww.marches-publics.gouv.fr", "marches-publics.gouv.fr"),
    // Atline.
    ("marches.securises.fr", "marches-securises.fr"),
    ("marches-securises", "marches-securises.fr"),
    ("marches-securise.fr", "marches-securises.fr"),
    ("marche-securises.fr", "marches-securises.fr"),
    ("marches-securises.com", "marches-securises.fr"),
    ("marches-securises.f", "marches-securises.fr"),
    ("marches-securises.fr.la", "marches-securises.fr"),
    ("marches-seucrises.fr", "marches-securises.fr"),
    ("marches_securises.fr", "marches-securises.fr"),
    ("marchessecurises.fr", "marches-securises.fr"),
    ("marches-securisés.fr", "marches-securises.fr"),
    ("marches-sécurises.fr", "marches-securises.fr"),
    ("marchés-securisés.fr", "marches-securises.fr"),
    ("marchés-sécurises.fr", "marches-securises.fr"),
    ("marchés-sécurisés.fr", "marches-securises.fr"),
    ("ww.marches-securises.fr", "marches-securises.fr"),
    ("wwww.marches-securises.fr", "marches-securises.fr"),
    // AWS.
    ("marche-public.info", "marches-publics.info"),
    ("marches-public.info", "marches-publics.info"),
    ("marches-publics.info.fr", "marches-publics.info"),
    ("marchespublics.info", "marches-publics.info"),
    (
        "savoie-dechets.marchespublics.info",
        "savoie-dechets.marches-publics.info",
    ),
    // Dematis, dont le trait d'union du domaine saute.
    ("e-marchepublics.com", "e-marchespublics.com"),
    (
        "beaupreau-en-mauges.e-marchespublics",
        "beaupreau-en-mauges.e-marchespublics.com",
    ),
    (
        "caprovenceverte.emarchespublics.com",
        "caprovenceverte.e-marchespublics.com",
    ),
    ("catp.emarchespublics.com", "catp.e-marchespublics.com"),
    (
        "ch-angouleme.e.marchespublics.com",
        "ch-angouleme.e-marchespublics.com",
    ),
    (
        "cmaroccitanie.emarchespublics.com",
        "cmaroccitanie.e-marchespublics.com",
    ),
    (
        "hautsdeseinehabitat.emarchespublics.com",
        "hautsdeseinehabitat.e-marchespublics.com",
    ),
    (
        "mairiecarquefou.emarchespublics.com",
        "mairiecarquefou.e-marchespublics.com",
    ),
    ("mnhn.emarchespublics.com", "mnhn.e-marchespublics.com"),
    (
        "sivubordeauxmerignac.emarchespublics.com",
        "sivubordeauxmerignac.e-marchespublics.com",
    ),
    ("syseg.emarchespublics.com", "syseg.e-marchespublics.com"),
    (
        "ville-plaisir.e-marches-publics.com",
        "ville-plaisir.e-marchespublics.com",
    ),
    // Safetender et achatpublic, sur le nom de l'instance.
    (
        "commandepublique-nca-vdn-ccas.safetender",
        "commandepublique-nca-vdn-ccas.safetender.com",
    ),
    (
        "marchespublics-ght-rouergue.safetnder.com",
        "marchespublics-ght-rouergue.safetender.com",
    ),
    (
        "vichycommunaute.satender.com",
        "vichycommunaute.safetender.com",
    ),
    (
        "vichycommunaute.sefetender.com",
        "vichycommunaute.safetender.com",
    ),
    ("elegia.achatspublic.com", "elegia.achatpublic.com"),
];

/// Hôte canonique : minuscules, sans `www.` ni point final, réécrit vers
/// l'hôte qui sert le profil quand [`ALIAS_HOTE`] en nomme un.
pub fn canon_host(h: &str) -> String {
    let h = h.trim().trim_end_matches('.').to_lowercase();
    let h = h.trim_start_matches("www.");
    ALIAS_HOTE
        .iter()
        .find(|(cite, _)| *cite == h)
        .map_or_else(|| h.to_string(), |(_, sert)| (*sert).to_string())
}

/// Classe une liste d'hôtes pondérée par le nombre d'avis BOAMP qui les
/// citent. Les hôtes sont canonisés et fusionnés avant sondage.
pub async fn recenser(poids: HashMap<String, i32>) -> Result<Vec<Profil>> {
    let mut fusion: HashMap<String, i32> = HashMap::new();
    for (h, n) in poids {
        *fusion.entry(canon_host(&h)).or_insert(0) += n;
    }
    let client = super::client_sonde();
    Ok(futures::stream::iter(fusion.into_iter().map(|(host, n)| {
        let client = &client;
        async move { classer(client, host, n).await }
    }))
    .buffer_unordered(HOTES_PARALLELES)
    .collect()
    .await)
}

/// Le résidu de couverture : hôtes qui ne portent ni profil d'acheteur ni DCE
/// (juridictions, Légifrance, europa.eu…).
static BRUIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(w3\.org|europa\.eu|boamp\.fr|legifrance|tribunal-administratif|juradm|conseil-etat|service-public|economie\.gouv\.fr|data\.gouv\.fr|inpi\.fr)",
    )
    .unwrap()
});

/// Les moteurs effectivement moissonnés — le dénominateur de la couverture.
/// Les autres (`agysoft`, `centraledesmarches`, `eurolegales`…) sont
/// identifiés mais non connectés : leurs avis comptent comme non couverts.
pub const MOTEURS_CONNECTES: [&str; 11] = [
    "atexo",
    "aws",
    "safetender",
    "dematis",
    "achatpublic",
    "atline",
    "synapse",
    "xmarches",
    "aji",
    "klekoon",
    "modula",
];

/// Résultat du recensement : les profils classés et la couverture mesurée.
#[derive(Debug)]
pub struct Recensement {
    pub profils: Vec<Profil>,
    /// Avis de marché actifs examinés.
    pub avis: usize,
    /// Avis dont au moins un hôte cité relève d'un moteur connecté — la couverture
    /// réelle du fond.
    pub avis_couverts: usize,
}

/// Recense les profils cités par les avis de marché BOAMP actifs et mesure la
/// couverture. Le comptage se fait **par avis** : un avis est couvert dès qu'une de
/// ses URLs porte un moteur connecté.
pub async fn recenser_depuis_boamp() -> Result<Recensement> {
    let par_avis = super::boamp::hotes_par_avis().await?;
    let mut poids: HashMap<String, i32> = HashMap::new();
    for hotes in &par_avis {
        for h in hotes.iter().filter(|h| !BRUIT.is_match(h)) {
            *poids.entry(h.clone()).or_insert(0) += 1;
        }
    }
    for (h, _) in HORS_BOAMP {
        poids.entry(h.to_string()).or_insert(0);
    }
    let profils = recenser(poids).await?;
    let connectes: std::collections::HashSet<&str> = profils
        .iter()
        .filter(|p| p.moteur.is_some_and(|m| MOTEURS_CONNECTES.contains(&m)))
        .map(|p| p.host.as_str())
        .collect();
    let avis_couverts = par_avis
        .iter()
        .filter(|hotes| {
            hotes
                .iter()
                .any(|h| connectes.contains(canon_host(h).as_str()))
        })
        .count();
    Ok(Recensement {
        avis: par_avis.len(),
        avis_couverts,
        profils,
    })
}

#[cfg(test)]
mod tests {
    use super::{canon_host, ALIAS_HOTE, HORS_BOAMP};

    /// L'hôte cité par l'avis n'est pas toujours celui qui sert le profil : coquille de
    /// saisie ou portail dont la MPE vit sur un sous-domaine. La canonicalisation rend
    /// l'hôte qui répond, celui que le recensement classe.
    #[test]
    fn l_hote_cite_est_reecrit_vers_celui_qui_sert_le_profil() {
        assert_eq!(
            canon_host("WWW.Marches.Securises.FR."),
            "marches-securises.fr"
        );
        assert_eq!(canon_host("place.gouv.fr"), "marches-publics.gouv.fr");
        assert_eq!(
            canon_host("megalis.bretagne.bzh"),
            "marches.megalis.bretagne.bzh"
        );
        assert_eq!(
            canon_host("www.marches-securises.fr"),
            "marches-securises.fr"
        );
    }

    /// Une entrée d'alias doit être atteignable et terminale : la clé se lit **après**
    /// minuscules et `www.` retiré, la cible n'est la clé d'aucune autre.
    #[test]
    fn chaque_alias_est_atteignable_et_terminal() {
        for (cite, sert) in ALIAS_HOTE {
            assert_eq!(canon_host(cite), sert, "alias « {cite} » inatteignable");
            assert_eq!(canon_host(sert), sert, "cible « {sert} » non terminale");
        }
    }

    /// `classer` reçoit l'hôte déjà canonisé : une entrée écrite `www.X` ou en
    /// capitales serait classée par la sonde plutôt que par son moteur déclaré.
    #[test]
    fn les_hotes_hors_boamp_sont_canoniques() {
        for (h, _) in HORS_BOAMP {
            assert_eq!(canon_host(h), h);
        }
    }
}
