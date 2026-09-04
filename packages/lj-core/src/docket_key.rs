//! Clé canonique d'un numéro de décision — pourvoi, rôle, requête.
//!
//! Un numéro réduit à ses alphanumériques minuscules. `19-20.312`, `19-20312`,
//! `1920312` et `19 20 312` donnent tous `1920312` : c'est la seule façon de
//! réunir les graphies, un tokenizer ne sachant que couper.
//!
//! Le pendant côté base est la fonction SQL `lj_docket_keys` (migration 0211),
//! sur laquelle l'index d'expression est bâti. Les deux implémentations sont
//! asservies au même oracle (`docket_keys.json`) : elles ne peuvent pas diverger
//! sans faire rougir le build.

use std::sync::LazyLock;

use regex::Regex;

/// Clé canonique d'un numéro (voir doc de module). Vide si le numéro ne porte
/// aucun alphanumérique.
pub fn docket_key(num: &str) -> String {
    num.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A decision number inside a query, every jurisdiction: `07-11.687`,
/// `19-23664`, `23/01001`, `31399/23`, `2202166`, `2022021570`, `21PA01295`,
/// `2005J00072`, `C-369/90`, `SAN-2022-019`. Sole source of what the exact key
/// leg looks up.
///
/// Bare digits run from five to ten: five carry 50,163 decisions (29/08/2026).
/// The floor held at six as long as a postcode could empty a search; it no
/// longer can, because a bare run standing alone buys weight and not a mode
/// ([`declares_reference_explicitly`]). Postcodes are there: `75012`, `75013`
/// and `33260`, three of the twelve distinct five-digit queries of real
/// traffic, against the earlier reading that found none (30/08/2026). The Strasbourg shape, digits over a two-digit year, covers
/// 45,181 of the 45,267 CEDH numbers; its floor sits at three because two over
/// two is a written date, `12/03` inside `12/03/2024`, for the 86 numbers left.
pub(crate) static QUERY_DOCKET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b\d{2}-\d{2}\.\d{3,4}\b",
        r"|\b\d{2}-\d{4,5}\b",
        r"|\b\d{2}/\d{5}\b",
        // Before the bare-digit branch, which would match `31399` alone.
        r"|\b\d{3,5}/\d{2}\b",
        r"|\b\d{2,4}[a-z]{1,3}\d{3,6}\b",
        r"|\b[ctf]-\d{1,4}/\d{2}\b",
        r"|\b[a-z]{2,4}-\d{4}-\d{3}\b",
        r"|\b\d{5,10}\b",
    ))
    .unwrap()
});

/// L'ordinal nu d'un numéro du Tribunal des conflits. Le fonds l'écrit selon
/// l'époque (`00012` avant 2000, `3220` jusqu'en 2004, `C4174` ensuite) et les
/// praticiens indifféremment : ni le `C` ni les zéros de tête ne distinguent
/// deux décisions. Le suffixe, lui, en sépare deux (`2530B` n'est pas `02530`)
/// et reste.
pub fn tc_ordinal(num: &str) -> String {
    let plie = docket_key(num);
    let nu = plie
        .strip_prefix('c')
        .unwrap_or(&plie)
        .trim_start_matches('0');
    if nu.is_empty() {
        plie
    } else {
        nu.to_string()
    }
}

/// L'unité qui suit un nombre en fait une quantité : `500000 euros`, `35 %`.
static UNITE_SUIVANTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^[\s.,]*(?:",
        r"€|\$|%|eur\b|euros?\b|francs?\b|dollars?\b|usd\b|centimes?\b",
        r"|m²|m2\b|km\b|kg\b|tonnes?\b|hectares?\b",
        r"|habitants?\b|salari[ée]s?\b|emplois?\b",
        r"|heures?\b|jours?\b|semaines?\b|mois\b|ans?\b|ann[ée]es?\b",
        r")",
    ))
    .unwrap()
});

/// Ce qui, avant un nombre, en annonce le montant quand aucune unité ne suit :
/// `condamné à payer 500000`.
static MONTANT_PRECEDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(?:",
        r"montants?|sommes?|indemnit[ée]s?|pr[ée]judices?|dommages?|int[ée]r[êe]ts?",
        r"|amendes?|astreintes?|cr[ée]ances?|dettes?|loyers?|chiffres?",
        r"|salaires?|r[ée]mun[ée]rations?|prix|valeurs?|co[ûu]ts?",
        r"|payer|verser|rembourser|allouer|[ée]valu[ée]e?s?|condamn[ée]e?s?",
        r")\b[^0-9]{0,24}$",
    ))
    .unwrap()
});

/// La marque de numérotation, qui rattrape un nombre précédé d'un mot d'argent.
///
/// Pas de `\b` de fin : `°` n'est pas un caractère de mot, donc aucune frontière
/// ne sépare `n°` de l'espace qui suit. `affaire` n'y est pas, « chiffre
/// d'affaires » le porterait.
static NUMERO_PRECEDENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:n°|n[°o]s?|num[ée]ros?|rg|pourvoi|requ[êe]te|dossier)[^0-9]{0,8}$")
        .unwrap()
});

/// Le nombre trouvé en `[debut, fin[` est-il une quantité et non un rôle ?
///
/// Trois signaux par ordre de force : l'unité qui suit tranche seule, sinon la
/// marque de numérotation rattrape, sinon le mot d'argent écarte.
///
/// Rare mais total, mesuré sur la prod le 29/08/2026 : `500000`, `200000` et
/// `120000` sont des rôles du Conseil d'État, donc `indemnité de 500000 euros`
/// prenait le poids de référence pure (8,0, au-dessus des trois autres jambes)
/// et `truncate_fused` coupait tout le reste. Deux résultats rendus au lieu de
/// dix, la décision de tête n'ayant que son numéro en commun avec la demande.
fn est_une_quantite(query: &str, debut: usize, fin: usize) -> bool {
    if UNITE_SUIVANTE_RE.is_match(&query[fin..]) {
        return true;
    }
    let avant = &query[..debut];
    !NUMERO_PRECEDENT_RE.is_match(avant) && MONTANT_PRECEDENT_RE.is_match(avant)
}

/// Clés canoniques des numéros portés par une requête, dédupliquées, dans
/// l'ordre d'apparition. Les quantités sont écartées ([`est_une_quantite`]).
pub fn query_docket_keys(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in QUERY_DOCKET_RE.find_iter(query) {
        if m.as_str().bytes().all(|b| b.is_ascii_digit())
            && est_une_quantite(query, m.start(), m.end())
        {
            continue;
        }
        let key = docket_key(m.as_str());
        if !key.is_empty() && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Does the query designate its number AS a number? The shape (`19-20.682`,
/// `C-369/90`) can be nothing else; quotes and a numbering mark state the intent
/// on a bare run, which otherwise shares its form with an amount, a postcode or
/// a year. Measured 29/08/2026 over 675 distinct external queries carrying a bare
/// run: 594 declare, 81 stick the digits to a name and a subject (`387763 Czabaj
/// délai raisonnable`), where the number is a clue and not the request.
pub fn declares_reference_explicitly(query: &str) -> bool {
    QUERY_DOCKET_RE
        .find_iter(query)
        .any(|m| marque_explicite(query, &m))
}

/// The number is the whole query. Too weak to pick a search mode on its own:
/// `75013`, `75012` and `33260` are postcodes, three of the twelve distinct
/// five-digit queries of real traffic (30/08/2026).
pub fn is_bare_number(query: &str) -> bool {
    let seule = query.trim();
    QUERY_DOCKET_RE
        .find_iter(query)
        .any(|m| m.as_str() == seule)
}

fn marque_explicite(query: &str, m: &regex::Match) -> bool {
    m.as_str().bytes().any(|b| !b.is_ascii_digit())
        || (query[..m.start()].ends_with('"') && query[m.end()..].starts_with('"'))
        || NUMERO_PRECEDENT_RE.is_match(&query[..m.start()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_trois_epoques_du_tribunal_des_conflits_donnent_le_meme_ordinal() {
        for n in ["00012", "12", "C12", "c 12", "0012"] {
            assert_eq!(tc_ordinal(n), "12", "numéro {n:?}");
        }
        assert_eq!(tc_ordinal("C4174"), "4174");
        assert_eq!(tc_ordinal("3220"), "3220");
    }

    #[test]
    fn le_suffixe_separe_deux_decisions_du_tribunal_des_conflits() {
        assert_ne!(tc_ordinal("2530B"), tc_ordinal("02530"));
        assert_eq!(tc_ordinal("2530B"), "2530b");
    }

    #[test]
    fn les_graphies_dun_numero_tombent_sur_la_meme_cle() {
        for n in ["19-20.312", "19-20312", "1920312", "19 20 312", "19/20.312"] {
            assert_eq!(docket_key(n), "1920312", "graphie {n:?}");
        }
        for n in ["22/01772", "22-01772", "2201772", "22 01772"] {
            assert_eq!(docket_key(n), "2201772", "graphie {n:?}");
        }
    }

    #[test]
    fn la_cle_garde_les_lettres_et_est_un_point_fixe() {
        assert_eq!(docket_key("C-806/18"), "c80618");
        assert_eq!(docket_key("c80618"), "c80618");
        assert_eq!(docket_key(&docket_key("19-20.312")), "1920312");
        assert_eq!(docket_key("---"), "");
    }

    #[test]
    fn extraction_depuis_la_requete() {
        assert_eq!(query_docket_keys("pourvoi 19-20.682"), ["1920682"]);
        assert_eq!(query_docket_keys("Douai 22/01772 sociale"), ["2201772"]);
        assert_eq!(query_docket_keys("CE 427345"), ["427345"]);
        assert!(query_docket_keys("congés payés").is_empty());
    }

    /// Le rôle des tribunaux de commerce et des activités économiques compte dix
    /// chiffres. Un plafond à huit le laissait hors de la jambe clé exacte : la
    /// requête tombait sur BM25 seul, donc nommer la juridiction en plus du
    /// numéro faisait descendre la cible au lieu de la remonter.
    #[test]
    fn le_role_a_dix_chiffres_des_tribunaux_de_commerce() {
        assert_eq!(query_docket_keys("2022021570"), ["2022021570"]);
        assert_eq!(
            query_docket_keys("TAE Paris RG n° 2022021570"),
            ["2022021570"]
        );
        assert_eq!(
            query_docket_keys("tribunal de commerce de Lille 2026015202"),
            ["2026015202"]
        );
    }

    /// Requêtes du trafic réel : le numéro est pris quel que soit ce qui
    /// l'entoure, prose comprise, et un mot d'argent qui suit ne le retire pas.
    #[test]
    fn les_numeros_du_trafic_reel() {
        assert_eq!(
            query_docket_keys("2302081 Montreuil marocain passeport talent carte bleue européenne"),
            ["2302081"]
        );
        assert_eq!(
            query_docket_keys(
                "\"402006\" Croë Suisse cession à un prix significativement inférieur à la valeur vénale"
            ),
            ["402006"]
        );
        assert_eq!(
            query_docket_keys("\"412010\" prescription quadriennale créance indemnitaire"),
            ["412010"]
        );
        assert_eq!(
            query_docket_keys("requête n° 36760/06 Stanev c. Bulgarie"),
            ["3676006"]
        );
        assert_eq!(query_docket_keys("CEDH 31399/23"), ["3139923"]);
        assert!(query_docket_keys("décision du 12/03/2024 sur le permis").is_empty());
    }

    /// Les trois montants qui coûtaient la recherche entière : rôles du Conseil
    /// d'État, donc `indemnité de 500000 euros` ne rendait que deux résultats.
    #[test]
    fn un_montant_ne_prend_pas_le_role_qui_porte_le_meme_nombre() {
        for q in [
            "indemnité de 500000 euros",
            "préjudice de 200000 euros",
            "astreinte de 120000 euros",
            "condamné à payer 500000",
            "chiffre d'affaires de 850000 €",
            "commune de 120000 habitants",
        ] {
            assert!(query_docket_keys(q).is_empty(), "requête {q:?}");
        }
    }

    /// La marque de numérotation rattrape ce que le mot d'argent écarterait, et
    /// un rôle structuré n'est jamais concerné.
    #[test]
    fn le_garde_fou_epargne_les_vrais_roles() {
        assert_eq!(query_docket_keys("dommages n° 250000"), ["250000"]);
        assert_eq!(
            query_docket_keys("indemnité de 150000 euros pourvoi 19-20.682"),
            ["1920682"]
        );
        assert_eq!(
            query_docket_keys(
                "\"402006\" Croë Suisse cession à un prix inférieur à la valeur vénale"
            ),
            ["402006"]
        );
    }

    /// Les deux bornes, chacune pour ce qu'elle écarte. Cinq chiffres entrent :
    /// `02589`, `11552`, `15293`, `16496` et `17270` sont tapés par de vrais
    /// visiteurs et portés par le fonds. Un code postal entre aussi, et c'est
    /// tenable parce qu'il ne déclare rien, donc il s'ajoute au lieu de
    /// remplacer ; quatre chiffres restent dehors, l'année y tomberait.
    #[test]
    fn les_bornes_des_chiffres_nus() {
        assert_eq!(query_docket_keys("11552"), ["11552"]);
        assert_eq!(
            query_docket_keys("bail commercial 75001 Paris"),
            ["75001"],
            "le code postal entre en clé"
        );
        assert!(
            !declares_reference_explicitly("bail commercial 75001 Paris")
                && !is_bare_number("bail commercial 75001 Paris"),
            "mais il ne fait pas loi"
        );
        assert!(query_docket_keys("arrêt de 1965").is_empty());
        assert!(query_docket_keys("12345678901").is_empty());
    }

    #[test]
    fn extraction_des_graphies_a_lettres() {
        assert_eq!(query_docket_keys("CAA Paris 21PA01295"), ["21pa01295"]);
        assert_eq!(query_docket_keys("caa douai n° 22da00744"), ["22da00744"]);
        assert_eq!(query_docket_keys("99PA2653"), ["99pa2653"]);
        assert_eq!(query_docket_keys("TCOM Paris 2005J00072"), ["2005j00072"]);
        assert_eq!(query_docket_keys("CJUE C-369/90 Micheletti"), ["c36990"]);
        assert_eq!(query_docket_keys("T-99/98"), ["t9998"]);
        assert_eq!(
            query_docket_keys("délibération SAN-2022-019"),
            ["san2022019"]
        );
    }

    /// Ce qu'une requête juridique écrit couramment et qui n'est pas un numéro de
    /// décision : la clé exacte rendrait une jambe vide, et sur une référence pure
    /// un verdict d'absence qui serait faux.
    #[test]
    fn ni_un_article_ni_un_texte_ne_sont_un_numero() {
        for q in [
            "article L761-1 du code de justice administrative",
            "L. 531-1",
            "décret 2020-1310",
            "loi 2016-1547 de modernisation de la justice",
            "2024-02-13",
            "arrêt du 13 février 2024",
        ] {
            assert!(query_docket_keys(q).is_empty(), "requête {q:?}");
        }
    }

    #[test]
    fn deux_graphies_du_meme_numero_dans_une_requete_donnent_une_cle() {
        assert_eq!(query_docket_keys("19-20312 ou 1920312"), ["1920312"]);
    }

    /// Les quatre façons de désigner un numéro comme un numéro, contre le chiffre
    /// nu jeté dans la prose. Requêtes tirées du trafic du 29/08/2026.
    #[test]
    fn la_requete_dit_ou_ne_dit_pas_que_le_nombre_est_un_numero() {
        for q in [
            "19-20.682",
            "Douai 22/01772 chambre sociale",
            "CJUE, 7 juillet 1992, C-369/90",
            "\"100888\"",
            "\"100489\" OU \"101053\"",
            "TA Montreuil, 4 décembre 2024, n° 2405440",
        ] {
            assert!(declares_reference_explicitly(q), "numéro déclaré {q:?}");
        }
        assert!(is_bare_number("1702092"));
        assert!(is_bare_number(" 75013 "));
        assert!(!declares_reference_explicitly("1702092"));
        for q in [
            "402006 Croë Suisse",
            "décision 387763 Czabaj délai raisonnable",
            "barème 500000 accident de la circulation",
            "licenciement sans cause réelle",
        ] {
            assert!(
                !declares_reference_explicitly(q) && !is_bare_number(q),
                "chiffre nu ou prose {q:?}"
            );
        }
    }

    #[derive(serde::Deserialize)]
    struct KeyCase {
        num: String,
        key: String,
    }

    #[test]
    fn parite_avec_la_fonction_sql_lj_docket_keys() {
        let raw = include_str!("../tests/fixtures/oracle/docket_keys.json");
        let cases: Vec<KeyCase> = serde_json::from_str(raw).expect("fixture docket_keys");
        for c in &cases {
            assert_eq!(docket_key(&c.num), c.key, "num {:?}", c.num);
        }
    }
}
