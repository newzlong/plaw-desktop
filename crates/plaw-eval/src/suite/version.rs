//! Suite-schema version compatibility checks.
//!
//! Suite TOML files declare a semver `version` field. We accept the suite if
//! its major version matches what this build of plaw-eval can read.
//! Pre-1.0 suites (`0.x.y`) are accepted only when `compatible_major == 0`.

use anyhow::{anyhow, Result};

/// Parse a semver string into `(major, minor, patch)`. Pre-release / build
/// metadata are stripped. Returns an error on malformed input.
pub fn parse_semver(version: &str) -> Result<(u32, u32, u32)> {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 2 {
        return Err(anyhow!(
            "malformed version '{version}': need at least major.minor"
        ));
    }
    let major: u32 = parts[0]
        .parse()
        .map_err(|e| anyhow!("major component invalid in '{version}': {e}"))?;
    let minor: u32 = parts[1]
        .parse()
        .map_err(|e| anyhow!("minor component invalid in '{version}': {e}"))?;
    let patch: u32 = parts
        .get(2)
        .map_or(Ok(0), |s| s.parse())
        .map_err(|e| anyhow!("patch component invalid in '{version}': {e}"))?;
    Ok((major, minor, patch))
}

/// Returns Ok(()) iff `version`'s major component matches `compatible_major`.
pub fn ensure_compatible(version: &str, compatible_major: u32) -> Result<()> {
    let (major, _, _) = parse_semver(version)?;
    if major != compatible_major {
        return Err(anyhow!(
            "suite version {version} (major {major}) is not compatible with plaw-eval major {compatible_major}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_semver() {
        assert_eq!(parse_semver("1.2.3").unwrap(), (1, 2, 3));
        assert_eq!(parse_semver("0.5.0").unwrap(), (0, 5, 0));
        assert_eq!(parse_semver("2.0").unwrap(), (2, 0, 0));
    }

    #[test]
    fn strips_pre_release_and_build_metadata() {
        assert_eq!(parse_semver("1.2.3-rc.1").unwrap(), (1, 2, 3));
        assert_eq!(parse_semver("1.2.3+build42").unwrap(), (1, 2, 3));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_semver("not.a.version").is_err());
        assert!(parse_semver("1").is_err());
        assert!(parse_semver("").is_err());
    }

    #[test]
    fn ensure_compatible_accepts_matching_major() {
        assert!(ensure_compatible("1.0.0", 1).is_ok());
        assert!(ensure_compatible("1.99.5-beta", 1).is_ok());
    }

    #[test]
    fn ensure_compatible_rejects_mismatch() {
        assert!(ensure_compatible("2.0.0", 1).is_err());
        assert!(ensure_compatible("0.9.9", 1).is_err());
    }
}
