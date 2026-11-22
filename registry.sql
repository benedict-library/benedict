-- Library registry (library.registry): one row per library, one library per information boundary.
-- Every library tool reads its settings here; nothing else holds a copy of these values.
-- Apply: psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f registry.sql   (idempotent)
-- One direction: the central library may read the others; never grant other libraries' roles access to this table.
CREATE SCHEMA IF NOT EXISTS library;
CREATE TABLE IF NOT EXISTS library.registry (
  key        text PRIMARY KEY,
  ord        int  NOT NULL,
  name       text NOT NULL,
  central    boolean NOT NULL DEFAULT false,
  room       text,
  search     text CHECK (search IN ('room', 'box')),
  maxd       real,
  golden     text,
  librarian  text,
  wiki_root  text,
  dirs       text,
  status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'closed')),
  updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE library.registry ADD COLUMN IF NOT EXISTS save_block jsonb;
COMMENT ON TABLE library.registry IS 'Library registry — per library: name, room, search kind (room/box), cross-library distance limit (maxd), golden set, librarian step, wiki root, save rules';
COMMENT ON COLUMN library.registry.save_block IS 'Filing-cabinet save rules {name: regex}; any match refuses the save (bin/save_query.py)';
ALTER TABLE library.registry ADD COLUMN IF NOT EXISTS options jsonb;
COMMENT ON COLUMN library.registry.options IS 'Other registry choices as JSON (reads, rooms, sections, terms, cards, index_block, clean, mail_reap, restart, journal_db, web_shelf, web_save_cmd, web_block); bin/registry_export.py writes them into the JSON registry';
