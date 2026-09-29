ALTER TABLE decision_full_text
  DROP CONSTRAINT decision_full_text_payload_format_check;

ALTER TABLE decision_full_text
  ADD CONSTRAINT decision_full_text_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html'));
