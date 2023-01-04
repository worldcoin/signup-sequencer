ALTER TABLE logs ADD COLUMN leaf BYTEA;
ALTER TABLE logs ADD COLUMN root BYTEA;

-- UPDATE
--     logs
-- SET
--     leaf = decode(substring((raw::json->'data')::text from 4 for 64), 'hex'),
--     root = decode(substring((raw::json->'data')::text from 68 for 64), 'hex');