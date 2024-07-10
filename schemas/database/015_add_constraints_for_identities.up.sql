CREATE UNIQUE INDEX idx_unique_insertion_leaf on identities(leaf_index) WHERE commitment != E'\\x0000000000000000000000000000000000000000000000000000000000000000';
CREATE UNIQUE INDEX idx_unique_deletion_leaf on identities(leaf_index) WHERE commitment = E'\\x0000000000000000000000000000000000000000000000000000000000000000';

-- Add the new 'prev_root' column
ALTER TABLE identities ADD COLUMN pre_root BYTEA;

-- This constraint ensures that we have consistent database and changes to the tre are done in a valid sequence.
CREATE OR REPLACE FUNCTION validate_pre_root() returns trigger as $$
    DECLARE
        last_id identities.id%type;
        last_root identities.root%type;
    BEGIN
        SELECT id, root
        INTO last_id, last_root
        FROM identities
        ORDER BY id DESC
        LIMIT 1;

        -- When last_id is NULL that means there are no records in identities table. The first prev_root can
        -- be a value not referencing previous root in database.
        IF last_id IS NULL THEN RETURN NEW;
        END IF;

        IF NEW.pre_root IS NULL THEN RAISE EXCEPTION 'Sent pre_root (%) can be null only for first record in table.', NEW.pre_root;
        END IF;

        IF (last_root != NEW.pre_root) THEN RAISE EXCEPTION 'Sent pre_root (%) is different than last root (%) in database.', NEW.pre_root, last_root;
        END IF;

        RETURN NEW;
    END;
$$ language plpgsql;

CREATE TRIGGER validate_pre_root_trigger BEFORE INSERT ON identities FOR EACH ROW EXECUTE PROCEDURE validate_pre_root();

-- Below function took around 10 minutes for 10 million records.
DO
$do$
DECLARE
    prev_root identities.pre_root%type := NULL;
    identity identities%rowtype;
BEGIN
    FOR identity IN SELECT * FROM identities ORDER BY id ASC
    LOOP
        IF identity.pre_root IS NULL THEN UPDATE identities SET pre_root = prev_root WHERE id = identity.id;
        END IF;
        prev_root = identity.root;
    END LOOP;
END
$do$;

CREATE UNIQUE INDEX idx_unique_pre_root on identities(pre_root) WHERE pre_root IS NOT NULL;
