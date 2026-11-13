use semver::{BuildMetadata, Version};

pub const MIN_RUST_BINDING_MAJOR: u64 = 20;

#[must_use]
pub fn resolve_rust_binding_version(
    fork_version: Option<&str>,
    fallback_version: Option<&str>,
) -> String {
    fork_version
        .and_then(parse_compatible_version)
        .or_else(|| fallback_version.and_then(parse_compatible_version))
        .unwrap_or_else(|| Version::new(MIN_RUST_BINDING_MAJOR, 0, 0).to_string())
}

#[must_use]
pub fn parse_compatible_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let mut version = Version::parse(normalized).ok()?;
    if version.major < MIN_RUST_BINDING_MAJOR {
        return None;
    }
    version.build = BuildMetadata::EMPTY;
    Some(version.to_string())
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "called from crates/cli/build.rs and unit tests")
)]
#[must_use]
pub fn bump_version_from_log(base: (u64, u64, u64), log: &str) -> (u64, u64, u64) {
    let mut has_minor = false;
    let mut has_patch = false;
    for line in log.lines() {
        if line.starts_with("BREAKING CHANGE:")
            || line
                .split_once(':')
                .is_some_and(|(head, _)| head.ends_with('!'))
        {
            return (base.0 + 1, 0, 0);
        }
        has_minor |= line.starts_with("feat:") || line.starts_with("feat(");
        has_patch |= line.starts_with("fix:")
            || line.starts_with("fix(")
            || line.starts_with("perf:")
            || line.starts_with("perf(")
            || line.starts_with("revert:")
            || line.starts_with("revert(");
    }
    if has_minor {
        (base.0, base.1 + 1, 0)
    } else if has_patch {
        (base.0, base.1, base.2 + 1)
    } else {
        base
    }
}
