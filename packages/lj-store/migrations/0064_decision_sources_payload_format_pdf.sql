ALTER TABLE decision_sources
  DROP CONSTRAINT decision_sources_payload_format_check;

ALTER TABLE decision_sources
  ADD CONSTRAINT decision_sources_payload_format_check
    CHECK (payload_format IN ('xml', 'json', 'dila-xml', 'html', 'pdf'));
