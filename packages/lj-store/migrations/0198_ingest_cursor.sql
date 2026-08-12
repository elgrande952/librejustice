-- Curseur de reprise des jobs d'ingest longs.
--
-- Les sources DILA n'ont jamais eu besoin de ça : `sync_dila` télécharge ses
-- stocks et ses incréments sous `state_dir` et **le disque est le watermark**
-- — ce qui est déjà là n'est pas retéléchargé. Une source qu'on lit en flux
-- sans rien garder (le backfill BOAMP : ~18 Go traversés pour
-- ~2 Go de lignes normalisées) n'a pas ce repère. Sans curseur explicite, une
-- coupure au 137e mois sur 199 fait tout recommencer.
--
-- `position` en text et pas en date : le concept est « où en est ce job »,
-- et la forme dépend du job (un mois ici, un id ou un offset ailleurs). Une
-- colonne typée forcerait une table par forme de curseur.

CREATE TABLE IF NOT EXISTS ingest_cursor (
    job        text PRIMARY KEY,
    position   text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
