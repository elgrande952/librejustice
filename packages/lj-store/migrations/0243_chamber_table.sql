CREATE TABLE chamber (
  jurisdiction_code TEXT NOT NULL REFERENCES jurisdiction(code),
  chamber_key       TEXT NOT NULL,
  label             TEXT NOT NULL,
  PRIMARY KEY (jurisdiction_code, chamber_key)
);
