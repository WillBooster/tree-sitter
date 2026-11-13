pub const MIN_RUST_BINDING_MAJOR: u64 = 20;

#[must_use]
pub fn resolve_rust_binding_version(
    fork_version: Option<&str>,
    fallback_version: Option<&str>,
) -> String {
    fork_version
        .and_then(parse_compatible_version)
        .or_else(|| fallback_version.and_then(parse_compatible_version))
        .unwrap_or_else(|| MIN_RUST_BINDING_MAJOR.to_string())
}

#[must_use]
pub fn parse_compatible_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let core = normalized
        .split_once(['-', '+'])
        .map_or(normalized, |(head, _)| head);
    let parts: Vec<&str> = core.split('.').collect();
    if !(1..=3).contains(&parts.len()) {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    if major < MIN_RUST_BINDING_MAJOR {
        return None;
    }
    if parts[1..].iter().any(|part| part.parse::<u64>().is_err()) {
        return None;
    }
    if normalized
        .bytes()
        .any(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+')))
    {
        return None;
    }
    Some(normalized.to_string())
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
