-- Additive, applies with the server up: nothing reads the three columns yet.
--
-- `origine` names the correction that put the amount there and `cite` what the
-- text carries at that spot, both null when the value is as written.
-- `indemnites_acquises_cents` sums what earlier liquidations already paid on the
-- poste when the decision only repairs an aggravation: `montant_cents` then
-- holds complement plus acquired.

ALTER TABLE quantum_observation
  ADD COLUMN origine                   text,
  ADD COLUMN cite                      text,
  ADD COLUMN indemnites_acquises_cents bigint;
