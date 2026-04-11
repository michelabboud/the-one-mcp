//! pgvector backend micro-benchmarks (v0.16.0 Phase 2).
//!
//! Measures throughput and latency of the new [`PgVectorBackend`]
//! against a live Postgres + pgvector instance. Sibling of
//! `crates/the-one-core/examples/production_hardening_bench.rs` —
//! the core bench stays SQLite-scoped (no cross-crate dep), and this
//! one carries the pgvector numbers for the Phase 2 commit body.
//!
//! # What gets measured
//!
//! 1. **Chunk upsert throughput** — batches of 50, 200, 1000 chunks.
//! 2. **Dense search latency** — p50 / p95 / p99 at 1k and 10k chunks.
//! 3. **HNSW ef_search sweep** — how latency vs recall moves as
//!    `ef_search` ranges from 10 (fast/lossy) to 200 (slow/precise).
//!
//! Prints markdown-ish output to stdout. No criterion dep; rolling
//! our own simple histograms keeps the workspace dep tree lean.
//!
//! # How to run
//!
//! 1. Start pgvector-enabled Postgres:
//!    ```bash
//!    docker run --rm -d --name pgvector-bench \
//!        -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=bench \
//!        -p 55432:5432 ankane/pgvector
//!    ```
//! 2. Run the bench (release mode is non-negotiable for meaningful
//!    throughput numbers):
//!    ```bash
//!    THE_ONE_VECTOR_TYPE=pgvector \
//!    THE_ONE_VECTOR_URL=postgres://postgres:pw@localhost:55432/bench \
//!    cargo run --release --example pgvector_bench \
//!        -p the-one-memory --features pg-vectors
//!    ```
//!
//! Skip condition: same as the integration test — if the env vars
//! aren't set to the expected values, the binary prints a "skipped"
//! banner and exits 0. No panic on missing DB.
//!
//! # Why the feature gate
//!
//! Without `pg-vectors` the file is a no-op `main` — the example
//! still exists but doesn't reference any pgvector symbols, so
//! `cargo check --examples` without the feature stays green.

#![cfg_attr(not(feature = "pg-vectors"), allow(unused_imports, dead_code))]

#[cfg(not(feature = "pg-vectors"))]
fn main() {
    eprintln!(
        "pgvector_bench: `pg-vectors` feature is not enabled. Rebuild with \
         `cargo run --release --example pgvector_bench -p the-one-memory \
         --features pg-vectors` to run this bench."
    );
}

#[cfg(feature = "pg-vectors")]
fn main() {
    runtime::main();
}

#[cfg(feature = "pg-vectors")]
mod runtime {
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use the_one_memory::embeddings::EmbeddingProvider;
    use the_one_memory::pg_vector::{PgVectorBackend, PgVectorConfig};
    use the_one_memory::vector_backend::{ChunkPayload, VectorBackend, VectorPoint};

    const DIM: usize = 1024;

    pub fn main() {
        let Some(url) = matching_env() else {
            println!("pgvector_bench: SKIPPED (THE_ONE_VECTOR_TYPE != 'pgvector' or URL unset)");
            return;
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async move {
            if let Err(e) = run(url).await {
                eprintln!("pgvector_bench: {e}");
                std::process::exit(1);
            }
        });
    }

    async fn run(url: String) -> Result<(), String> {
        println!("# pgvector_bench — v0.16.0 Phase 2");
        println!();

        // Reset schema for a clean baseline.
        reset_schema(&url).await?;

        let cfg = PgVectorConfig::default();
        let provider = DeterministicProvider;
        let backend = PgVectorBackend::new(&cfg, &url, "bench", &provider).await?;
        backend
            .ensure_collection(DIM)
            .await
            .map_err(|e| format!("ensure_collection: {e}"))?;

        bench_upsert_throughput(&backend).await?;
        bench_search_latency(&backend).await?;

        backend.close().await;
        Ok(())
    }

    // ── Upsert throughput ─────────────────────────────────────────

    async fn bench_upsert_throughput(backend: &PgVectorBackend) -> Result<(), String> {
        println!("## 1. Chunk upsert throughput");
        println!();
        println!("| batch | chunks/batch | batches | total chunks | total ms | chunks/sec |");
        println!("|-------|--------------|---------|--------------|----------|------------|");

        for batch_size in [50usize, 200, 1000] {
            let batches: usize = match batch_size {
                50 => 20,  // 1000 chunks total
                200 => 10, // 2000
                1000 => 5, // 5000
                _ => 1,
            };
            let total = batch_size * batches;

            let start = Instant::now();
            for b in 0..batches {
                let chunks: Vec<VectorPoint> = (0..batch_size)
                    .map(|i| {
                        let id = format!("b{b}-{i}");
                        VectorPoint {
                            vector: fake_vector(&id),
                            payload: ChunkPayload {
                                chunk_id: id.clone(),
                                source_path: format!("src/b{b}.md"),
                                heading: format!("H{i}"),
                                chunk_index: i,
                            },
                            content: Some(format!("content for {id}")),
                            id,
                        }
                    })
                    .collect();
                backend
                    .upsert_chunks(chunks)
                    .await
                    .map_err(|e| format!("upsert: {e}"))?;
            }
            let elapsed = start.elapsed();
            let throughput = total as f64 / elapsed.as_secs_f64();
            println!(
                "| {batch_size:>5} | {batch_size:>12} | {batches:>7} | {total:>12} | {:>7.1} | {throughput:>10.0} |",
                elapsed.as_millis() as f64
            );
        }

        println!();
        Ok(())
    }

    // ── Search latency ────────────────────────────────────────────

    async fn bench_search_latency(backend: &PgVectorBackend) -> Result<(), String> {
        println!("## 2. Dense search latency");
        println!();
        println!("Latency percentiles over 100 queries against an 8000-chunk corpus (from the upsert bench above).");
        println!();
        println!("| percentile | ms |");
        println!("|------------|----|");

        let queries = 100;
        let mut samples: Vec<Duration> = Vec::with_capacity(queries);
        for i in 0..queries {
            let q = fake_vector(&format!("query-{i}"));
            let start = Instant::now();
            let _hits = backend
                .search_chunks(q, 10, -1.0)
                .await
                .map_err(|e| format!("search: {e}"))?;
            samples.push(start.elapsed());
        }
        samples.sort();
        let pct = |p: f64| samples[(p * (samples.len() as f64 - 1.0)).round() as usize];
        println!("| p50 | {:>4.1} |", pct(0.50).as_millis() as f64);
        println!("| p95 | {:>4.1} |", pct(0.95).as_millis() as f64);
        println!("| p99 | {:>4.1} |", pct(0.99).as_millis() as f64);

        println!();
        println!("(HNSW ef_search sweep deferred until Decision D lands hybrid search.)");
        println!();
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────

    fn matching_env() -> Option<String> {
        if std::env::var("THE_ONE_VECTOR_TYPE").ok().as_deref() != Some("pgvector") {
            return None;
        }
        let url = std::env::var("THE_ONE_VECTOR_URL").ok()?;
        if url.trim().is_empty() {
            return None;
        }
        Some(url)
    }

    async fn reset_schema(url: &str) -> Result<(), String> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(|e| format!("reset_schema connect: {e}"))?;
        sqlx::query("DROP SCHEMA IF EXISTS the_one CASCADE")
            .execute(&pool)
            .await
            .map_err(|e| format!("reset_schema drop: {e}"))?;
        pool.close().await;
        Ok(())
    }

    struct DeterministicProvider;

    #[async_trait]
    impl EmbeddingProvider for DeterministicProvider {
        fn name(&self) -> &str {
            "bench-provider"
        }
        fn dimensions(&self) -> usize {
            DIM
        }
        async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|t| fake_vector(t)).collect())
        }
    }

    fn fake_vector(text: &str) -> Vec<f32> {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in text.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let mut v: Vec<f32> = Vec::with_capacity(DIM);
        let mut state = hash;
        for _ in 0..DIM {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let raw = ((state >> 32) as u32) as f32 / u32::MAX as f32;
            v.push(raw * 2.0 - 1.0);
        }
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}
