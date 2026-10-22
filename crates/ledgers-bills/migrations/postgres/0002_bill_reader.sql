-- Which reader produced a bill's fields: `rules` (the PDF text layer),
-- `anthropic` or `minimax` (the model readers), or `unread-scan` (a scanned
-- PDF that no configured reader takes: it needs a photo or a typed entry).
-- NULL for hand-typed bills and for bills made before this column. The
-- source column keeps its three values; this names the reader inside `llm`.
ALTER TABLE bill ADD COLUMN reader TEXT CHECK (reader IN ('rules', 'anthropic', 'minimax', 'unread-scan'));
