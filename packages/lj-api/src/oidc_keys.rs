//! ES256 signing key of the OIDC `id_token`, and the JWKS that publishes it.
//!
//! One key material, in `LIBREJUSTICE_API_OIDC_SIGNING_KEY`: a P-256 private
//! key in PKCS#8 DER, base64 on one line. Both the signer and the public JWK
//! come out of it, so the published `x`/`y` cannot drift from what signs.

use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::{json, Value};

use crate::oauth::{base64_url_no_pad, sha256};

/// An `id_token` only ever travels back as `id_token_hint` on a
/// reauthorization, so it does not have to outlive a linking flow.
const TTL_SECONDS: i64 = 3_600;

pub struct OidcSigner {
    encoding: EncodingKey,
    kid: String,
    jwk: Value,
}

#[derive(Serialize)]
struct IdTokenClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    exp: i64,
    iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email_verified: Option<bool>,
}

impl OidcSigner {
    /// `None` when no key is configured, which is the local-dev case: the
    /// server then issues no `id_token` and says so in its discovery.
    pub fn new(base64_pkcs8: Option<&str>) -> Option<Self> {
        let der = match STANDARD.decode(base64_pkcs8?.trim()) {
            Ok(der) => der,
            Err(e) => {
                tracing::error!("OIDC signing key unreadable (base64): {e}");
                return None;
            }
        };
        let pair = match EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &der) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("OIDC signing key refused (PKCS#8 P-256): {e}");
                return None;
            }
        };

        // Uncompressed SEC1 point: 0x04, then the two 32-byte coordinates.
        let point = pair.public_key().as_ref().to_vec();
        if point.len() != 65 || point[0] != 0x04 {
            tracing::error!("OIDC signing key: unexpected public point");
            return None;
        }
        let kid: String = base64_url_no_pad(&sha256(&point))
            .chars()
            .take(16)
            .collect();

        Some(Self {
            encoding: EncodingKey::from_ec_der(&der),
            jwk: json!({
                "kty": "EC",
                "crv": "P-256",
                "use": "sig",
                "alg": "ES256",
                "kid": kid,
                "x": base64_url_no_pad(&point[1..33]),
                "y": base64_url_no_pad(&point[33..65]),
            }),
            kid,
        })
    }

    pub fn jwks(&self) -> Value {
        json!({ "keys": [self.jwk] })
    }

    /// `aud` is the OAuth client that asked for it and `iss` our public base:
    /// an ID token naming neither is verifiable by nobody.
    pub fn id_token(
        &self,
        issuer: &str,
        subject: &str,
        client_id: &str,
        email: Option<&str>,
        email_verified: bool,
    ) -> Option<String> {
        let now = chrono::Utc::now().timestamp();
        let claims = IdTokenClaims {
            iss: issuer,
            sub: subject,
            aud: client_id,
            exp: now + TTL_SECONDS,
            iat: now,
            email,
            email_verified: email.map(|_| email_verified),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        match jsonwebtoken::encode(&header, &claims, &self.encoding) {
            Ok(jwt) => Some(jwt),
            Err(e) => {
                tracing::error!("id_token signing failed: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::jwk::Jwk;
    use jsonwebtoken::{decode, DecodingKey, Validation};

    fn signataire() -> OidcSigner {
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .unwrap();
        OidcSigner::new(Some(&STANDARD.encode(pkcs8.as_ref()))).unwrap()
    }

    #[derive(serde::Deserialize)]
    struct Recu {
        sub: String,
        email: Option<String>,
        email_verified: Option<bool>,
    }

    #[test]
    fn le_jwks_publie_verifie_ce_que_la_cle_signe() {
        let signer = signataire();
        let jwt = signer
            .id_token(
                "https://librejustice.fr",
                "u-42",
                "lj_client",
                Some("a@b.fr"),
                true,
            )
            .unwrap();

        let jwk: Jwk = serde_json::from_value(signer.jwks()["keys"][0].clone()).unwrap();
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&["lj_client"]);
        validation.set_issuer(&["https://librejustice.fr"]);
        let recu =
            decode::<Recu>(&jwt, &DecodingKey::from_jwk(&jwk).unwrap(), &validation).unwrap();

        assert_eq!(recu.claims.sub, "u-42");
        assert_eq!(recu.claims.email.as_deref(), Some("a@b.fr"));
        assert_eq!(recu.claims.email_verified, Some(true));
        assert_eq!(recu.header.kid, Some(signer.kid.clone()));
    }

    #[test]
    fn un_compte_sans_adresse_ne_porte_aucun_claim_email() {
        let signer = signataire();
        let jwt = signer
            .id_token("https://librejustice.fr", "u-42", "lj_client", None, false)
            .unwrap();

        let jwk: Jwk = serde_json::from_value(signer.jwks()["keys"][0].clone()).unwrap();
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&["lj_client"]);
        validation.set_issuer(&["https://librejustice.fr"]);
        let recu =
            decode::<Recu>(&jwt, &DecodingKey::from_jwk(&jwk).unwrap(), &validation).unwrap();

        assert_eq!(recu.claims.email, None);
        assert_eq!(recu.claims.email_verified, None);
    }

    #[test]
    fn une_cle_absente_ou_informe_ne_donne_pas_de_signataire() {
        assert!(OidcSigner::new(None).is_none());
        assert!(OidcSigner::new(Some("pas du base64 !")).is_none());
        assert!(OidcSigner::new(Some(&STANDARD.encode("pas du pkcs8"))).is_none());
    }
}
