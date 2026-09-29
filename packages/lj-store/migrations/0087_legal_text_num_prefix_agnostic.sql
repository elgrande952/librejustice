ALTER TABLE legal_text
    ADD COLUMN IF NOT EXISTS num_prefix_agnostic BOOLEAN NOT NULL DEFAULT false;
