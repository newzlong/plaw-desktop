//! LLM-as-Judge implementations — pairwise (mandatory dual-pass) and
//! multi-judge jury with cross-family enforcement.

pub mod builder;
pub mod client;
pub mod jury;
pub mod pairwise;

pub use builder::{api_key_env_var, build_from_spec};
pub use client::{
    AnthropicClient, JudgeClient, JudgeCompletion, JudgeFamily, MockJudgeClient,
    OpenAiCompatClient, DEFAULT_HTTP_TIMEOUT,
};
pub use jury::{Jury, JuryMemberRecord, JuryVerdict};
pub use pairwise::{
    compare_dual_pass, render_pairwise_prompt, PairwiseDecision, PairwiseRecord,
    DEFAULT_PAIRWISE_SYSTEM,
};
