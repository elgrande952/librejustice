//! Registre de quota Mistral.
//!
//! Une réponse Mistral porte l'état du quota de la clé
//! (`x-ratelimit-*-minute`), mais aucun `Retry-After` : ce module tient les
//! compteurs et n'admet une requête que s'il a de quoi la payer.
//!
//! Les deux compteurs n'ont pas la même loi :
//!
//! - **requêtes** : seau à frontière dure, remis à plein au top de la minute
//!   murale ;
//! - **tokens** : gelé pendant la minute où l'on consomme, puis la minute
//!   suivante il restitue linéairement ce qui a été consommé.
//!
//! D'où [`Bucket::tok_available`] :
//!
//! ```text
//! restant(t) = limite − consommé(M) − consommé(M−1) × (1 − s/60)
//! ```
//!
//! `M` = minute murale courante, `s` = seconde écoulée dans `M`.

use std::sync::Mutex;
use std::time::Duration;

/// Plafonds d'amorce : valeurs de `mistral-small-2603`, le modèle chat qu'on
/// utilise. Elles évitent qu'un client neuf parte à l'aveugle ; la première réponse
/// les remplace par celles que le service annonce.
const SEED_REQ_PER_MIN: u32 = 50;
const SEED_TOK_PER_MIN: u32 = 50_000;

/// Marge posée après la frontière de minute : la bascule du service traîne de
/// quelques centaines de millisecondes après le top. Avec [`JITTER_SPREAD`], les
/// réveils tombent dans `[500, 750[` ms.
const BOUNDARY_MARGIN: Duration = Duration::from_millis(500);

/// Étalement des réveils, et nombre de crans distincts.
const JITTER_SPREAD: Duration = Duration::from_millis(250);
const JITTER_STEPS: u64 = 16;

/// Plancher d'attente. L'attente sur les tokens vaut `déficit / débit`, qui tend
/// vers zéro quand il ne manque qu'une poignée de tokens ; sans plancher,
/// l'admission re-sonde tous les seaux en boucle serrée pour ne gagner que la
/// granularité du timer.
const MIN_WAIT: Duration = Duration::from_millis(25);

/// Ratio d'amorce tokens/caractère. L'écart avec le ratio réellement facturé est
/// absorbé dès la 1ʳᵉ réponse.
const SEED_TOKENS_PER_CHAR: f64 = 1.0 / lj_core::tokens::CHARS_PER_TOKEN_SAFE;

/// Plafond et reste d'un compteur, tels que la réponse les annonce.
#[derive(Debug, Clone, Copy)]
pub struct Counter {
    pub limit: u32,
    pub left: u32,
}

/// Compteurs de quota lus sur une réponse Mistral.
///
/// Un 200 les porte tous. Un refus ne porte que la paire de la ressource épuisée —
/// requêtes ou tokens, jamais les deux, jamais le coût. Chaque paire se lit donc
/// pour elle-même.
#[derive(Debug, Clone, Copy, Default)]
pub struct Quota {
    pub req: Option<Counter>,
    pub tok: Option<Counter>,
    /// `x-ratelimit-tokens-query-cost` — coût facturé de CETTE requête. Exact :
    /// la somme des coûts colle au décrément du restant, à l'unité.
    pub cost: Option<u32>,
}

impl Quota {
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let num = |name: &str| -> Option<u32> { headers.get(name)?.to_str().ok()?.parse().ok() };
        let pair = |limit: &str, left: &str| {
            Some(Counter {
                limit: num(limit)?,
                left: num(left)?,
            })
        };
        Self {
            req: pair(
                "x-ratelimit-limit-req-minute",
                "x-ratelimit-remaining-req-minute",
            ),
            tok: pair(
                "x-ratelimit-limit-tokens-minute",
                "x-ratelimit-remaining-tokens-minute",
            ),
            cost: num("x-ratelimit-tokens-query-cost"),
        }
    }
}

/// Horloge murale en secondes depuis l'époque. La frontière des seaux est murale,
/// pas monotone : `Instant` ne saurait pas la situer.
pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// État de quota d'une clé.
#[derive(Debug)]
struct Bucket {
    req_limit: u32,
    req_left: u32,
    tok_limit: u32,
    /// Tokens consommés pendant `minute`, tous consommateurs de la clé confondus
    /// (déduit des en-têtes, pas de nos seules réservations).
    consumed_this: f64,
    /// Consommés pendant la minute précédente. C'est ce que le service restitue
    /// linéairement pendant `minute`.
    consumed_prev: f64,
    /// Minute murale (unix / 60) à laquelle `req_left` et `consumed_this` se rapportent.
    minute: u64,
    inflight_req: u32,
    inflight_tok: u32,
    /// Vrai dès qu'une réponse a parlé des tokens de cette clé. Avant, les
    /// compteurs ne sont que les plafonds d'amorce, et ils supposent le seau
    /// libre — ce qu'un process neuf ne peut pas savoir : le seau est tenu par le
    /// service, partagé avec qui d'autre tape la clé, et ne s'observe qu'en le
    /// consommant.
    informed: bool,
}

impl Bucket {
    fn seed(now: f64) -> Self {
        Self {
            req_limit: SEED_REQ_PER_MIN,
            req_left: SEED_REQ_PER_MIN,
            tok_limit: SEED_TOK_PER_MIN,
            consumed_this: 0.0,
            consumed_prev: 0.0,
            minute: (now / 60.0) as u64,
            inflight_req: 0,
            inflight_tok: 0,
            informed: false,
        }
    }

    /// Avance à la minute murale de `now` si elle a tourné : le seau de requêtes
    /// repart plein, et la consommation de la minute écoulée devient ce qui sera
    /// restitué pendant celle qui commence. Deux minutes ou plus d'écart ⇒ tout a
    /// déjà été rendu.
    fn roll(&mut self, now: f64) {
        let m = (now / 60.0) as u64;
        if m <= self.minute {
            return;
        }
        self.consumed_prev = if m == self.minute + 1 {
            self.consumed_this
        } else {
            0.0
        };
        self.consumed_this = 0.0;
        self.req_left = self.req_limit;
        self.minute = m;
    }

    /// Tokens payables à `now`, réservations en vol déduites. Peut être négatif :
    /// l'appelant en tire le déficit à combler.
    fn tok_available(&self, now: f64) -> f64 {
        let unrestored = self.consumed_prev * (1.0 - (now % 60.0) / 60.0);
        self.tok_limit as f64 - self.consumed_this - unrestored - f64::from(self.inflight_tok)
    }

    /// Débit de restitution en cours : la minute rend en 60 s ce que la
    /// précédente a consommé. Zéro si rien n'a été consommé avant.
    fn refill_per_s(&self) -> f64 {
        self.consumed_prev / 60.0
    }

    fn try_reserve(&mut self, cost: u32, now: f64) -> Result<(), Duration> {
        self.roll(now);
        // Démarrage prudent : tant qu'aucune réponse n'a parlé de cette clé, un
        // seul appel en vol. Il sert de sonde — son `settle` rend les compteurs
        // exacts, les suivants partent informés. Sans ça, la première vague vaut
        // ce que vaut l'amorce, et l'amorce ignore les autres consommateurs.
        if !self.informed && self.inflight_req > 0 {
            return Err(MIN_WAIT);
        }
        // Une requête plus chère que le seau entier ne passerait jamais : on la laisse
        // partir se faire refuser par le service.
        let cost = cost.min(self.tok_limit);
        let to_boundary = Duration::from_secs_f64(60.0 - now % 60.0) + BOUNDARY_MARGIN;

        let req_ok = self.req_left > self.inflight_req;
        let available = self.tok_available(now);
        let tok_ok = available >= f64::from(cost);
        if req_ok && tok_ok {
            self.inflight_req += 1;
            self.inflight_tok += cost;
            return Ok(());
        }

        let req_wait = if req_ok { Duration::ZERO } else { to_boundary };
        let tok_wait = if tok_ok {
            Duration::ZERO
        } else {
            let rate = self.refill_per_s();
            if rate <= 0.0 {
                // Rien à restituer cette minute : la capacité ne revient qu'au
                // prochain top, quand la consommation courante bascule en dette.
                to_boundary
            } else {
                // Borné à la frontière : le débit y change, on se réévalue.
                Duration::from_secs_f64((f64::from(cost) - available) / rate).min(to_boundary)
            }
        };
        Err(req_wait.max(tok_wait))
    }

    fn release(&mut self, reserved: u32) {
        self.inflight_req = self.inflight_req.saturating_sub(1);
        self.inflight_tok = self.inflight_tok.saturating_sub(reserved);
    }

    /// Réconcilie sur les compteurs de la réponse — ils font autorité. Chaque
    /// paire s'applique séparément, un refus n'en portant qu'une. Inverse de
    /// [`tok_available`](Self::tok_available) : ce qu'il reste dit ce qui a été
    /// consommé dans la minute, y compris par d'autres processus sur la clé.
    fn settle(&mut self, q: Quota, now: f64) {
        self.roll(now);
        if let Some(req) = q.req {
            self.req_limit = req.limit;
            self.req_left = req.left;
        }
        if let Some(tok) = q.tok {
            self.tok_limit = tok.limit;
            let unrestored = self.consumed_prev * (1.0 - (now % 60.0) / 60.0);
            self.consumed_this = (tok.limit as f64 - tok.left as f64 - unrestored).max(0.0);
            self.informed = true;
        }
    }
}

/// Estimateur du coût d'une requête, en tokens facturés par caractère de payload,
/// mis à jour en moyenne glissante sur le coût réellement facturé.
///
/// L'estimation ne sert qu'à réserver ; [`Bucket::settle`] rend le compte juste.
/// Elle ne peut pas viser mieux : à payload constant, le préfixe déjà en cache
/// change ce qui est facturé. C'est un régime stable, pas une anomalie — la moyenne
/// le suit au lieu d'y résister.
#[derive(Debug)]
struct CostModel {
    tokens_per_char: Mutex<f64>,
}

impl CostModel {
    fn new() -> Self {
        Self {
            tokens_per_char: Mutex::new(SEED_TOKENS_PER_CHAR),
        }
    }

    fn estimate(&self, payload_chars: usize) -> u32 {
        let ratio = *self
            .tokens_per_char
            .lock()
            .expect("mutex coût non empoisonné");
        (payload_chars as f64 * ratio).ceil() as u32
    }

    fn observe(&self, payload_chars: usize, billed: u32) {
        if payload_chars == 0 {
            return;
        }
        let observed = f64::from(billed) / payload_chars as f64;
        let mut cur = self
            .tokens_per_char
            .lock()
            .expect("mutex coût non empoisonné");
        *cur = *cur * 0.8 + observed * 0.2;
    }
}

/// Registre de quota d'un pool de clés — un seau par clé.
///
/// Le seau est par organisation, et les clés en sont une chacune. Pas de dimension
/// modèle : le seul autre modèle appelé (OCR) part d'une autre instance de client,
/// donc d'un autre registre.
#[derive(Debug)]
pub struct Ledger {
    buckets: Vec<Mutex<Bucket>>,
    cost: CostModel,
    /// Nombre d'attentes servies, pour étaler les réveils.
    waits: std::sync::atomic::AtomicU64,
}

/// Réservation en vol. Libère son budget en [`Drop`] si elle n'a pas été
/// réconciliée : une requête annulée par l'échéance du rerank ne fuit pas.
pub struct Reservation<'a> {
    ledger: &'a Ledger,
    key_idx: usize,
    cost: u32,
    payload_chars: usize,
    settled: bool,
}

impl Reservation<'_> {
    pub fn key_idx(&self) -> usize {
        self.key_idx
    }

    /// Verse les compteurs de la réponse et rend la réservation.
    pub fn settle(mut self, quota: Quota, now: f64) {
        if let Some(cost) = quota.cost {
            self.ledger.cost.observe(self.payload_chars, cost);
        }
        let mut b = self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné");
        b.release(self.cost);
        b.settle(quota, now);
        self.settled = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.ledger.buckets[self.key_idx]
            .lock()
            .expect("mutex seau non empoisonné")
            .release(self.cost);
    }
}

impl Ledger {
    pub fn new(key_count: usize) -> Self {
        let now = now_unix();
        Self {
            buckets: (0..key_count)
                .map(|_| Mutex::new(Bucket::seed(now)))
                .collect(),
            cost: CostModel::new(),
            waits: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Coût estimé d'un payload de `payload_chars` caractères.
    pub fn estimate(&self, payload_chars: usize) -> u32 {
        self.cost.estimate(payload_chars)
    }

    /// Réserve sur la première clé de `order` qui peut payer. `order` porte la
    /// préférence de l'appelant (round-robin, clés refusées retirées).
    ///
    /// `Err(d)` : aucune ne peut, réessayer dans `d` — la plus courte des attentes,
    /// plus un jitter, jamais moins de [`MIN_WAIT`].
    pub fn reserve(
        &self,
        order: &[usize],
        payload_chars: usize,
        now: f64,
    ) -> Result<Reservation<'_>, Duration> {
        let cost = self.cost.estimate(payload_chars);
        let mut soonest = Duration::MAX;
        for &i in order {
            let mut bucket = self.buckets[i].lock().expect("mutex seau non empoisonné");
            match bucket.try_reserve(cost, now) {
                Ok(()) => {
                    return Ok(Reservation {
                        ledger: self,
                        key_idx: i,
                        cost,
                        payload_chars,
                        settled: false,
                    })
                }
                Err(wait) => soonest = soonest.min(wait),
            }
        }
        let wait = if soonest == Duration::MAX {
            BOUNDARY_MARGIN
        } else {
            soonest
        };
        Err((wait + self.jitter()).max(MIN_WAIT))
    }

    /// Étalement ∈ [0, `JITTER_SPREAD`[, tiré d'un compteur et non de l'horloge : deux
    /// appels refusés à quelques microsecondes d'écart liraient la même sous-seconde.
    fn jitter(&self) -> Duration {
        let n = self
            .waits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        JITTER_SPREAD.mul_f64((n % JITTER_STEPS) as f64 / JITTER_STEPS as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Réponse complète : celle d'un 200.
    fn quota(req_left: u32, tok_left: u32, cost: u32) -> Quota {
        Quota {
            req: Some(Counter {
                limit: 50,
                left: req_left,
            }),
            tok: Some(Counter {
                limit: 50_000,
                left: tok_left,
            }),
            cost: Some(cost),
        }
    }

    /// Un seau neuf n'admet qu'un appel, quelle que soit la place qu'annoncent
    /// ses plafonds d'amorce : ils supposent le seau libre, et un process qui
    /// partage la clé avec un résident ne peut pas le savoir avant d'avoir tiré.
    /// La première réponse le renseigne, et il s'ouvre.
    #[test]
    fn un_seau_neuf_n_admet_qu_une_sonde() {
        let m = 30_000_000u64;
        let t = m as f64 * 60.0 + 10.0;
        let mut b = Bucket::seed(t);

        assert!(b.try_reserve(1_000, t).is_ok());
        assert_eq!(b.try_reserve(1_000, t), Err(MIN_WAIT));

        // Le `settle` de la sonde renseigne le seau : les suivants passent.
        b.settle(quota(43, 45_537, 1097), t);
        assert!(b.informed);
        assert!(b.try_reserve(1_000, t).is_ok());
    }

    /// Charge brûlée à la seconde 10 d'une minute, puis restitution linéaire de la
    /// minute suivante.
    #[test]
    fn la_restitution_suit_la_loi_mesuree() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);

        b.settle(quota(43, 45_537, 1097), m as f64 * 60.0 + 10.0);
        assert_eq!(b.consumed_this.round() as u32, 4_463);

        assert_eq!(
            b.tok_available(m as f64 * 60.0 + 55.6).round() as u32,
            45_537
        );

        let t = (m + 1) as f64 * 60.0 + 54.8;
        b.roll(t);
        assert_eq!(b.consumed_prev.round() as u32, 4_463);
        assert_eq!(b.tok_available(t).round() as u32, 49_613);
    }

    #[test]
    fn le_seau_de_requetes_repart_plein_au_top_de_minute() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);
        b.settle(quota(0, 49_000, 17), m as f64 * 60.0 + 20.0);

        let wait = b.try_reserve(10, m as f64 * 60.0 + 20.0).unwrap_err();
        assert_eq!(wait.as_secs(), 40);

        b.roll((m + 1) as f64 * 60.0 + 1.0);
        assert_eq!(b.req_left, 50);
    }

    /// Tokens insuffisants mais restitution en cours : l'attente est
    /// proportionnelle au déficit, pas un saut à la minute suivante.
    #[test]
    fn l_attente_sur_les_tokens_est_proportionnelle_au_deficit() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);
        // Une consommation observée sur la minute précédente : ce seau a déjà vu
        // une réponse, sinon il n'admettrait qu'une sonde.
        b.informed = true;
        b.consumed_prev = 50_000.0;
        b.consumed_this = 0.0;

        let t = m as f64 * 60.0 + 30.0;
        assert_eq!(b.tok_available(t).round() as u32, 25_000);
        assert!(b.try_reserve(20_000, t).is_ok());

        let wait = b.try_reserve(10_000, t).unwrap_err();
        assert_eq!(wait.as_secs(), 6);
    }

    /// Sans restitution en cours, la capacité ne revient qu'au top de minute.
    #[test]
    fn sans_restitution_l_attente_va_a_la_frontiere() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);
        b.settle(quota(49, 100, 17), m as f64 * 60.0 + 15.0);
        let wait = b.try_reserve(1_000, m as f64 * 60.0 + 15.0).unwrap_err();
        assert_eq!(wait.as_secs(), 45);
    }

    #[test]
    fn une_reservation_abandonnee_rend_son_budget() {
        let ledger = Ledger::new(1);
        let now = now_unix();
        let before = ledger.buckets[0].lock().unwrap().tok_available(now);
        {
            let r = ledger.reserve(&[0], 10_000, now).unwrap();
            assert_eq!(r.key_idx(), 0);
            let during = ledger.buckets[0].lock().unwrap().tok_available(now);
            assert!(during < before, "la réservation doit grever le seau");
        }
        let after = ledger.buckets[0].lock().unwrap().tok_available(now);
        assert_eq!(after, before);
    }

    /// La réservation contourne une clé drainée au lieu de l'attendre.
    #[test]
    fn la_reservation_contourne_la_cle_drainee() {
        let ledger = Ledger::new(2);
        let now = now_unix();
        ledger.buckets[0]
            .lock()
            .unwrap()
            .settle(quota(0, 0, 17), now);
        let r = ledger.reserve(&[0, 1], 1_000, now).unwrap();
        assert_eq!(r.key_idx(), 1);
    }

    /// Le coût facturé ne se déduit pas du payload : l'estimation doit converger
    /// vers ce que le service annonce, si loin de l'amorce soit-il.
    #[test]
    fn le_cout_suit_ce_qui_est_facture() {
        let c = CostModel::new();
        let plein = c.estimate(6_210);
        assert!(plein > 1_500, "amorce non cachée : {plein}");
        for _ in 0..30 {
            c.observe(6_210, 24);
        }
        assert!(
            (24..30).contains(&c.estimate(6_210)),
            "{}",
            c.estimate(6_210)
        );
    }

    /// Un 429 de requêtes n'annonce que `limit`/`remaining-req-minute`. Cette seule
    /// paire doit murer la clé jusqu'au top de minute.
    #[test]
    fn un_refus_de_requetes_mure_la_cle() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);
        let refus = Quota {
            req: Some(Counter { limit: 50, left: 0 }),
            ..Quota::default()
        };
        b.settle(refus, m as f64 * 60.0 + 20.0);
        let wait = b.try_reserve(10, m as f64 * 60.0 + 20.0).unwrap_err();
        assert_eq!(wait.as_secs(), 40);
    }

    /// Un 429 de tokens n'annonce que la paire des tokens, et rien ne dit combien la
    /// minute précédente restitue : l'attente va à la frontière.
    #[test]
    fn un_refus_de_tokens_mure_la_cle() {
        let m = 30_000_000u64;
        let mut b = Bucket::seed(m as f64 * 60.0);
        let refus = Quota {
            tok: Some(Counter {
                limit: 50_000,
                left: 0,
            }),
            ..Quota::default()
        };
        b.settle(refus, m as f64 * 60.0 + 20.0);
        assert!(b.tok_available(m as f64 * 60.0 + 20.0) <= 0.0);
        let wait = b.try_reserve(10, m as f64 * 60.0 + 20.0).unwrap_err();
        assert_eq!(wait.as_secs(), 40);
    }

    /// Un déficit d'une demi-unité se comble en moins d'une milliseconde :
    /// l'admission attend quand même le plancher plutôt que de re-sonder.
    #[test]
    fn l_attente_ne_descend_pas_sous_le_plancher() {
        let ledger = Ledger::new(1);
        let now = 20_000_000.0 * 60.0 + 30.0;
        {
            let mut b = ledger.buckets[0].lock().unwrap();
            b.consumed_prev = 50_000.0;
            b.consumed_this = 24_900.5;
        }
        let wait = |l: &Ledger| match l.reserve(&[0], 320, now) {
            Err(d) => d,
            Ok(_) => panic!("le seau ne peut pas payer, la réservation doit échouer"),
        };
        assert!(wait(&ledger) >= MIN_WAIT);
    }

    /// Deux attentes consécutives ne se réveillent pas au même instant.
    #[test]
    fn les_attentes_sont_etalees() {
        let ledger = Ledger::new(1);
        let now = now_unix();
        ledger.buckets[0]
            .lock()
            .unwrap()
            .settle(quota(0, 0, 17), now);
        let wait = |l: &Ledger| match l.reserve(&[0], 100, now) {
            Err(d) => d,
            Ok(_) => panic!("le seau est à sec, la réservation doit échouer"),
        };
        let a = wait(&ledger);
        let b = wait(&ledger);
        assert_ne!(a, b);
        assert!(b.abs_diff(a) < JITTER_SPREAD);
    }
}
