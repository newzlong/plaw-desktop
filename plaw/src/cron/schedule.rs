use crate::cron::Schedule;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cron::Schedule as CronExprSchedule;
use std::str::FromStr;

pub fn next_run_for_schedule(schedule: &Schedule, from: DateTime<Utc>) -> Result<DateTime<Utc>> {
    match schedule {
        Schedule::Cron { expr, tz } => {
            let normalized = normalize_expression(expr)?;
            let cron = CronExprSchedule::from_str(&normalized)
                .with_context(|| format!("Invalid cron expression: {expr}"))?;

            if let Some(tz_name) = tz {
                let timezone = chrono_tz::Tz::from_str(tz_name)
                    .with_context(|| format!("Invalid IANA timezone: {tz_name}"))?;
                let localized_from = from.with_timezone(&timezone);
                let next_local = cron.after(&localized_from).next().ok_or_else(|| {
                    anyhow::anyhow!("No future occurrence for expression: {expr}")
                })?;
                Ok(next_local.with_timezone(&Utc))
            } else {
                cron.after(&from)
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("No future occurrence for expression: {expr}"))
            }
        }
        Schedule::At { at } => Ok(*at),
        Schedule::Every { every_ms } => {
            if *every_ms == 0 {
                anyhow::bail!("Invalid schedule: every_ms must be > 0");
            }
            let ms = i64::try_from(*every_ms).context("every_ms is too large")?;
            let delta = ChronoDuration::milliseconds(ms);
            from.checked_add_signed(delta)
                .ok_or_else(|| anyhow::anyhow!("every_ms overflowed DateTime"))
        }
    }
}

/// Compute the next run time AFTER a job has executed, given the tick it was
/// scheduled for (`scheduled_for`) and the current time (`now`).
///
/// For `Every` schedules this anchors the next fire to the SCHEDULED tick and
/// advances by whole intervals until strictly after `now`, so a periodic job
/// keeps a fixed rate instead of drifting forward by its own execution time
/// each cycle. Missed ticks (the job ran longer than the interval, or the
/// process was down across several) are skipped rather than fired back-to-back
/// — this mirrors the scheduler loop's own `MissedTickBehavior::Skip`.
///
/// `Cron` and `At` are anchored to `now`, exactly as [`next_run_for_schedule`]:
/// Cron then correctly skips any occurrence missed during execution, and `At`
/// is a one-shot whose stored time is returned unchanged.
pub fn next_run_after_run(
    schedule: &Schedule,
    scheduled_for: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    match schedule {
        Schedule::Every { every_ms } => next_every_fixed_rate(scheduled_for, *every_ms, now),
        _ => next_run_for_schedule(schedule, now),
    }
}

/// Smallest `anchor + k * every_ms` (k >= 1) strictly greater than `now`.
/// Fixed-rate with missed-tick skip; O(1) and overflow-checked.
fn next_every_fixed_rate(
    anchor: DateTime<Utc>,
    every_ms: u64,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    if every_ms == 0 {
        anyhow::bail!("Invalid schedule: every_ms must be > 0");
    }
    let ms = i64::try_from(every_ms).context("every_ms is too large")?;
    // Whole intervals elapsed since the anchor; +1 lands on the next tick
    // strictly after `now`. A fast job (elapsed ~0) yields k = 1, i.e. anchor +
    // one interval. If the anchor is somehow in the future, fall back to one.
    let elapsed_ms = now.signed_duration_since(anchor).num_milliseconds();
    let steps = if elapsed_ms < 0 {
        1
    } else {
        elapsed_ms / ms + 1
    };
    let advance_ms = ms
        .checked_mul(steps)
        .context("schedule interval advance overflowed")?;
    anchor
        .checked_add_signed(ChronoDuration::milliseconds(advance_ms))
        .ok_or_else(|| anyhow::anyhow!("every_ms overflowed DateTime"))
}

pub fn validate_schedule(schedule: &Schedule, now: DateTime<Utc>) -> Result<()> {
    match schedule {
        Schedule::Cron { expr, .. } => {
            let _ = normalize_expression(expr)?;
            let _ = next_run_for_schedule(schedule, now)?;
            Ok(())
        }
        Schedule::At { at } => {
            if *at <= now {
                anyhow::bail!("Invalid schedule: 'at' must be in the future");
            }
            Ok(())
        }
        Schedule::Every { every_ms } => {
            if *every_ms == 0 {
                anyhow::bail!("Invalid schedule: every_ms must be > 0");
            }
            Ok(())
        }
    }
}

/// Upper bound for a per-job shell timeout. A shell cron job that needs to run
/// longer than 24h is almost certainly a mistake and would pin a scheduler
/// worker under `[scheduler] max_concurrent`, so we reject it secure-by-default.
pub const MAX_SHELL_TIMEOUT_SECS: u64 = 86_400;

/// Validate an optional per-job shell timeout override.
///
/// `None` is always valid (means "use the built-in default"). `Some(0)` is
/// rejected because `tokio::time::timeout(Duration::ZERO, ..)` fires
/// immediately, which would make every such job time out instantly. Values
/// above [`MAX_SHELL_TIMEOUT_SECS`] are rejected as runaway-job protection.
pub fn validate_timeout_secs(timeout_secs: Option<u64>) -> Result<()> {
    let Some(secs) = timeout_secs else {
        return Ok(());
    };
    if secs == 0 {
        anyhow::bail!("timeout_secs must be greater than 0");
    }
    if secs > MAX_SHELL_TIMEOUT_SECS {
        anyhow::bail!("timeout_secs must be <= {MAX_SHELL_TIMEOUT_SECS} seconds (24h); got {secs}");
    }
    Ok(())
}

pub fn schedule_cron_expression(schedule: &Schedule) -> Option<String> {
    match schedule {
        Schedule::Cron { expr, .. } => Some(expr.clone()),
        _ => None,
    }
}

pub fn normalize_expression(expression: &str) -> Result<String> {
    let expression = expression.trim();
    let field_count = expression.split_whitespace().count();

    match field_count {
        // standard crontab syntax: minute hour day month weekday
        5 => Ok(format!("0 {expression}")),
        // crate-native syntax includes seconds (+ optional year)
        6 | 7 => Ok(expression.to_string()),
        _ => anyhow::bail!(
            "Invalid cron expression: {expression} (expected 5, 6, or 7 fields, got {field_count})"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn next_run_for_schedule_supports_every_and_at() {
        let now = Utc::now();
        let every = Schedule::Every { every_ms: 60_000 };
        let next = next_run_for_schedule(&every, now).unwrap();
        assert!(next > now);

        let at = now + ChronoDuration::minutes(10);
        let at_schedule = Schedule::At { at };
        let next_at = next_run_for_schedule(&at_schedule, now).unwrap();
        assert_eq!(next_at, at);
    }

    #[test]
    fn validate_timeout_secs_accepts_none_and_in_range() {
        assert!(validate_timeout_secs(None).is_ok());
        assert!(validate_timeout_secs(Some(1)).is_ok());
        assert!(validate_timeout_secs(Some(300)).is_ok());
        assert!(validate_timeout_secs(Some(MAX_SHELL_TIMEOUT_SECS)).is_ok());
    }

    #[test]
    fn validate_timeout_secs_rejects_zero_and_over_max() {
        let zero = validate_timeout_secs(Some(0)).unwrap_err().to_string();
        assert!(zero.contains("greater than 0"), "{zero}");

        let over = validate_timeout_secs(Some(MAX_SHELL_TIMEOUT_SECS + 1))
            .unwrap_err()
            .to_string();
        assert!(over.contains("must be <="), "{over}");
    }

    #[test]
    fn every_reschedule_is_fixed_rate_not_completion_anchored() {
        // A job scheduled for t=0 with a 60s interval that finishes 5s later
        // must next fire at t=60 (anchor + interval), NOT t=65 (completion +
        // interval). This is the drift the completion-anchored path accumulated.
        let scheduled_for = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let every = Schedule::Every { every_ms: 60_000 };
        let finished_at = scheduled_for + ChronoDuration::seconds(5);

        let next = next_run_after_run(&every, scheduled_for, finished_at).unwrap();
        assert_eq!(next, scheduled_for + ChronoDuration::seconds(60));
    }

    #[test]
    fn every_reschedule_does_not_accumulate_drift() {
        // Ten cycles of a 60s job that each take 7s must land exactly on the
        // minute, with zero accumulated drift.
        let mut scheduled_for = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let every = Schedule::Every { every_ms: 60_000 };
        for i in 1..=10 {
            let finished_at = scheduled_for + ChronoDuration::seconds(7);
            scheduled_for = next_run_after_run(&every, scheduled_for, finished_at).unwrap();
            assert_eq!(
                scheduled_for,
                Utc.with_ymd_and_hms(2026, 1, 1, 0, i, 0).unwrap(),
                "cycle {i} drifted"
            );
        }
    }

    #[test]
    fn every_reschedule_skips_missed_ticks_no_catchup_burst() {
        // A 60s job that took 150s (longer than the interval) must skip to the
        // next future tick rather than fire back-to-back for every missed one.
        let scheduled_for = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let every = Schedule::Every { every_ms: 60_000 };
        let finished_at = scheduled_for + ChronoDuration::seconds(150);

        let next = next_run_after_run(&every, scheduled_for, finished_at).unwrap();
        // Elapsed 150s -> two whole intervals passed (t=60, t=120 both <= now);
        // next strictly-future tick is t=180.
        assert_eq!(next, scheduled_for + ChronoDuration::seconds(180));
        assert!(next > finished_at);
    }

    #[test]
    fn next_run_after_run_delegates_cron_and_at_to_now() {
        // Cron/At must be unchanged: anchored to `now`, ignoring scheduled_for.
        let scheduled_for = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 2, 16, 0, 0, 0).unwrap();

        let at_time = now + ChronoDuration::hours(3);
        let at = Schedule::At { at: at_time };
        assert_eq!(
            next_run_after_run(&at, scheduled_for, now).unwrap(),
            at_time
        );

        let cron = Schedule::Cron {
            expr: "0 9 * * *".into(),
            tz: None,
        };
        assert_eq!(
            next_run_after_run(&cron, scheduled_for, now).unwrap(),
            next_run_for_schedule(&cron, now).unwrap(),
        );
    }

    #[test]
    fn next_run_for_schedule_supports_timezone() {
        let from = Utc.with_ymd_and_hms(2026, 2, 16, 0, 0, 0).unwrap();
        let schedule = Schedule::Cron {
            expr: "0 9 * * *".into(),
            tz: Some("America/Los_Angeles".into()),
        };

        let next = next_run_for_schedule(&schedule, from).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 2, 16, 17, 0, 0).unwrap());
    }
}
