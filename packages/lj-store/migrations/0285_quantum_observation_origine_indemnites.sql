ALTER TABLE quantum_observation
  ADD COLUMN origine                   text,
  ADD COLUMN cite                      text,
  ADD COLUMN indemnites_acquises_cents bigint;
