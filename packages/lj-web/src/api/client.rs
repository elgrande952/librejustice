//! Client typé contre `lj-dtos`, à surface unique (`ApiClient::from_context()` +
//! ~18 méthodes) et transport aiguillé par cible :
//!
//! - **SSR** (`ssr`) : appel **in-process** à la couche service `lj-api`
//!   (`search::search`, `decisions::get_decision`/`similar_decisions`) via
//!   l'[`AppState`] fourni au contexte Leptos par `lj-server`. Aucun
//!   HTTP-vers-soi sur le chemin chaud du rendu.
//! - **Hydrate** (`hydrate`, wasm) : `reqwest` (backend `fetch` du navigateur)
//!   vers `/internal` same-origin. Aucun en-tete d'auth a poser : le cookie de session
//!   part avec la requete, `fetch` etant en `credentials: "same-origin"`.
//!
//! Les composants ignorent le transport : ils appellent les mêmes méthodes sur
//! le `ApiClient` obtenu via [`ApiClient::from_context`].

use lj_dtos::{
    AnnonceCatalogueResponse, AnnonceDetailResponse, AnnonceSearchRequest, AnnonceSearchResponse,
    AnnuaireStatsResponse, ArticleSearchResponse, CitingDecisionHit, CoCitedArticle,
    CodeCatalogueResponse, CodeTocResponse, CorpusStatsResponse, DecisionDetail,
    DecisionPartiesResponse, DecisionViewsResponse, EntityDecisionsResponse,
    EntityDirectoryResponse, EntityPageResponse, EntityRegistreResponse, EntitySearchResponse,
    JurisdictionCatalogueResponse, JurisdictionHubResponse, JurisdictionYearResponse,
    LawArticleResponse, LawCodeSummary, LawCompareResponse, LawSectionResponse,
    NormCatalogueResponse, NormFondResponse, NormYearResponse, SearchContext,
    SearchHistoryResponse, SearchRequest, SearchResponse, SimilarDecisionsResponse,
    SuggestResponse,
};
// Types portés uniquement par les routes client (`/me`, signets, hover cards).
#[cfg(feature = "hydrate")]
use lj_dtos::{AccountSummary, BookmarksResponse, DecisionPreview, UserProfile};

/// Fenetre de pagination offset-based des listes « Mon activite ».
#[derive(Debug, Clone, Copy)]
pub struct PageParams {
    pub limit: u32,
    pub offset: u32,
}

/// Filtres de la recherche d'articles (page `/textes`). `code` borne à un
/// texte ; `jurisdiction`/`nature`/`source` filtrent le corpus. Chaque borne
/// `None` (ou vide) est ignorée.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextesFilters<'a> {
    pub code: Option<&'a str>,
    pub jurisdiction: Option<&'a str>,
    pub nature: Option<&'a str>,
    pub source: Option<&'a str>,
    /// Sur-facette « portée » : `norme` | `doctrine_administrative`.
    pub scope: Option<&'a str>,
}

/// Erreur de la frontiere API.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for ApiError {}

/// Ce qu'une recherche ratée montre : un titre et la conduite à tenir. Le 429 et
/// le 422 ont la leur, ce sont les échecs que l'utilisateur peut résoudre.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchFailure {
    pub title: String,
    pub hint: String,
}

impl SearchFailure {
    /// Takes the query because a 422 has two causes calling for opposite
    /// gestures, and the refusal body is not carried this far.
    pub fn from_error(err: &ApiError, query: &str) -> Self {
        match err.status {
            429 => Self {
                title: "Trop de recherches en peu de temps.".to_string(),
                hint: "Réessayez dans une minute.".to_string(),
            },
            422 if query.chars().count() > lj_dtos::QUERY_MAX_CHARS => Self {
                title: format!(
                    "Cette recherche fait {} caractères, le maximum est {}.",
                    query.chars().count(),
                    lj_dtos::QUERY_MAX_CHARS
                ),
                hint: "Gardez la question, retirez le texte collé autour : le moteur cherche sur le sens des mots, pas sur la longueur.".to_string(),
            },
            422 => Self {
                title: "Un filtre de cette recherche n'est pas reconnu.".to_string(),
                hint: "Le lien porte sans doute une valeur périmée : retirez le filtre pour relancer la recherche.".to_string(),
            },
            _ => Self {
                title: err.message.clone(),
                hint: "L'API est peut-être indisponible. Réessayez dans un instant.".to_string(),
            },
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ════════════════════════════ SSR : services in-process ════════════════════════════

/// Client SSR : appelle la couche service `lj-api` sur l'[`AppState`] partagé,
/// sans HTTP. Seules les routes rendues côté serveur (recherche amorcée,
/// détail décision + voisins) sont portées ici ; le reste de l'API (`/me`,
/// signets, historique…) est client-only (`hydrate`).
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct ApiClient {
    state: lj_api::state::AppState,
}

#[cfg(feature = "ssr")]
impl ApiClient {
    /// Récupère l'[`AppState`] fourni au contexte Leptos par `lj-server`.
    pub fn from_context() -> Self {
        let state = leptos::prelude::use_context::<lj_api::state::AppState>()
            .expect("AppState fourni au contexte SSR par lj-server (leptos_routes_with_context)");
        Self { state }
    }

    /// Jamais atteinte au rendu de `/decisions` : la page y peint son squelette
    /// (`search_is_loading` vrai côté serveur, la ressource n'est donc pas lue) et
    /// la recherche part du client à l'hydratation.
    pub async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse> {
        // Rendu SSR = client in-process sans session (auth côté hydrate) → anonyme.
        // Pas de budget non plus : l'appel n'a pas de socket client à nommer, et la
        // recherche de l'utilisateur part du wasm sur `/internal/search`, où elle est
        // comptée.
        lj_api::search::search(
            &self.state,
            request,
            lj_api::usage::Caller::web_anonymous(context),
            None,
        )
        .await
        .map_err(map_service_error)
    }

    /// Décompte et facettes de la même recherche, second appel de la page.
    pub async fn search_count(
        &self,
        request: &SearchRequest,
    ) -> ApiResult<lj_dtos::SearchCountResponse> {
        lj_api::search::count(&self.state, request, None)
            .await
            .map_err(map_service_error)
    }

    /// Suggestions multi-mots de la barre de recherche (in-process, jamais appelée
    /// au rendu SSR : le fetch part d'un Effect client).
    pub async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        lj_api::search::suggest::suggest(&self.state, q, mode)
            .await
            .map_err(map_service_error)
    }

    pub async fn fetch_decision(&self, id: &str) -> ApiResult<DecisionDetail> {
        lj_api::decisions::get_decision(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    pub async fn fetch_similar_decisions(&self, id: &str) -> ApiResult<SimilarDecisionsResponse> {
        lj_api::decisions::similar_decisions(&self.state, id, 4)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    // ── Référentiel LEGI (`/texte`) ───────────────────────────────────────────

    /// Article LEGI à une date (`date = Some`) ou en vigueur (`date = None`),
    /// avec sa timeline. Slug inconnu / article absent ⇒ 404.
    pub async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
    ) -> ApiResult<LawArticleResponse> {
        let result = match date {
            Some(date) => lj_api::legi::article_at_date_str(&self.state, code, num, date).await,
            None => lj_api::legi::article_in_force(&self.state, code, num).await,
        };
        result.map_err(|e| map_not_found(e, "Article introuvable"))
    }

    /// Comparaison de deux versions d'un article. Bornes = dates ISO de fenêtre de
    /// version (`initiale` = borne ouverte).
    pub async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
    ) -> ApiResult<LawCompareResponse> {
        lj_api::legi::article_compare(&self.state, code, num, de, a)
            .await
            .map_err(|e| map_not_found(e, "Version introuvable"))
    }

    pub async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        lj_api::legi::article_citing(
            &self.state,
            code,
            num,
            date,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    /// Articles co-cités avec l'article (« souvent cité avec »).
    pub async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        lj_api::legi::article_co_cited(&self.state, code, num)
            .await
            .map_err(|e| map_not_found(e, "Article introuvable"))
    }

    pub async fn fetch_legi_code_summary(&self, code: &str) -> ApiResult<LawCodeSummary> {
        lj_api::legi::code_summary(&self.state, code)
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    /// Recherche plein-texte d'articles (page `/textes`). `code` borne la recherche
    /// à un texte ; `jurisdiction`/`nature`/`source` filtrent le corpus. Chaque
    /// filtre `None`/vide est ignoré côté service. Le `context` ne pilote pas
    /// l'historique ici (le service in-process n'enregistre rien, seul le transport
    /// HTTP le fait) mais il qualifie le signal d'usage : un `teaser` reste un fetch
    /// machine.
    pub async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        lj_api::legi::search_textes(
            &self.state,
            q,
            filters.code,
            filters.jurisdiction,
            filters.nature,
            filters.source,
            filters.scope,
            None,
            i64::from(page.limit),
            i64::from(page.offset),
            lj_api::usage::Caller::web_anonymous(context),
            // SSR in-process : pas de socket, donc pas d'appelant à nommer.
            None,
        )
        .await
        .map_err(map_service_error)
    }

    /// Catalogue du fond codes (`/normes/codes`) : codes et constitutions
    /// (`scope=head` de l'API). Servi depuis le cache process-local 12 h.
    pub async fn fetch_codes_catalogue(&self) -> ApiResult<CodeCatalogueResponse> {
        lj_api::legi::code_catalogue(&self.state, true)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    /// Catalogue des juridictions groupé par famille. Servi depuis le cache
    /// process-local 12 h.
    pub async fn fetch_juridictions(&self) -> ApiResult<JurisdictionCatalogueResponse> {
        lj_api::jurisdiction_hubs::catalogue(&self.state)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    /// Hub d'une juridiction : années couvertes + volume.
    pub async fn fetch_juridiction_hub(&self, code: &str) -> ApiResult<JurisdictionHubResponse> {
        lj_api::jurisdiction_hubs::hub(&self.state, code)
            .await
            .map_err(map_service_error)
    }

    /// Page paginée des décisions d'une juridiction×année.
    pub async fn fetch_juridiction_year(
        &self,
        code: &str,
        year: i32,
        page: u32,
    ) -> ApiResult<JurisdictionYearResponse> {
        lj_api::jurisdiction_hubs::year_page(&self.state, code, year, page)
            .await
            .map_err(map_service_error)
    }

    /// Catalogue des normes par fond. Servi depuis le cache process-local 12 h.
    pub async fn fetch_normes(&self) -> ApiResult<NormCatalogueResponse> {
        lj_api::norm_hubs::catalogue(&self.state)
            .await
            .map(|r| (*r).clone())
            .map_err(map_service_error)
    }

    /// Hub d'un fond de normes : années couvertes + volume.
    pub async fn fetch_norm_fond(&self, fond: &str) -> ApiResult<NormFondResponse> {
        lj_api::norm_hubs::hub(&self.state, fond)
            .await
            .map_err(map_service_error)
    }

    /// Page paginée des textes d'un fond×année (`year = None` = bucket sans date).
    pub async fn fetch_norm_year(
        &self,
        fond: &str,
        year: Option<i32>,
        page: u32,
    ) -> ApiResult<NormYearResponse> {
        lj_api::norm_hubs::year_page(&self.state, fond, year, page)
            .await
            .map_err(map_service_error)
    }

    /// Page du moteur d'annonces (fond `marche`).
    pub async fn search_annonces(
        &self,
        request: &AnnonceSearchRequest,
    ) -> ApiResult<AnnonceSearchResponse> {
        lj_api::annonces::search(&self.state, request)
            .await
            .map_err(map_service_error)
    }

    /// Fiche d'une consultation (`/marche/{uid}`). Uid inconnu ⇒ 404.
    pub async fn fetch_annonce(&self, uid: &str) -> ApiResult<AnnonceDetailResponse> {
        lj_api::annonces::detail(&self.state, uid)
            .await
            .map_err(|e| map_not_found(e, "Consultation introuvable"))
    }

    /// Catalogue du fond annonces (stock ouvert par département et nature).
    pub async fn fetch_annonces_catalogue(&self) -> ApiResult<AnnonceCatalogueResponse> {
        lj_api::annonces::catalogue(&self.state)
            .await
            .map(|c| (*c).clone())
            .map_err(map_service_error)
    }

    /// Compteurs globaux du corpus (page d'accueil), lus dans l'instantané
    /// `corpus_counts` : trois lignes indexées par rendu.
    pub async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        lj_api::stats::corpus_stats(&self.state)
            .await
            .map_err(map_service_error)
    }

    /// Table des matières d'un code (`/texte/{code}/sommaire`). Slug inconnu ⇒ 404.
    pub async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
    ) -> ApiResult<CodeTocResponse> {
        lj_api::legi::code_toc_str(&self.state, code, date)
            .await
            .map_err(|e| map_not_found(e, "Code introuvable"))
    }

    /// Vue-lecture d'une section (`/texte/{code}/section/{cid}`).
    pub async fn fetch_law_section(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
    ) -> ApiResult<LawSectionResponse> {
        lj_api::legi::law_section_str(&self.state, code, cid, date)
            .await
            .map_err(|e| map_not_found(e, "Section introuvable"))
    }

    /// Décisions citant une division (`…/section/{cid}/citing`) : union des
    /// citantes de son sous-arbre à la date demandée.
    pub async fn fetch_law_section_citing(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        lj_api::legi::section_citing(
            &self.state,
            code,
            cid,
            date,
            i64::from(page.limit),
            i64::from(page.offset),
        )
        .await
        .map_err(|e| map_not_found(e, "Section introuvable"))
    }

    // ── Fiche entité (`/entite`) ────────────────────────────────────────────

    /// Identité registre + agrégats contentieux d'une entité. Uid inconnu ⇒ 404.
    pub async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        lj_api::entities::entity_page(&self.state, ns, id)
            .await
            .map_err(|e| map_not_found(e, "Entité introuvable"))
    }

    /// Décisions citant l'entité, paginées (plus récentes d'abord).
    pub async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse> {
        lj_api::entities::entity_decisions(&self.state, ns, id, page, page_size)
            .await
            .map_err(|e| map_not_found(e, "Entité introuvable"))
    }

    /// Volet registre de l'entité (APIs publiques à l'affichage).
    pub async fn fetch_entity_registre(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<EntityRegistreResponse> {
        lj_api::registre::entity_registre(&self.state, ns, id)
            .await
            .map_err(map_service_error)
    }

    /// Acteurs (parties + conseils) extraits d'une décision (encart « Parties »).
    pub async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        lj_api::entities::decision_parties(&self.state, id)
            .await
            .map_err(|e| map_not_found(e, "Décision introuvable"))
    }

    // ── Annuaire des entités (`/annuaire`) ───────────────────────────────────

    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    pub async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        lj_api::entities::annuaire_stats(&self.state)
            .await
            .map_err(map_service_error)
    }

    /// Recherche d'entités (annuaire). `kind` (slug de catégorie) borne si fourni.
    pub async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse> {
        let category = kind.map(resolve_kind).transpose()?;
        lj_api::entities::entity_search(&self.state, q, category, i64::from(limit))
            .await
            .map_err(map_service_error)
    }

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    /// `barreau` filtre les avocats `cnb:`.
    pub async fn fetch_entities_directory(
        &self,
        kind: &str,
        barreau: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let category = resolve_kind(kind)?;
        lj_api::entities::entity_directory(&self.state, category, barreau, page, page_size)
            .await
            .map_err(map_service_error)
    }

    // Listes « Mon activité » : portées par les fetchers infinite-scroll, qui ne
    // s'exécutent que côté client (Effect `hydrate`) — le SSR est anonyme, sans
    // session in-process. Présentes pour que les closures compilent en `ssr`.
    pub async fn list_search_history(&self, _page: PageParams) -> ApiResult<SearchHistoryResponse> {
        Err(anonymous())
    }

    pub async fn list_decision_views(&self, _page: PageParams) -> ApiResult<DecisionViewsResponse> {
        Err(anonymous())
    }
}

/// Slug de catégorie annuaire → catégorie stockée (mapping `lj_api`). Slug
/// inconnu ⇒ 422, parité de la route HTTP (le front valide en amont via
/// `pages::annuaire::common::Kind`).
#[cfg(feature = "ssr")]
fn resolve_kind(kind: &str) -> ApiResult<&'static str> {
    lj_api::entities::kind_to_category(kind).ok_or_else(|| ApiError {
        message: format!("Catégorie inconnue : {kind}"),
        status: 422,
    })
}

/// Erreur « pas de session » du client SSR (anonyme in-process).
#[cfg(feature = "ssr")]
fn anonymous() -> ApiError {
    ApiError {
        message: "auth_required".to_string(),
        status: 401,
    }
}

/// Mappe une erreur service `lj-api` en [`ApiError`] (status + message).
#[cfg(feature = "ssr")]
fn map_service_error(err: lj_api::error::ApiError) -> ApiError {
    ApiError {
        status: err.status(),
        message: err.to_string(),
    }
}

/// Comme [`map_service_error`], mais remplace le message d'un 404 par `not_found`
/// (parité du message spécifique des routes décision).
#[cfg(feature = "ssr")]
fn map_not_found(err: lj_api::error::ApiError, not_found: &str) -> ApiError {
    let status = err.status();
    let message = if status == 404 {
        not_found.to_string()
    } else {
        err.to_string()
    };
    ApiError { message, status }
}

// ════════════════════════════ Hydrate : reqwest /internal same-origin ════════════════════

#[cfg(feature = "hydrate")]
use reqwest::{Client, Method, RequestBuilder, StatusCode};

#[cfg(feature = "hydrate")]
impl ApiError {
    fn new(message: &str, status: StatusCode) -> Self {
        Self {
            message: message.to_string(),
            status: status.as_u16(),
        }
    }

    /// Erreur reseau (fetch/transport echoue avant toute reponse HTTP). Status 0.
    fn transport(err: reqwest::Error) -> Self {
        Self {
            message: err.to_string(),
            status: 0,
        }
    }
}

/// Délai au bout duquel une requête `/internal` du navigateur est abandonnée.
///
/// Calé au-dessus du budget que le serveur s'accorde à lui-même
/// (`statement_timeout=30s`) : une réponse lente mais légitime arrive avant,
/// seule une réponse qui n'arrivera jamais est coupée.
#[cfg(feature = "hydrate")]
const REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// Client hydrate : `reqwest` vers `/internal` (route same-origin par `lj-server`).
#[cfg(feature = "hydrate")]
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: Client,
}

#[cfg(feature = "hydrate")]
impl ApiClient {
    /// Client navigateur : base `{origine}/internal` same-origin, backend `fetch`.
    /// L'URL doit être ABSOLUE : reqwest-wasm parse l'URL via `url::Url` qui
    /// rejette un chemin relatif (`/internal/…`) → "builder error". On préfixe
    /// donc l'origine de la page (`lj-server` sert `/internal` en same-origin).
    pub fn from_context() -> Self {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        Self {
            base_url: format!("{origin}/internal"),
            http: Client::new(),
        }
    }

    /// Construit une requete. La session voyage seule : le cookie `HttpOnly` part
    /// avec toute requete same-origin.
    ///
    /// Toute requête porte [`REQUEST_DEADLINE`] : les pages montent leurs
    /// ressources dans un `<Suspense>` sans `<Transition>`, donc une réponse qui
    /// n'arrive jamais (connexion HTTP/2 morte en silence, onglet réveillé, bascule
    /// de réseau) laisse la zone de contenu vide indéfiniment. Le délai transforme
    /// ce gel en `ApiError` que les pages rendent déjà.
    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .timeout(REQUEST_DEADLINE)
    }

    /// Envoie une requete sans corps, mappant `!ok` -> `ApiError(message)`.
    /// `not_found` permet le message specifique des routes decision.
    async fn send_unit(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<&str>,
    ) -> ApiResult<()> {
        let resp = self.envoyer(builder).await?;
        check_status(&resp, message, not_found)?;
        Ok(())
    }

    /// Envoie une requete et desserialise le corps JSON en `T`.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        builder: RequestBuilder,
        message: &str,
        not_found: Option<&str>,
    ) -> ApiResult<T> {
        let resp = self.envoyer(builder).await?;
        check_status(&resp, message, not_found)?;
        resp.json::<T>().await.map_err(ApiError::transport)
    }

    async fn envoyer(&self, builder: RequestBuilder) -> ApiResult<reqwest::Response> {
        // `build` then `execute`: the deadline set above rides on the request, so
        // the split keeps it, and the method stays readable after the send.
        let requete = builder.build().map_err(ApiError::transport)?;
        let is_get = requete.method() == Method::GET;
        let resp = self
            .http
            .execute(requete)
            .await
            .map_err(ApiError::transport)?;
        note_build(&resp);
        if is_get {
            recover_from_challenge(&resp);
        }
        Ok(resp)
    }

    // ── Recherche ──────────────────────────────────────────────────────────

    pub async fn search(
        &self,
        request: &SearchRequest,
        context: SearchContext,
    ) -> ApiResult<SearchResponse> {
        let mut query = build_search_query(request);
        if context == SearchContext::Teaser {
            query.push_str("&context=teaser");
        }
        let builder = self.request(Method::GET, &format!("/search?{query}"));
        self.send_json(builder, "search failed", None).await
    }

    /// Décompte et facettes de la même recherche, lancé en parallèle d'elle : la
    /// route les retrouve depuis les mêmes paramètres.
    pub async fn search_count(
        &self,
        request: &SearchRequest,
    ) -> ApiResult<lj_dtos::SearchCountResponse> {
        let query = build_search_query(request);
        let builder = self.request(Method::GET, &format!("/search/decompte?{query}"));
        self.send_json(builder, "search count failed", None).await
    }

    // ── Decision ───────────────────────────────────────────────────────────

    pub async fn fetch_decision(&self, id: &str) -> ApiResult<DecisionDetail> {
        let builder = self.request(Method::GET, &format!("/decision/{id}"));
        self.send_json(
            builder,
            "decision fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    pub async fn fetch_similar_decisions(&self, id: &str) -> ApiResult<SimilarDecisionsResponse> {
        let builder = self.request(Method::GET, &format!("/decision/{id}/similar"));
        self.send_json(
            builder,
            "similar decisions fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    /// Prévisualisation légère d'une décision (hover card). Client-only : le survol
    /// n'existe qu'au navigateur.
    pub async fn fetch_decision_preview(&self, id: &str) -> ApiResult<DecisionPreview> {
        let builder = self.request(Method::GET, &format!("/decision/{id}/preview"));
        self.send_json(
            builder,
            "decision preview fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    // ── Référentiel LEGI (`/texte`) ───────────────────────────────────────────

    pub async fn fetch_legi_article(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
    ) -> ApiResult<LawArticleResponse> {
        let path = match date {
            Some(date) => format!("/texte/{code}/{num}/{date}"),
            None => format!("/texte/{code}/{num}"),
        };
        let builder = self.request(Method::GET, &path);
        self.send_json(
            builder,
            "legi article fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    /// Comparaison de deux versions d'un article.
    pub async fn fetch_legi_compare(
        &self,
        code: &str,
        num: &str,
        de: &str,
        a: &str,
    ) -> ApiResult<LawCompareResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/texte/{code}/{num}/compare/{de}/{a}"),
        );
        self.send_json(
            builder,
            "legi compare fetch failed",
            Some("Version introuvable"),
        )
        .await
    }

    pub async fn fetch_legi_citing(
        &self,
        code: &str,
        num: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        let date_q = date.map(|d| format!("&date={d}")).unwrap_or_default();
        let builder = self.request(
            Method::GET,
            &format!("/texte/{code}/{num}/citing{}{date_q}", page_query(page)),
        );
        self.send_json(
            builder,
            "legi citing fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    /// Articles co-cités avec l'article (« souvent cité avec »).
    pub async fn fetch_legi_related(
        &self,
        code: &str,
        num: &str,
    ) -> ApiResult<Vec<CoCitedArticle>> {
        let builder = self.request(Method::GET, &format!("/texte/{code}/{num}/related"));
        self.send_json(
            builder,
            "legi related fetch failed",
            Some("Article introuvable"),
        )
        .await
    }

    pub async fn fetch_legi_code_summary(&self, code: &str) -> ApiResult<LawCodeSummary> {
        let builder = self.request(Method::GET, &format!("/texte/{code}"));
        self.send_json(builder, "legi code fetch failed", Some("Code introuvable"))
            .await
    }

    /// Recherche plein-texte d'articles (page `/textes`).
    pub async fn search_textes(
        &self,
        q: &str,
        filters: TextesFilters<'_>,
        page: PageParams,
        context: SearchContext,
    ) -> ApiResult<ArticleSearchResponse> {
        let mut path = format!(
            "/search-textes?q={}&limit={}&offset={}",
            url_encode(q),
            page.limit,
            page.offset
        );
        if context == SearchContext::Teaser {
            path.push_str("&context=teaser");
        }
        for (key, value) in [
            ("code", filters.code),
            ("jurisdiction", filters.jurisdiction),
            ("nature", filters.nature),
            ("source", filters.source),
            ("scope", filters.scope),
        ] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                path.push_str(&format!("&{key}={}", url_encode(v)));
            }
        }
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "search textes failed", None).await
    }

    /// Catalogue du fond codes (`/normes/codes`) : codes et constitutions
    /// (`scope=head` de l'API).
    pub async fn fetch_codes_catalogue(&self) -> ApiResult<CodeCatalogueResponse> {
        let builder = self.request(Method::GET, "/codes?scope=head");
        self.send_json(builder, "codes catalogue fetch failed", None)
            .await
    }

    /// Catalogue des juridictions groupé par famille.
    pub async fn fetch_juridictions(&self) -> ApiResult<JurisdictionCatalogueResponse> {
        let builder = self.request(Method::GET, "/juridictions");
        self.send_json(builder, "juridictions catalogue fetch failed", None)
            .await
    }

    /// Hub d'une juridiction : années couvertes + volume.
    pub async fn fetch_juridiction_hub(&self, code: &str) -> ApiResult<JurisdictionHubResponse> {
        let path = format!("/juridictions/{code}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "juridiction hub fetch failed", None)
            .await
    }

    /// Page paginée des décisions d'une juridiction×année.
    pub async fn fetch_juridiction_year(
        &self,
        code: &str,
        year: i32,
        page: u32,
    ) -> ApiResult<JurisdictionYearResponse> {
        let path = format!("/juridictions/{code}/{year}?page={page}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "juridiction year fetch failed", None)
            .await
    }

    /// Catalogue des normes par fond.
    pub async fn fetch_normes(&self) -> ApiResult<NormCatalogueResponse> {
        let builder = self.request(Method::GET, "/normes");
        self.send_json(builder, "normes catalogue fetch failed", None)
            .await
    }

    /// Hub d'un fond de normes : années couvertes + volume.
    pub async fn fetch_norm_fond(&self, fond: &str) -> ApiResult<NormFondResponse> {
        let path = format!("/normes/{fond}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "norm fond fetch failed", None)
            .await
    }

    /// Page paginée des textes d'un fond×année (`year = None` = bucket sans date).
    pub async fn fetch_norm_year(
        &self,
        fond: &str,
        year: Option<i32>,
        page: u32,
    ) -> ApiResult<NormYearResponse> {
        let annee = match year {
            Some(y) => y.to_string(),
            None => "sans-date".to_string(),
        };
        let path = format!("/normes/{fond}/{annee}?page={page}");
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "norm year fetch failed", None)
            .await
    }

    /// Page du moteur d'annonces.
    pub async fn search_annonces(
        &self,
        request: &AnnonceSearchRequest,
    ) -> ApiResult<AnnonceSearchResponse> {
        let path = format!("/annonces?{}", build_annonces_query(request));
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "annonces search failed", None)
            .await
    }

    /// Fiche d'une consultation (`/marche/{uid}`).
    pub async fn fetch_annonce(&self, uid: &str) -> ApiResult<AnnonceDetailResponse> {
        let path = format!("/annonces/{}", url_encode(uid));
        let builder = self.request(Method::GET, &path);
        self.send_json(
            builder,
            "annonce fetch failed",
            Some("Consultation introuvable"),
        )
        .await
    }

    /// Catalogue du fond annonces.
    pub async fn fetch_annonces_catalogue(&self) -> ApiResult<AnnonceCatalogueResponse> {
        let builder = self.request(Method::GET, "/annonces/catalogue");
        self.send_json(builder, "annonces catalogue fetch failed", None)
            .await
    }

    /// Compteurs globaux du corpus (page d'accueil).
    pub async fn fetch_corpus_stats(&self) -> ApiResult<CorpusStatsResponse> {
        let builder = self.request(Method::GET, "/corpus-stats");
        self.send_json(builder, "corpus stats fetch failed", None)
            .await
    }

    /// Table des matières d'un code (`/texte/{code}/sommaire`). Slug inconnu ⇒ 404.
    pub async fn fetch_code_toc(
        &self,
        code: &str,
        date: Option<&str>,
    ) -> ApiResult<CodeTocResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let builder = self.request(Method::GET, &format!("/texte/{code}/sommaire{date_q}"));
        self.send_json(builder, "code toc fetch failed", Some("Code introuvable"))
            .await
    }

    /// Vue-lecture d'une section (`/texte/{code}/section/{cid}`).
    pub async fn fetch_law_section(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
    ) -> ApiResult<LawSectionResponse> {
        let date_q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let builder = self.request(Method::GET, &format!("/texte/{code}/section/{cid}{date_q}"));
        self.send_json(
            builder,
            "law section fetch failed",
            Some("Section introuvable"),
        )
        .await
    }

    /// Décisions citant une division (`…/section/{cid}/citing`).
    pub async fn fetch_law_section_citing(
        &self,
        code: &str,
        cid: &str,
        date: Option<&str>,
        page: PageParams,
    ) -> ApiResult<Vec<CitingDecisionHit>> {
        let date_q = date.map(|d| format!("&date={d}")).unwrap_or_default();
        let builder = self.request(
            Method::GET,
            &format!(
                "/texte/{code}/section/{cid}/citing{}{date_q}",
                page_query(page)
            ),
        );
        self.send_json(
            builder,
            "law section citing fetch failed",
            Some("Section introuvable"),
        )
        .await
    }

    // ── Fiche entité (`/entite`) ────────────────────────────────────────────

    pub async fn fetch_entity(&self, ns: &str, id: &str) -> ApiResult<EntityPageResponse> {
        let builder = self.request(Method::GET, &format!("/entity/{ns}/{id}"));
        self.send_json(builder, "entity fetch failed", Some("Entité introuvable"))
            .await
    }

    pub async fn fetch_entity_decisions(
        &self,
        ns: &str,
        id: &str,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDecisionsResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/entity/{ns}/{id}/decisions?page={page}&page_size={page_size}"),
        );
        self.send_json(
            builder,
            "entity decisions fetch failed",
            Some("Entité introuvable"),
        )
        .await
    }

    /// Volet registre de l'entité (APIs publiques à l'affichage).
    pub async fn fetch_entity_registre(
        &self,
        ns: &str,
        id: &str,
    ) -> ApiResult<EntityRegistreResponse> {
        let builder = self.request(Method::GET, &format!("/entity/{ns}/{id}/registre"));
        self.send_json(builder, "entity registre fetch failed", None)
            .await
    }

    pub async fn fetch_decision_parties(&self, id: &str) -> ApiResult<DecisionPartiesResponse> {
        let builder = self.request(Method::GET, &format!("/decision/{id}/parties"));
        self.send_json(
            builder,
            "decision parties fetch failed",
            Some("Décision introuvable"),
        )
        .await
    }

    // ── Autocomplétion ────────────────────────────────────────────────────────

    /// Suggestions multi-mots de la barre de recherche.
    /// `mode` ∈ `jurisprudence` | `textes` | `annuaire`.
    pub async fn suggest(&self, q: &str, mode: &str) -> ApiResult<SuggestResponse> {
        let path = format!("/suggest?q={}&mode={mode}", url_encode(q));
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "suggest fetch failed", None).await
    }

    // ── Annuaire des entités (`/annuaire`) ───────────────────────────────────

    /// Compteurs d'entités avec contentieux par catégorie (accueil annuaire).
    pub async fn fetch_annuaire_stats(&self) -> ApiResult<AnnuaireStatsResponse> {
        let builder = self.request(Method::GET, "/entities/stats");
        self.send_json(builder, "annuaire stats fetch failed", None)
            .await
    }

    /// Recherche d'entités (annuaire). `kind` borne à une catégorie si fourni.
    pub async fn search_entities(
        &self,
        q: &str,
        kind: Option<&str>,
        limit: u32,
    ) -> ApiResult<EntitySearchResponse> {
        let mut path = format!("/entities/search?q={}&limit={limit}", url_encode(q));
        if let Some(kind) = kind.filter(|k| !k.is_empty()) {
            path.push_str(&format!("&kind={}", url_encode(kind)));
        }
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "entities search failed", None)
            .await
    }

    /// Listing paginé d'une catégorie (tri contentieux décroissant côté API).
    /// `barreau` filtre les avocats `cnb:`.
    pub async fn fetch_entities_directory(
        &self,
        kind: &str,
        barreau: Option<&str>,
        page: i64,
        page_size: i64,
    ) -> ApiResult<EntityDirectoryResponse> {
        let mut path = format!(
            "/entities/directory?kind={}&page={page}&page_size={page_size}",
            url_encode(kind)
        );
        if let Some(barreau) = barreau.filter(|b| !b.is_empty()) {
            path.push_str(&format!("&barreau={}", url_encode(barreau)));
        }
        let builder = self.request(Method::GET, &path);
        self.send_json(builder, "entities directory fetch failed", None)
            .await
    }

    // ── Compte ─────────────────────────────────────────────────────────────

    pub async fn fetch_me(&self) -> ApiResult<UserProfile> {
        let builder = self.request(Method::GET, "/me");
        self.send_json(builder, "me fetch failed", None).await
    }

    /// Déconnexion de CE navigateur. Sans effet sur les autorisations MCP :
    /// elles vivent dans nos tables de jetons, cycle de vie distinct de la
    /// session web.
    pub async fn logout(&self) -> ApiResult<()> {
        let builder = self.request(Method::POST, "/auth/logout");
        self.send_unit(builder, "logout failed", None).await
    }

    /// Résumé du compte pour `/profile` en navigation SPA — le premier rendu le
    /// lit côté serveur, sans passer ici.
    pub async fn fetch_account(&self) -> ApiResult<AccountSummary> {
        let builder = self.request(Method::GET, "/account");
        self.send_json(builder, "account fetch failed", None).await
    }

    pub async fn set_activity_tracking(&self, enabled: bool) -> ApiResult<UserProfile> {
        let builder = self
            .request(Method::PUT, "/me/activity-tracking")
            .json(&lj_dtos::ActivityTrackingUpdate { enabled });
        self.send_json(builder, "activity tracking update failed", None)
            .await
    }

    // ── Clés de la surface publique ──────────────────────────────────────────

    pub async fn list_api_keys(&self) -> ApiResult<lj_dtos::ApiKeysResponse> {
        let builder = self.request(Method::GET, "/me/api-keys");
        self.send_json(builder, "api keys fetch failed", None).await
    }

    /// Le secret ne revient que là : il n'est pas stocké en clair, donc la liste
    /// ne le resservira pas.
    pub async fn create_api_key(&self, label: &str) -> ApiResult<lj_dtos::ApiKeyCreated> {
        let builder = self
            .request(Method::POST, "/me/api-keys")
            .json(&lj_dtos::ApiKeyCreate {
                label: label.to_string(),
            });
        self.send_json(builder, "api key create failed", None).await
    }

    pub async fn revoke_api_key(&self, id: &str) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, &format!("/me/api-keys/{id}"));
        self.send_unit(builder, "api key revoke failed", None).await
    }

    // ── Signets ──────────────────────────────────────────────────────────────

    pub async fn list_bookmarks(&self) -> ApiResult<BookmarksResponse> {
        let builder = self.request(Method::GET, "/me/bookmarks");
        self.send_json(builder, "bookmarks fetch failed", None)
            .await
    }

    pub async fn add_bookmark(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self.request(Method::PUT, &format!("/me/bookmarks/{decision_id}"));
        self.send_unit(builder, "bookmark add failed", None).await
    }

    pub async fn remove_bookmark(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, &format!("/me/bookmarks/{decision_id}"));
        self.send_unit(builder, "bookmark remove failed", None)
            .await
    }

    // ── Historique de recherche ────────────────────────────────────────────────

    pub async fn list_search_history(&self, page: PageParams) -> ApiResult<SearchHistoryResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/me/search-history{}", page_query(page)),
        );
        self.send_json(builder, "history fetch failed", None).await
    }

    pub async fn delete_search_history_entry(&self, entry_id: i64) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, &format!("/me/search-history/{entry_id}"));
        self.send_unit(builder, "history delete failed", None).await
    }

    pub async fn clear_search_history(&self) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, "/me/search-history");
        self.send_unit(builder, "history clear failed", None).await
    }

    // ── Decisions consultees ───────────────────────────────────────────────────

    pub async fn list_decision_views(&self, page: PageParams) -> ApiResult<DecisionViewsResponse> {
        let builder = self.request(
            Method::GET,
            &format!("/me/decision-views{}", page_query(page)),
        );
        self.send_json(builder, "decision views fetch failed", None)
            .await
    }

    pub async fn record_decision_view(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self.request(Method::POST, &format!("/me/decision-views/{decision_id}"));
        self.send_unit(builder, "decision view record failed", None)
            .await
    }

    pub async fn delete_decision_view(&self, decision_id: &str) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, &format!("/me/decision-views/{decision_id}"));
        self.send_unit(builder, "decision view delete failed", None)
            .await
    }

    pub async fn clear_decision_views(&self) -> ApiResult<()> {
        let builder = self.request(Method::DELETE, "/me/decision-views");
        self.send_unit(builder, "decision views clear failed", None)
            .await
    }
}

/// Key of the one-shot marker of a reload already spent in this tab.
#[cfg(feature = "hydrate")]
const CHALLENGE_RELOAD: &str = "lj-defi-recharge";

/// A 403 here can only come from the edge: the API returns none, on any route. An
/// edge challenge needs a **document** response to be solved, which a `fetch`
/// cannot show, so one reload per tab hands the browser one. One, never two: a
/// client the edge always challenges would otherwise reload forever.
#[cfg(feature = "hydrate")]
fn recover_from_challenge(resp: &reqwest::Response) {
    if resp.status() != StatusCode::FORBIDDEN {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.session_storage() else {
        return;
    };
    if storage.get_item(CHALLENGE_RELOAD).ok().flatten().is_some() {
        return;
    }
    let _ = storage.set_item(CHALLENGE_RELOAD, "1");
    let _ = window.location().reload();
}

/// Transmet au suivi de l'onglet l'identite du bundle portee par la reponse. Le
/// suivi lui-meme vit dans le script de tete (`app::BANDEAU`), pose avant
/// l'hydratation : une seule memoire par onglet, une seule annonce.
#[cfg(feature = "hydrate")]
fn note_build(resp: &reqwest::Response) {
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;

    let Some(id) = resp
        .headers()
        .get("x-lj-front-build")
        .and_then(|v| v.to_str().ok())
    else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(suivi) = js_sys::Reflect::get(&window, &JsValue::from_str("__ljBuild")) else {
        return;
    };
    if let Ok(suivi) = suivi.dyn_into::<js_sys::Function>() {
        let _ = suivi.call1(&window, &JsValue::from_str(id));
    }
}

/// Verifie le status HTTP, mappant `404` vers `not_found` (si fourni) et tout
/// autre `!ok` vers `message`.
#[cfg(feature = "hydrate")]
fn check_status(resp: &reqwest::Response, message: &str, not_found: Option<&str>) -> ApiResult<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::NOT_FOUND {
        if let Some(nf) = not_found {
            return Err(ApiError::new(nf, status));
        }
    }
    Err(ApiError::new(message, status))
}

/// Query string `?limit=&offset=`.
#[cfg(feature = "hydrate")]
fn page_query(page: PageParams) -> String {
    format!("?limit={}&offset={}", page.limit, page.offset)
}

/// Encode un `SearchRequest` en query string (clés camelCase, omissions
/// conditionnelles).
#[cfg(any(feature = "hydrate", test))]
fn build_search_query(request: &SearchRequest) -> String {
    use lj_dtos::SortOrder;

    let mut params: Vec<(String, String)> = Vec::new();
    params.push(("q".to_string(), request.query.clone()));

    append_multi(&mut params, "jurisdictionType", &request.jurisdiction_type);
    append_multi(&mut params, "solution", &request.solution);
    append_multi(&mut params, "procedure", &request.procedure);
    append_multi(&mut params, "office", &request.office);
    append_multi(&mut params, "legalDomain", &request.legal_domain);
    append_multi_str(&mut params, "jurisdictionCode", &request.jurisdiction_code);
    append_multi_str(&mut params, "seat", &request.seat);
    append_multi_str(&mut params, "legalInstrument", &request.legal_instrument);
    append_multi_str(&mut params, "legalArticle", &request.legal_article);
    append_multi_str(&mut params, "publication", &request.publication);

    if let Some(date_from) = &request.date_from {
        params.push(("dateFrom".to_string(), date_from.clone()));
    }
    if let Some(date_to) = &request.date_to {
        params.push(("dateTo".to_string(), date_to.clone()));
    }
    // `mode` toujours emis : le DTO le materialise (defaut Auto). L'API resout
    // Auto cote serveur.
    params.push(("mode".to_string(), enum_value(&request.mode)));
    if !matches!(request.sort, SortOrder::Relevance) {
        params.push(("sort".to_string(), enum_value(&request.sort)));
    }
    params.push(("limit".to_string(), request.limit.to_string()));
    if request.offset != 0 {
        params.push(("offset".to_string(), request.offset.to_string()));
    }
    if request.ai_mode {
        params.push(("aiMode".to_string(), "true".to_string()));
    }

    encode_pairs(&params)
}

/// Encode un `AnnonceSearchRequest` en query string — mêmes clés que l'URL de
/// la page `/annonces`, listes en paramètres répétés.
#[cfg(feature = "hydrate")]
fn build_annonces_query(request: &AnnonceSearchRequest) -> String {
    let mut params: Vec<(String, String)> = Vec::new();
    if let Some(q) = request.q.as_deref().filter(|q| !q.is_empty()) {
        params.push(("q".to_string(), q.to_string()));
    }
    for (key, values) in [
        ("fond", &request.fond),
        ("departement", &request.departement),
        ("famille", &request.famille),
        ("cpv", &request.cpv),
    ] {
        for v in values {
            params.push((key.to_string(), v.clone()));
        }
    }
    if let Some(j) = request.echeance_jours {
        params.push(("echeanceJours".to_string(), j.to_string()));
    }
    if request.closes {
        params.push(("closes".to_string(), "true".to_string()));
    }
    params.push(("sort".to_string(), enum_value(&request.sort)));
    params.push(("limit".to_string(), request.limit.to_string()));
    if request.offset != 0 {
        params.push(("offset".to_string(), request.offset.to_string()));
    }
    encode_pairs(&params)
}

/// Serialise une valeur enum DTO en sa chaine serde (sans guillemets).
#[cfg(any(feature = "hydrate", test))]
fn enum_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .expect("enum DTO serialise en chaine")
}

/// Ajoute une cle multivaluee a partir d'enums DTO.
#[cfg(any(feature = "hydrate", test))]
fn append_multi<T: serde::Serialize>(
    params: &mut Vec<(String, String)>,
    key: &str,
    values: &Option<Vec<T>>,
) {
    if let Some(values) = values {
        for v in values {
            params.push((key.to_string(), enum_value(v)));
        }
    }
}

/// Ajoute une cle multivaluee de chaines.
#[cfg(any(feature = "hydrate", test))]
fn append_multi_str(params: &mut Vec<(String, String)>, key: &str, values: &Option<Vec<String>>) {
    if let Some(values) = values {
        for v in values {
            params.push((key.to_string(), v.clone()));
        }
    }
}

/// Encode une liste de paires en query string (RFC 3986, `URLSearchParams`).
#[cfg(any(feature = "hydrate", test))]
fn encode_pairs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode `application/x-www-form-urlencoded` (espace -> `+`).
#[cfg(any(feature = "hydrate", test))]
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_dtos::{Domain, JurisdictionType, Office, Procedure, SearchMode, Solution, SortOrder};

    /// Un filtre invalide n'est pas une panne : le 422 nomme le lien, pas la
    /// disponibilité de l'API.
    #[test]
    fn le_422_ne_met_pas_l_api_en_cause() {
        let echec = SearchFailure::from_error(
            &ApiError {
                message: "search failed".to_string(),
                status: 422,
            },
            "bail commercial",
        );
        assert!(!echec.title.contains("search failed"));
        assert!(!echec.hint.contains("indisponible"));
        assert!(echec.hint.contains("filtre"));

        let panne = SearchFailure::from_error(
            &ApiError {
                message: "search failed".to_string(),
                status: 500,
            },
            "bail commercial",
        );
        assert!(panne.hint.contains("indisponible"));
    }

    /// An over-length 422 sent two people looking for a filter they never set,
    /// measured on 01/09/2026. Both counts must appear, neither may say filter.
    #[test]
    fn over_length_422_names_the_length() {
        let too_long = "x".repeat(lj_dtos::QUERY_MAX_CHARS + 1);
        let echec = SearchFailure::from_error(
            &ApiError {
                message: "search failed".to_string(),
                status: 422,
            },
            &too_long,
        );
        assert!(!echec.title.contains("filtre"));
        assert!(!echec.hint.contains("filtre"));
        assert!(echec
            .title
            .contains(&(lj_dtos::QUERY_MAX_CHARS + 1).to_string()));
        assert!(echec.title.contains(&lj_dtos::QUERY_MAX_CHARS.to_string()));
    }

    /// Toute clé sérialisée par le `SearchRequest` doit partir dans la query
    /// string : le filtre manquant à cette liste ne dit rien, l'URL montre la
    /// facette cochée et l'API cherche sans elle.
    #[test]
    fn build_search_query_emet_chaque_filtre_du_dto() {
        let request = SearchRequest {
            query: "observations devant la cour".to_string(),
            jurisdiction_type: Some(vec![JurisdictionType::Cc]),
            solution: Some(vec![Solution::Rejet]),
            procedure: Some(vec![Procedure::Qpc]),
            office: Some(vec![Office::Jex]),
            legal_domain: Some(vec![Domain::Social]),
            jurisdiction_code: Some(vec!["cc".to_string()]),
            seat: Some(vec!["CC.C2".to_string()]),
            legal_instrument: Some(vec!["code-civil".to_string()]),
            legal_article: Some(vec!["code-civil|203".to_string()]),
            publication: Some(vec!["publie".to_string()]),
            date_from: Some("2020-01-01".to_string()),
            date_to: Some("2026-01-01".to_string()),
            mode: SearchMode::Semantic,
            sort: SortOrder::DateDesc,
            limit: 20,
            offset: 20,
            ai_mode: true,
        };

        let qs = build_search_query(&request);
        let emitted: Vec<&str> = qs
            .split('&')
            .filter_map(|pair| pair.split('=').next())
            .collect();
        let serialized = serde_json::to_value(&request).expect("SearchRequest sérialise");
        for field in serialized.as_object().expect("objet JSON").keys() {
            // Seul renommage entre le DTO et la query string.
            let key = if field == "query" { "q" } else { field };
            assert!(emitted.contains(&key), "clé `{key}` absente de `{qs}`");
        }
    }
}
