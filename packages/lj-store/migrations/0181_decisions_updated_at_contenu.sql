-- 0181 — `decisions.updated_at` reseme sur la date du document.
--
-- Le tampon portait la date de la dernière ÉCRITURE, pas celle du dernier
-- changement de contenu : deux bumps d'EXTRACT_VERSION le 03/08/2026 ont
-- réécrit 3 687 864 lignes sur 3 693 004, alors que les correctifs
-- concernés ne changeaient la sortie d'extraction que d'une fraction du
-- fonds. Le `lastmod` des sitemaps, qui dérive de ce tampon, annonçait donc
-- tout le fonds modifié le même jour : 94 sous-sitemaps sur 102 se
-- déclaraient frais chaque nuit, et 89 % du budget de crawl Googlebot
-- (97 requêtes sur 109) partait à relire des sitemaps au lieu de pages.
--
-- On reseme le tampon sur `date_lecture`, seule date de contenu disponible
-- pour le fonds existant. Les écritures ultérieures ne l'avancent plus que
-- sur changement réel : comparaison de tuples NULL-safe côté colonnes, ids
-- rendus par les quatre couches satellites (refs légales, liens, citations
-- de jurisprudence, acteurs).
--
-- Coût : HOT update, aucun index ne porte `updated_at` — les 32 Go d'index
-- de `decisions` ne sont pas touchés, seuls les 6 Go de heap sont réécrits.
-- Le prédicat écarte les lignes déjà au bon tampon, la migration est donc
-- idempotente et sans churn au re-jeu.
--
-- Les 8 décisions sans `date_lecture` gardent leur tampon : `GREATEST` le
-- retient, et la fenêtre de plausibilité tranche le reste.

UPDATE decisions
SET updated_at = date_lecture::timestamptz
WHERE date_lecture IS NOT NULL
  AND updated_at::date <> date_lecture;
