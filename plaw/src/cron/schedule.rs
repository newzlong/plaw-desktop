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
