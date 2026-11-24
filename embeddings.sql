-- Vector table for optional hybrid search (bin/embed_sync.py writes it for a deployment's room search).
-- One row per document (doc_path = absolute path) or per 1,000-character piece ('<path>::c<n>').
-- Apply: psql "$LIBRARY_PG_DSN" -v ON_ERROR_STOP=1 -f embeddings.sql   (idempotent; needs the pgvector extension)
-- The dimension must match LIBRARY_EMBED_DIM (default 768).
CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE IF NOT EXISTS wiki_embeddings (
  id           bigserial PRIMARY KEY,
  doc_path     text UNIQUE,
  file_hash    text,
  front_matter jsonb,
  embedding    vector(768),
  created_at   timestamptz DEFAULT now(),
  model        text
);
ALTER TABLE wiki_embeddings ADD COLUMN IF NOT EXISTS model text;
CREATE INDEX IF NOT EXISTS wiki_emb_doc_hnsw ON wiki_embeddings USING hnsw (embedding vector_cosine_ops)
  WITH (ef_construction = 200) WHERE doc_path NOT LIKE '%::c%';
CREATE INDEX IF NOT EXISTS wiki_emb_chunk_hnsw ON wiki_embeddings USING hnsw (embedding vector_cosine_ops)
  WHERE doc_path LIKE '%::c%';
