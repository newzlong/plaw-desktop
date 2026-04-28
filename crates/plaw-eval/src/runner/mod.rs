//! Runner — schedules cases, talks to plaw via WebSocket, caches judge calls.

pub mod aggregate;
pub mod cache;
pub mod executor;
pub mod plaw_client;

pub use aggregate::{aggregate, aggregate_in_memory, DEFAULT_AGGREGATE_ALPHA};
pub use cache::{cache_key, CacheStats, JudgeCache};
pub use executor::{execute, RunSummary, RunnerConfig, DEFAULT_CONCURRENCY};
pub use plaw_client::{PlawClient, PlawResponse, ToolCallEvent, ToolResultEvent, Usage, DEFAULT_TIMEOUT};
