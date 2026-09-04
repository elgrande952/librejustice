//! Clé publique d'un numéro d'article — segment d'URL `/texte/{code}/{clé}` et
//! alphabet des colonnes `num_key`.
//!
//! Alphabet : minuscules ASCII, chiffres, `-`, plus `()`. Le préfixe de partie
//! (`L`, `R`, `D`, `A`, `LO`, `LP`) est collé au numéro : « L. 761-1 » →
//! `l761-1`. Tout séparateur se plie en `-` : « 106 Bis » → `106-bis`.
//! L'astérisque de Légifrance, marqueur et non séparateur, s'efface sans rien
//! laisser : « R*600-1 » → `r600-1` comme « R. 600-1 ». Le libellé autoritaire
//! qui la garde est `legal_article.num`.
//!
//! La fonction est un point fixe (`article_key(article_key(x)) ==
//! article_key(x)`) : les backfills se rejouent, et toute forme citée d'un
//! même numéro (« L761-1 », « l. 761-1 », « L 761-1 ») tombe sur la même clé.

/// Préfixes de partie collés au numéro (minuscules, après pliage).
const GLUED_PREFIXES: [&str; 6] = ["l", "r", "d", "a", "lo", "lp"];

/// Clé publique d'un numéro d'article (voir doc de module).
pub fn article_key(num: &str) -> String {
    // Pliage : minuscules, accents ASCII, alphabet restreint, séparateurs en `-`.
    let mut folded = String::with_capacity(num.len());
    for c in num.to_lowercase().chars().filter(|c| *c != '*') {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'î' | 'ï' => 'i',
            'ó' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'œ' => {
                folded.push('o');
                'e'
            }
            c => c,
        };
        if c.is_ascii_alphanumeric() || matches!(c, '(' | ')') {
            folded.push(c);
        } else if !folded.is_empty() && !folded.ends_with('-') {
            folded.push('-');
        }
    }
    let folded = folded.trim_end_matches('-');

    // Collage du préfixe de partie : `l-761-1` → `l761-1`.
    for prefix in GLUED_PREFIXES {
        if let Some(tail) = folded.strip_prefix(prefix) {
            if let Some(tail) = tail.strip_prefix('-') {
                if tail.starts_with(|c: char| c.is_ascii_digit()) {
                    return format!("{prefix}{tail}");
                }
            }
        }
    }
    folded.to_string()
}

/// Clé d'IDENTITÉ d'un numéro d'article servi intégral — la valeur de
/// `legal_article.num_key` et du segment d'URL `/texte/{code}/{clé}`.
///
/// Distincte de la clé de CITATION (`normalize_article` de `lj-extract`, pliée par
/// [`article_key`]) : celle-ci tronque volontairement au cœur de l'identifiant
/// (ordinaux hors vocabulaire, discriminants « -0 W », numéros pointés « 1.01 »),
/// ce qui apparie des citations bruitées mais confond des articles distincts.
/// L'identité, elle, garde le numéro DILA entier à deux normalisations près :
/// le marqueur ordinal de tête (« 1er » ≡ « 1 », « 1er bis » ≡ « 1 bis »,
/// « premier » ≡ « 1 ») et l'astérisque, tous deux marqueurs et non
/// discriminants. Point fixe comme `article_key`.
pub fn identity_key(num: &str) -> String {
    let t = num.trim();
    if t.eq_ignore_ascii_case("premier") {
        return "1".to_string();
    }
    let nd = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if nd > 0 {
        let rest = &t[nd..];
        let low = rest.to_lowercase();
        for marker in ["ère", "ere", "er"] {
            if low.starts_with(marker)
                && low[marker.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                return article_key(&format!("{}{}", &t[..nd], &rest[marker.len()..]));
            }
        }
    }
    article_key(t)
}

/// Clé d'identité d'un article de **recueil** : le numéro plié, qualifié par la
/// pièce qui le numérote. `("KALITEXT000005675711", "2")` → `kalitext000005675711-2`.
///
/// Un recueil rassemble des pièces qui renumérotent chacune à partir de 1, et le
/// numéro seul n'y identifie rien. Les articles de la pièce de tête gardent leur
/// clé nue : c'est eux que désigne un numéro cité sans autre précision. Point
/// fixe comme [`article_key`], dont l'alphabet absorbe l'identifiant de pièce.
pub fn piece_key(piece_uid: &str, num_key: &str) -> String {
    format!("{}-{}", article_key(piece_uid), num_key)
}

/// Même clé, séparateur réinséré à la frontière chiffre/lettre finale : `l80a`
/// → `l80-a`, `119bis` → `119-bis`. `None` hors de cette forme, clé déjà
/// séparée comprise (`l80-a`, `l312-12`, `lo6213-1`).
///
/// Rattrape une lecture d'article après échec de la clé exacte, le fonds
/// portant `l80-a` là où l'appelant écrit `l80a`. La borne est étroite par
/// nécessité : ignorer les séparateurs confondrait `l312-12` et `l312-1-2`,
/// articles distincts, là où la frontière chiffre/lettre ne heurte que 4 des
/// 17 216 clés à suffixe de lettres du fonds.
pub fn letter_suffix_separated(key: &str) -> Option<String> {
    let b = key.as_bytes();
    let mut cut = b.len();
    while cut > 0 && b[cut - 1].is_ascii_lowercase() {
        cut -= 1;
    }
    if cut == b.len() || cut == 0 || !b[cut - 1].is_ascii_digit() {
        return None;
    }
    Some(format!("{}-{}", &key[..cut], &key[cut..]))
}

/// The key left when a written number opens with the word it names: « Article
/// 1240 » folds to `article-1240`, and the article is `1240`. Tried only after
/// the literal key, 116 keys of the corpus really being `article-N`, and only
/// before a digit, `art-premier` naming no article of the fonds.
pub fn without_spoken_word(key: &str) -> Option<&str> {
    ["article-", "art-"]
        .iter()
        .find_map(|word| key.strip_prefix(word))
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Forme d'affichage d'une clé publique à préfixe de partie simple :
/// `l761-1` → « L. 761-1 », `lo6213-1` → « LO. 6213-1 ». Hors de ce
/// sous-ensemble (numérique nu, annexes…), la clé se rend telle quelle. Le
/// libellé autoritaire reste `legal_article.num` quand on l'a : lui seul porte
/// l'astérisque de Légifrance.
pub fn display(key: &str) -> String {
    for prefix in ["lo", "lp", "l", "r", "d", "a"] {
        if let Some(tail) = key.strip_prefix(prefix) {
            if tail.starts_with(|c: char| c.is_ascii_digit()) {
                return format!("{}. {tail}", prefix.to_uppercase());
            }
        }
    }
    key.to_string()
}

/// Numéros d'article **cités dans une requête de recherche**, rendus en clés
/// [`article_key`] : « L. 423-7 ceseda parent » → `["l423-7"]`.
///
/// L'index stocke le numéro en **un seul token** (`L423-7` → `l423-7`), alors
/// que « L. 423-7 » se coupe en `l` (stopword) + `423-7`, qui ne matche pas.
///
/// Cinq formes reconnues :
///
/// - préfixe de partie **collé** au numéro — `L423-7`, `R*212-4` ;
/// - préfixe **détaché** — `L. 423-7`, `L 423-7`, `l.423-7` ;
/// - numéro **nu à segments** — `706-88`, `131-30-1` ;
/// - nombre **nu annoncé par « article »** — « article 21 loi 65-557 » cite
///   l'article 21 ; « 21 jours », lui, ne cite rien ;
/// - numéro **pointé annoncé** — « article 8.1 » cite `8-1`. Le point seul ne
///   suffit pas à annoncer une citation, contrairement au trait d'union : sans
///   le mot « article », `1.2` est un décimal bien plus souvent qu'un numéro.
///
/// Un préfixe détaché suivi d'un nombre **sans trait d'union** ne compte que
/// s'il est écrit en capitale : « L 21 » cite, « à 21 ans » ne cite pas (`à` se
/// plie en `a`, qui est un préfixe de partie).
///
/// L'**ordinal** en fait partie : « article 7 bis » cite `7-bis`, pas `7` — deux
/// articles distincts, et 23 195 du corpus portent un tel suffixe. Ordre
/// d'apparition conservé, doublons retirés.
pub fn query_article_keys(query: &str) -> Vec<String> {
    let tokens = cited_tokens(query);
    let mut keys: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        let digits = |t: &str| t.starts_with(|c: char| c.is_ascii_digit());
        let announced = i > 0
            && matches!(
                tokens[i - 1].to_lowercase().as_str(),
                "article" | "articles" | "art"
            );
        let key = match part_prefix_len(tok) {
            // Préfixe collé : `L423-7`, `R*212-4`.
            Some(n) if digits(&tok[n..]) => Some(article_key(tok)),
            // Préfixe détaché du numéro qui suit : `L. 423-7`, `l.423-7`.
            Some(n) if n == tok.len() => match tokens.get(i + 1) {
                Some(next)
                    if digits(next)
                        && (next.contains('-') || tok.chars().any(|c| c.is_uppercase())) =>
                {
                    i += 1;
                    Some(article_key(&format!("{tok}{next}")))
                }
                _ => None,
            },
            _ if digits(tok) && (tok.contains('-') || announced) => Some(article_key(tok)),
            _ => None,
        };
        if let Some(mut key) = key {
            if let Some(ord) = tokens.get(i + 1).filter(|t| is_ordinal(t)) {
                key = article_key(&format!("{key} {ord}"));
                i += 1;
            }
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        i += 1;
    }
    keys
}

/// Découpage d'une requête en tokens candidats à la citation : alphanumériques,
/// `-` et `*` de l'alphabet des clés, plus un `.` ou un `/` **encadré de
/// chiffres**, qui reste dans le token.
///
/// Le point ne peut pas être gardé sans condition : « art. 1240 » donnerait le
/// token `art.`, et la détection de l'article annoncé, qui compare à `art`,
/// tomberait. Encadré de chiffres, il ne peut être qu'un séparateur de numéro —
/// 45 274 articles du corpus en portent un, surtout des conventions collectives
/// (`8.1`) et des codes étrangers (`286.2`).
fn cited_tokens(query: &str) -> Vec<&str> {
    let bytes = query.as_bytes();
    let kept = |c: char| c.is_alphanumeric() || c == '-' || c == '*';
    let framed = |i: usize| {
        i > 0 && bytes[i - 1].is_ascii_digit() && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
    };
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in query.char_indices() {
        if kept(c) || (matches!(c, '.' | '/') && framed(i)) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            tokens.push(&query[s..i]);
        }
    }
    if let Some(s) = start {
        tokens.push(&query[s..]);
    }
    tokens
}

/// Ordinaux latins qui suffixent un numéro d'article, dans l'ordre où le
/// législateur les emploie.
const ORDINALS: [&str; 9] = [
    "bis",
    "ter",
    "quater",
    "quinquies",
    "sexies",
    "septies",
    "octies",
    "nonies",
    "decies",
];

fn is_ordinal(token: &str) -> bool {
    ORDINALS.contains(&token.to_lowercase().as_str())
}

/// Forme d'une clé où l'ordinal final est **détaché** : `7-bis` → « 7 bis ».
/// `None` si la clé n'en porte pas.
///
/// Le corpus écrit l'ordinal séparé — les 23 195 numéros concernés sont stockés
/// « 7 bis », jamais « 7-bis ». Un index qui garde le trait d'union dans le
/// token ne rapproche donc pas les deux : chercher la clé telle quelle ne
/// trouve rien, il faut la forme du corpus.
pub fn spaced_ordinal(key: &str) -> Option<String> {
    let (head, tail) = key.rsplit_once('-')?;
    is_ordinal(tail).then(|| format!("{head} {tail}"))
}

/// Forme d'une clé **tout en chiffres** où le séparateur est rendu au point :
/// `8-1` → « 8.1 ». `None` si la clé porte une lettre ou pas de séparateur.
///
/// Même raison que [`spaced_ordinal`] : la clé plie tout séparateur en `-`, le
/// corpus écrit `8.1`, et l'index tient ce numéro en un token — chercher `8-1`
/// n'y tombe pas. Depuis la clé on ne peut pas savoir laquelle des deux
/// graphies le corpus porte, donc on cherche les deux ; c'est une alternative de
/// plus dans un `should`, pas un filtre.
pub fn dotted_form(key: &str) -> Option<String> {
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = key.split('-').collect();
    (parts.len() > 1 && parts.iter().all(|p| numeric(p))).then(|| parts.join("."))
}

/// Longueur du préfixe de partie en tête d'un token — `[*]*(l|lo|lp|r|d|a)[*]*`,
/// casse indifférente. `None` si le token n'en porte pas.
fn part_prefix_len(token: &str) -> Option<usize> {
    let low = token.to_lowercase();
    let stars = low.chars().take_while(|c| *c == '*').count();
    GLUED_PREFIXES
        .iter()
        .copied()
        .filter(|p| low[stars..].starts_with(p))
        .map(|p| {
            let end = stars + p.len();
            end + low[end..].chars().take_while(|c| *c == '*').count()
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model writes the word, the corpus sometimes carries it: the fallback
    /// only opens on a digit, and gives back nothing else.
    #[test]
    fn le_mot_article_tombe_devant_un_numero() {
        assert_eq!(without_spoken_word("article-1240"), Some("1240"));
        assert_eq!(without_spoken_word("art-l761-1"), None);
        assert_eq!(without_spoken_word("art-706-88"), Some("706-88"));
        assert_eq!(without_spoken_word("1240"), None);
        assert_eq!(without_spoken_word("article-annexe"), None);
        assert_eq!(article_key("Article 1240"), "article-1240");
    }

    #[test]
    fn display_inverse_les_prefixes_simples() {
        assert_eq!(display("l761-1"), "L. 761-1");
        assert_eq!(display("lo6213-1"), "LO. 6213-1");
        assert_eq!(display("r*212-4"), "r*212-4");
        assert_eq!(display("1240"), "1240");
        assert_eq!(display("106-bis"), "106-bis");
        assert_eq!(display("annexe-1"), "annexe-1");
    }

    #[test]
    fn prefixe_de_partie_colle_au_numero() {
        assert_eq!(article_key("L. 761-1"), "l761-1");
        assert_eq!(article_key("L761-1"), "l761-1");
        assert_eq!(article_key("l. 761-1"), "l761-1");
        assert_eq!(article_key("L 761-1"), "l761-1");
        assert_eq!(article_key("R. 313-4"), "r313-4");
        assert_eq!(article_key("LO. 6213-1"), "lo6213-1");
        assert_eq!(article_key("1240"), "1240");
    }

    #[test]
    fn parentheses_conservees() {
        assert_eq!(article_key("(1)"), "(1)");
    }

    /// L'étoile de Légifrance sort de la clé, où qu'elle soit et quel qu'en
    /// soit le nombre : elle ne distingue jamais deux articles applicables le
    /// même jour, et la forme citée ne la porte pas.
    #[test]
    fn l_etoile_tombe_et_la_forme_citee_rejoint_la_forme_officielle() {
        assert_eq!(article_key("R*212-4"), "r212-4");
        assert_eq!(article_key("R. 212-4"), "r212-4");
        assert_eq!(article_key("R**119-5"), "r119-5");
        assert_eq!(article_key("**R. 111-1"), "r111-1");
        assert_eq!(article_key("*R441-22"), "r441-22");
        assert_eq!(article_key("ANNEXE*"), "annexe");
        assert_eq!(article_key("10-**perime**"), "10-perime");
        // Effacée, pas pliée en séparateur : au milieu d'une clé, le pliage
        // laisserait un tiret que personne ne cite.
        assert_eq!(
            article_key("Annexe à l'article R*351-1 art. 1"),
            "annexe-a-l-article-r351-1-art-1"
        );
    }

    #[test]
    fn separateurs_plies_en_tiret() {
        assert_eq!(article_key("106 Bis"), "106-bis");
        assert_eq!(article_key("11 BIS"), "11-bis");
        assert_eq!(article_key("100/1"), "100-1");
        assert_eq!(article_key("Annexe 1"), "annexe-1");
        assert_eq!(article_key("annexe I"), "annexe-i");
        assert_eq!(article_key("ANNEXE, 2"), "annexe-2");
        assert_eq!(article_key("Préambule."), "preambule");
    }

    #[test]
    fn le_suffixe_de_lettres_colle_se_resepare() {
        assert_eq!(letter_suffix_separated("l80a").as_deref(), Some("l80-a"));
        assert_eq!(letter_suffix_separated("l64a").as_deref(), Some("l64-a"));
        assert_eq!(
            letter_suffix_separated("119bis").as_deref(),
            Some("119-bis")
        );
        assert_eq!(letter_suffix_separated("7ter").as_deref(), Some("7-ter"));
    }

    #[test]
    fn la_reseparation_ne_touche_pas_la_frontiere_chiffre_chiffre() {
        // `l312-12` et `l312-1-2` sont deux articles : la règle large les confondrait.
        assert_eq!(letter_suffix_separated("l312-12"), None);
        assert_eq!(letter_suffix_separated("l761-1"), None);
        assert_eq!(letter_suffix_separated("lo6213-1"), None);
    }

    #[test]
    fn la_reseparation_est_sans_effet_sur_une_cle_deja_separee() {
        assert_eq!(letter_suffix_separated("l80-a"), None);
        assert_eq!(letter_suffix_separated("119-bis"), None);
        assert_eq!(letter_suffix_separated("annexe"), None);
        assert_eq!(letter_suffix_separated(""), None);
    }

    #[test]
    fn identity_preserve_les_discriminants_que_la_citation_tronque() {
        assert_eq!(identity_key("199 duovicies"), "199-duovicies");
        assert_eq!(identity_key("1609 quatertricies"), "1609-quatertricies");
        assert_eq!(identity_key("46 quater-0 W"), "46-quater-0-w");
        assert_eq!(identity_key("41 duovicies-0 H bis"), "41-duovicies-0-h-bis");
        assert_eq!(identity_key("1.01"), "1-01");
        assert_eq!(identity_key("213-6.02 bis"), "213-6-02-bis");
        assert_eq!(identity_key("221-II-1/13"), "221-ii-1-13");
        assert_eq!(identity_key("A80 CB-3-1"), "a80-cb-3-1");
    }

    #[test]
    fn identity_plie_le_marqueur_ordinal_de_tete() {
        assert_eq!(identity_key("1er"), "1");
        assert_eq!(identity_key("1ER"), "1");
        assert_eq!(identity_key("1ère"), "1");
        assert_eq!(identity_key("premier"), "1");
        assert_eq!(identity_key("PREMIER"), "1");
        assert_eq!(identity_key("1er bis"), "1-bis");
        assert_eq!(identity_key("1er-1"), "1-1");
        assert_eq!(identity_key("1er (1)"), "1-(1)");
        // Lettre de division ≠ marqueur ordinal.
        assert_eq!(identity_key("124 E"), "124-e");
        assert_eq!(identity_key("2e"), "2e");
        // Marqueur suivi d'alphanumérique = pas un marqueur.
        assert_eq!(identity_key("1ers"), "1ers");
    }

    #[test]
    fn la_piece_qualifie_la_cle_sans_la_deformer() {
        assert_eq!(
            piece_key("KALITEXT000005675711", &identity_key("2")),
            "kalitext000005675711-2"
        );
        assert_eq!(
            piece_key("KALITEXT000005675425", &identity_key("1er bis")),
            "kalitext000005675425-1-bis"
        );
        // Point fixe : un rebasage se rejoue sans redéformer la clé.
        for piece in ["KALITEXT000005675711", "KALITEXT000024359407"] {
            for num in ["2", "L. 761-1", "Annexe 1", "8.1"] {
                let key = piece_key(piece, &identity_key(num));
                assert_eq!(article_key(&key), key, "piece={piece} num={num}");
            }
        }
        // Deux pièces du même conteneur ne se confondent plus sur le même numéro.
        assert_ne!(
            piece_key("KALITEXT000005675711", "2"),
            piece_key("KALITEXT000005675733", "2")
        );
    }

    #[test]
    fn identity_point_fixe() {
        for raw in [
            "199 duovicies",
            "46 quater-0 W",
            "1.01",
            "1er bis",
            "premier",
            "L. 761-1",
            "Annexe 1",
        ] {
            let key = identity_key(raw);
            assert_eq!(identity_key(&key), key, "raw={raw}");
        }
    }

    #[test]
    fn point_fixe() {
        for raw in ["L. 761-1", "R*212-4", "106 Bis", "(1)", "Annexe 1"] {
            let key = article_key(raw);
            assert_eq!(article_key(&key), key);
        }
    }

    #[test]
    fn requete_les_formes_citees_tombent_sur_la_cle() {
        for q in ["L423-7 ceseda", "L. 423-7 ceseda", "l.423-7 ceseda"] {
            assert_eq!(query_article_keys(q), ["l423-7"]);
        }
        assert_eq!(
            query_article_keys("\"L. 433-3-1\" résidence habituelle définition"),
            ["l433-3-1"]
        );
        // Étoile tapée ou non, la requête tombe sur la même clé qu'en base.
        for q in [
            "R*212-4 permis de construire",
            "R. 212-4 permis de construire",
        ] {
            assert_eq!(query_article_keys(q), ["r212-4"]);
        }
        assert_eq!(
            query_article_keys("LO 6213-1 Saint-Barthélemy"),
            ["lo6213-1"]
        );
        assert_eq!(query_article_keys("garde à vue 706-88 du CPP"), ["706-88"]);
    }

    #[test]
    fn requete_le_numero_pointe_annonce_tombe_sur_la_cle() {
        // Convention collective, code étranger : le point sépare le numéro.
        assert_eq!(
            query_article_keys("article 8.1 convention collective assainissement"),
            ["8-1"]
        );
        assert_eq!(
            query_article_keys("art 286.2 code procédure civile"),
            ["286-2"]
        );
        // Le point détaché du préfixe garde son chemin : ni `art.` ni `l.` ne
        // sont encadrés de chiffres.
        assert_eq!(query_article_keys("art. 1240 code civil"), ["1240"]);
        assert_eq!(query_article_keys("l.423-7 ceseda"), ["l423-7"]);
        // Sans annonce, un décimal reste un décimal.
        assert!(query_article_keys("taux de 1.2 % sur douze mois").is_empty());
        // Un numéro d'instrument n'est pas un numéro d'article.
        assert!(query_article_keys("directive 2008/115 retour").is_empty());
    }

    #[test]
    fn la_graphie_pointee_du_corpus_se_reconstruit_depuis_la_cle() {
        assert_eq!(dotted_form("8-1").as_deref(), Some("8.1"));
        assert_eq!(dotted_form("286-2").as_deref(), Some("286.2"));
        assert_eq!(dotted_form("3-181").as_deref(), Some("3.181"));
        // Une lettre ou pas de séparateur : rien à reconstruire.
        assert_eq!(dotted_form("l423-7"), None);
        assert_eq!(dotted_form("7-bis"), None);
        assert_eq!(dotted_form("1240"), None);
    }

    #[test]
    fn requete_un_nombre_nu_ne_cite_que_derriere_article() {
        assert_eq!(
            query_article_keys("article 21 loi 65-557 délai convocation 21 jours copropriété"),
            ["21", "65-557"]
        );
        assert!(query_article_keys("délai de 21 jours pour convoquer").is_empty());
        // « à » se plie en « a », préfixe de partie — sans capitale, ça ne cite rien.
        assert!(query_article_keys("retraite a 62 ans").is_empty());
        assert!(query_article_keys("le contrat de travail").is_empty());
    }

    #[test]
    fn requete_l_ordinal_appartient_au_numero() {
        // Requête réelle du 11/08/2026 : « 7 » et « 7 bis » sont deux articles.
        assert_eq!(
            query_article_keys("article 7 bis accord franco-algérien 1968 salarié"),
            ["7-bis"]
        );
        assert_eq!(query_article_keys("L. 123-4 ter du code"), ["l123-4-ter"]);
        assert_eq!(query_article_keys("article 40 quinquies"), ["40-quinquies"]);
        // Un mot qui n'est pas un ordinal ne s'agrège pas.
        assert_eq!(query_article_keys("article 7 accord franco"), ["7"]);
    }

    #[test]
    fn ordinal_detache_pour_la_forme_du_corpus() {
        assert_eq!(spaced_ordinal("7-bis").as_deref(), Some("7 bis"));
        assert_eq!(spaced_ordinal("l123-4-ter").as_deref(), Some("l123-4 ter"));
        assert_eq!(spaced_ordinal("l423-7"), None);
        assert_eq!(spaced_ordinal("706-88"), None);
    }
}
