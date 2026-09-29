CREATE TABLE IF NOT EXISTS sitemaps (
    filename     text PRIMARY KEY,
    content_type text NOT NULL,
    body         bytea NOT NULL,
    lastmod      date NOT NULL,
    updated_at   timestamptz NOT NULL DEFAULT now()
);
