-- the-one-mcp v0.16.0 Phase 2 — pgvector extension bootstrap.
--
-- By the time this runs, `pg_vector::PgVectorBackend::new` has
-- already called `preflight_vector_extension` which performs the
-- defensive pg_extension / pg_available_extensions check with
-- targeted Supabase/RDS/Cloud SQL/Azure/self-hosted error
-- messages. This migration is the point of last resort: if the
-- preflight succeeded, `CREATE EXTENSION IF NOT EXISTS vector`
-- here is a no-op; if the preflight was bypassed by a future
-- code path, this line will produce a clean error at apply time.
--
-- The schema is already created in `0000_migrations_table.sql`
-- (because the tracking table itself lives in `the_one`).

CREATE EXTENSION IF NOT EXISTS vector;
