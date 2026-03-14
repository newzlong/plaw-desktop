//! Memory capsule tools — search and recall archived conversation context.
//!
//! - `capsule_search`: FTS5 keyword search over capsule metadata (lightweight)
//! - `capsule_recall`: Read a capsule's full content for detailed recall

use super::traits::{Tool, ToolResult};
use crate::memory::capsules::CapsuleStore;
use crate::memory::embeddings::EmbeddingProvider;
use crate::memory::vector;
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;

/// Maximum chars returned by capsule_recall to prevent context overflow.
const CAPSULE_RECALL_MAX_CHARS: usize = 8_000;

// ── CapsuleSearchTool ──────────────────────────────────────────────

pub struct CapsuleSearchTool {
    store: Arc<CapsuleStore>,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_weight: f32,
    keyword_weight: f32,
}

impl CapsuleSearchTool {
    pub fn new(store: Arc<CapsuleStore>) -> Self {
        Self {
            store,
            embedding: None,
            vector_weight: 0.7,
            keyword_weight: 0.3,
        }
    }

    pub fn with_embedding(
        mut self,
        provider: Arc<dyn EmbeddingProvider>,
        vector_weight: f32,
        keyword_weight: f32,
    ) -> Self {
        self.embedding = Some(provider);
        self.vector_weight = vector_weight;
        self.keyword_weight = keyword_weight;
        self
    }
}

#[async_trait]
impl Tool for CapsuleSearchTool {
    fn name(&self) -> &str {
        "capsule_search"
    }

    fn description(&self) -> &str {
        "Search memory capsules (archived conversation segments) by keywords. \
         Returns capsule summaries and IDs. Use capsule_recall to read full content of a specific capsule. \
         Memory capsules are created automatically when conversation context is compacted."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords or phrase to search for in capsule archives"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(5, |v| v as usize);

        // Hybrid search: if embedding provider is available, combine vector + FTS5
        let capsule_ids = if let Some(ref emb) = self.embedding {
            match self.hybrid_search(emb, query, limit).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("hybrid search failed, falling back to FTS5: {e}");
                    Vec::new() // fall through to FTS5
                }
            }
        } else {
            Vec::new()
        };

        // If hybrid search returned results, fetch capsule metadata by IDs
        if !capsule_ids.is_empty() {
            return self.format_results_by_ids(&capsule_ids);
        }

        // Fallback: pure FTS5 keyword search
        match self.store.search(query, limit) {
            Ok(capsules) if capsules.is_empty() => Ok(ToolResult {
                success: true,
                output: "No memory capsules found matching that query.".into(),
                error: None,
            }),
            Ok(capsules) => Ok(ToolResult {
                success: true,
                output: Self::format_capsule_results(&capsules),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Capsule search failed: {e}")),
            }),
        }
    }
}

impl CapsuleSearchTool {
    /// Hybrid search: embed the query, run both vector + FTS5, merge results.
    async fn hybrid_search(
        &self,
        emb: &Arc<dyn EmbeddingProvider>,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<String>> {
        // Embed the query
        let query_vec = emb.embed_one(query).await?;

        // Vector search
        let vector_results = self.store.vector_search(&query_vec, limit * 2)?;

        // FTS5 keyword search (with BM25 scores)
        let keyword_results = self.fts5_with_scores(query, limit * 2)?;

        // Hybrid merge
        let merged = vector::hybrid_merge(
            &vector_results,
            &keyword_results,
            self.vector_weight,
            self.keyword_weight,
            limit,
        );

        Ok(merged.into_iter().map(|r| r.id).collect())
    }

    /// FTS5 search returning (id, bm25_score) tuples for hybrid merge.
    fn fts5_with_scores(&self, query: &str, limit: usize) -> anyhow::Result<Vec<(String, f32)>> {
        // Reuse the store's search and assign positional scores (BM25 rank order)
        let capsules = self.store.search(query, limit)?;
        Ok(capsules
            .iter()
            .enumerate()
            .map(|(i, cap)| {
                // Approximate BM25 score from rank position (higher is better)
                let score = (limit as f32 - i as f32) / limit as f32;
                (cap.id.clone(), score)
            })
            .collect())
    }

    /// Fetch capsule metadata by IDs and format the output.
    fn format_results_by_ids(&self, ids: &[String]) -> anyhow::Result<ToolResult> {
        let mut capsules = Vec::new();
        for id in ids {
            if let Ok(Some(cap)) = self.store.get(id) {
                capsules.push(crate::memory::capsules::CapsuleMeta {
                    id: cap.id,
                    session_id: cap.session_id,
                    created_at: cap.created_at,
                    keywords: cap.keywords,
                    summary: cap.summary,
                    token_count: cap.token_count,
                    message_count: cap.message_count,
                });
            }
        }
        if capsules.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No memory capsules found matching that query.".into(),
                error: None,
            });
        }
        Ok(ToolResult {
            success: true,
            output: Self::format_capsule_results(&capsules),
            error: None,
        })
    }

    fn format_capsule_results(capsules: &[crate::memory::capsules::CapsuleMeta]) -> String {
        let mut output = format!("Found {} memory capsule(s):\n\n", capsules.len());
        for cap in capsules {
            let keywords = cap.keywords.join(", ");
            let _ = writeln!(output, "--- Capsule {} ---", cap.id);
            let _ = writeln!(output, "Session: {}", cap.session_id);
            let _ = writeln!(output, "Created: {}", cap.created_at);
            let _ = writeln!(output, "Keywords: {keywords}");
            let _ = writeln!(
                output,
                "Size: {} tokens, {} messages",
                cap.token_count, cap.message_count
            );
            let _ = writeln!(output, "Summary:\n{}\n", cap.summary);
        }
        output
    }
}

// ── CapsuleRecallTool ──────────────────────────────────────────────

pub struct CapsuleRecallTool {
    store: Arc<CapsuleStore>,
}

impl CapsuleRecallTool {
    pub fn new(store: Arc<CapsuleStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for CapsuleRecallTool {
    fn name(&self) -> &str {
        "capsule_recall"
    }

    fn description(&self) -> &str {
        "Read the full archived content of a specific memory capsule. \
         Use capsule_search first to find relevant capsule IDs, \
         then use this tool to retrieve the detailed conversation history."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "capsule_id": {
                    "type": "string",
                    "description": "The capsule ID returned by capsule_search"
                }
            },
            "required": ["capsule_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let capsule_id = args
            .get("capsule_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'capsule_id' parameter"))?;

        match self.store.get(capsule_id) {
            Ok(Some(capsule)) => {
                let mut output = format!(
                    "=== Memory Capsule: {} ===\n\
                     Session: {}\n\
                     Created: {}\n\
                     Keywords: {}\n\
                     Messages: {}\n\n\
                     --- Archived Conversation ---\n",
                    capsule.id,
                    capsule.session_id,
                    capsule.created_at,
                    capsule.keywords.join(", "),
                    capsule.message_count,
                );
                // Truncate content to prevent context overflow
                if capsule.content.len() > CAPSULE_RECALL_MAX_CHARS {
                    output.push_str(&capsule.content[..CAPSULE_RECALL_MAX_CHARS]);
                    output.push_str("\n\n[... content truncated ...]");
                } else {
                    output.push_str(&capsule.content);
                }
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Ok(None) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Capsule not found: {capsule_id}")),
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to read capsule: {e}")),
            }),
        }
    }
}
