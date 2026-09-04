//! Identité de l'appelant : une résolution au bord, deux projections.
//!
//! [`Identity::resolve`] lit l'en-tête client **une fois** par requête ; la clé
//! de quota (`ratelimit`) et les pseudonymes exportés ([`Pseudonymiser`]) en
//! dérivent. Leur grain diffère volontairement : l'User-Agent entre dans le
//! pseudonyme de visite et jamais dans la clé de quota — un quota souffre des
//! faux **partages**, une analyse d'usage des faux **regroupements**.
//!
//! Les pseudonymes sont des MAC à clé tronqués. La clé se dérive de
//! `LIBREJUSTICE_API_USAGE_PSEUDONYM_KEY` ; réglage absent ⇒ pas d'instance,
//! donc aucun attribut exporté.

use std::net::IpAddr;

use axum::http::HeaderMap;

/// Contexte de dérivation blake3 : il isole cette clé de tout autre usage qu'on
/// ferait un jour de la même chaîne secrète.
const KEY_CONTEXT: &str = "librejustice 2026-08-13 api usage pseudonym";

/// Longueur du pseudonyme rendu : 12 hex, soit les 6 premiers octets du MAC.
const PSEUDONYM_HEX: usize = 12;

/// Heure de bascule du sel de visite, en secondes depuis minuit UTC.
const SALT_BOUNDARY_SECS: i64 = 3 * 3_600;

const DAY_SECS: i64 = 86_400;

/// Appelant d'une requête, résolu au bord puis projeté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// Le `sub` du compte, quel que soit le chemin d'authentification.
    Account(String),
    /// Réseau de l'appelant : IPv4 entière, IPv6 ramenée au /64.
    Network(String),
    /// Sans socket client : l'appel in-process du rendu SSR.
    Unknown,
}

impl Identity {
    /// Résolution unique au bord : le compte s'il est authentifié, sinon le
    /// réseau, sinon rien.
    pub fn resolve(user: Option<&str>, headers: &HeaderMap) -> Self {
        if let Some(sub) = user {
            return Identity::Account(sub.to_string());
        }
        match client_network(headers) {
            Some(network) => Identity::Network(network),
            None => Identity::Unknown,
        }
    }
}

/// Caller network, read from `CF-Connecting-IP`. The header is only trustworthy
/// because the origin firewall accepts `:443` from the Cloudflare ranges alone,
/// which a system timer refreshes.
///
/// **Never falls back on the peer socket address**: that is the Cloudflare PoP,
/// so every visitor behind one PoP would merge into a single visit.
///
/// L'IPv6 est ramenée à son `/64` pour deux raisons distinctes. Côté quota, un
/// hôte v6 s'en voit attribuer un entier et change d'adresse dedans sans rien
/// payer. Côté visite, les extensions de vie privée font tourner l'adresse
/// complète : la garder créerait de faux dédoublements pires que le faux
/// regroupement qu'on évite. Regroupement résiduel assumé : deux personnes du
/// même foyer ou du même opérateur, même navigateur et même version d'UA,
/// comptent pour une visite.
pub(crate) fn client_network(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("cf-connecting-ip")?.to_str().ok()?;
    match raw.parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some(v4.to_string()),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            Some(format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3]))
        }
    }
}

/// Index de période du sel de visite : le jour dont la borne est 03:00 UTC.
///
/// La rotation est journalière parce que les sessions mesurées vont jusqu'à
/// 56 min, qu'une rotation horaire fragmenterait ; la borne à 03:00 UTC tombe
/// dans le creux de trafic d'une audience française. Ce qui la traverse quand
/// même se scinde en deux visites, c'est assumé.
pub fn salt_period(unix_secs: i64) -> i64 {
    (unix_secs - SALT_BOUNDARY_SECS).div_euclid(DAY_SECS)
}

/// Dérive les pseudonymes exportés en télémétrie.
#[derive(Clone)]
pub struct Pseudonymiser {
    key: [u8; 32],
}

impl Pseudonymiser {
    /// `None` si le réglage est absent ou vide.
    pub fn new(secret: Option<&str>) -> Option<Self> {
        let secret = secret.filter(|s| !s.is_empty())?;
        Some(Self {
            key: blake3::derive_key(KEY_CONTEXT, secret.as_bytes()),
        })
    }

    /// `caller` : pseudonyme stable du compte. `None` hors [`Identity::Account`].
    pub fn caller(&self, id: &Identity) -> Option<String> {
        let Identity::Account(sub) = id else {
            return None;
        };
        Some(self.mac(sub.as_bytes()))
    }

    /// `visit` : pseudonyme de visite, salé par la période. `None` hors
    /// [`Identity::Network`]. `period` vient de [`salt_period`].
    pub fn visit(&self, id: &Identity, user_agent: Option<&str>, period: i64) -> Option<String> {
        let Identity::Network(network) = id else {
            return None;
        };
        // Chaque champ de longueur variable est précédé de sa longueur : avec un
        // séparateur littéral, il suffirait de le poser dans l'User-Agent pour
        // déplacer la frontière et fabriquer une collision.
        let mut input = Vec::new();
        for field in [
            network.as_bytes(),
            user_agent.unwrap_or_default().as_bytes(),
        ] {
            input.extend_from_slice(&(field.len() as u64).to_le_bytes());
            input.extend_from_slice(field);
        }
        input.extend_from_slice(&period.to_le_bytes());
        Some(self.mac(&input))
    }

    fn mac(&self, input: &[u8]) -> String {
        blake3::keyed_hash(&self.key, input).to_hex()[..PSEUDONYM_HEX].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(ip: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", ip.parse().unwrap());
        h
    }

    fn pseudonymiser() -> Pseudonymiser {
        Pseudonymiser::new(Some("clé-de-test")).unwrap()
    }

    fn network(s: &str) -> Identity {
        Identity::Network(s.to_string())
    }

    #[test]
    fn ipv6_is_grouped_by_64() {
        let a = Identity::resolve(None, &headers("2001:db8:1:2:3:4:5:6"));
        let b = Identity::resolve(None, &headers("2001:db8:1:2:ffff::1"));
        assert_eq!(a, b);
        assert_eq!(a, network("2001:db8:1:2::/64"));
        assert_ne!(a, Identity::resolve(None, &headers("2001:db8:1:3::1")));
    }

    #[test]
    fn salt_period_is_constant_within_a_shifted_day() {
        // 2026-08-13T03:00:00Z, puis 23:30:00Z, puis 2026-08-14T02:59:59Z.
        assert_eq!(salt_period(1_786_590_000), salt_period(1_786_663_800));
        assert_eq!(salt_period(1_786_590_000), salt_period(1_786_676_399));
    }

    #[test]
    fn salt_period_turns_at_three_utc() {
        // 2026-08-13T02:59:59Z, puis 03:00:00Z.
        assert_eq!(salt_period(1_786_589_999) + 1, salt_period(1_786_590_000));
        // Avant l'époque, la division doit rester euclidienne.
        assert_eq!(salt_period(SALT_BOUNDARY_SECS), 0);
        assert_eq!(salt_period(SALT_BOUNDARY_SECS - 1), -1);
    }

    #[test]
    fn caller_is_stable_and_separates_accounts() {
        let p = pseudonymiser();
        let a = Identity::Account("sub-42".to_string());
        assert_eq!(p.caller(&a), p.caller(&a));
        assert_eq!(p.caller(&a).unwrap().len(), PSEUDONYM_HEX);
        assert_ne!(
            p.caller(&a),
            p.caller(&Identity::Account("sub-43".to_string()))
        );
    }

    #[test]
    fn visit_inputs_cannot_collide() {
        let p = pseudonymiser();
        // Concaténation nue : "a" ‖ "bc" == "ab" ‖ "c".
        assert_ne!(
            p.visit(&network("a"), Some("bc"), 0),
            p.visit(&network("ab"), Some("c"), 0)
        );
        // Séparateur littéral : "a" | "|b" == "a|" | "b".
        assert_ne!(
            p.visit(&network("a"), Some("|b"), 0),
            p.visit(&network("a|"), Some("b"), 0)
        );
        // La période entre bien dans le MAC.
        assert_ne!(
            p.visit(&network("203.0.113.7"), Some("curl/8"), 0),
            p.visit(&network("203.0.113.7"), Some("curl/8"), 1)
        );
    }
}
