-- Librarian journal (library.journal): one row per librarian action — cycle, variable clean-up, mailbox reaping,
-- room restart. bin/librarian.py inserts here first when a registry row chooses journal_db, then copies the line into
-- the librarian room's `journal`. Rows are only appended.
-- Apply: psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f journal.sql   (idempotent)
CREATE SCHEMA IF NOT EXISTS library;
CREATE TABLE IF NOT EXISTS library.journal (
  seq    bigserial PRIMARY KEY,
  at     timestamptz NOT NULL DEFAULT now(),
  room   text NOT NULL,
  kind   text NOT NULL,
  target text,
  result text
);
CREATE INDEX IF NOT EXISTS journal_room_at ON library.journal (room, at);
CREATE OR REPLACE FUNCTION library.journal_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'library.journal only takes new rows';
END $$;
DROP TRIGGER IF EXISTS journal_append_only ON library.journal;
CREATE TRIGGER journal_append_only BEFORE UPDATE OR DELETE ON library.journal
  FOR EACH ROW EXECUTE FUNCTION library.journal_append_only();
COMMENT ON TABLE library.journal IS 'Librarian journal — per action: time, librarian room, kind, target, result (append only)';
