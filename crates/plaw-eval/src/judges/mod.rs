//! LLM-as-Judge implementations — pairwise (mandatory dual-pass) and
//! multi-judge jury with cross-family enforcement.

pub mod client;
pub mod pairwise;
pub mod jury;

pub use client::{
    AnthropicClient, JudgeClient, JudgeCompletion, JudgeFamily, MockJudgeClient,
    OpenAiCompatClient, DEFAULT_HTTP_TIMEOUT,
};
pub use pairwise::{
    compare_dual_pass, render_pairwise_prompt, PairwiseDecision, PairwiseRecord,
    DEFAULT_PAIRWISE_SYSTEM,
};
pub use jury::{Jury, JuryMemberRecord, JuryVerdict};
