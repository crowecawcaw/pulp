// backend/src/collectors/github_filter.rs

#[derive(serde::Deserialize, Default)]
pub(crate) struct GitHubSettings {
    #[serde(default)]
    pub(crate) ignore_repos: Vec<String>,
    #[serde(default)]
    pub(crate) ignore_orgs: Vec<String>,
    #[serde(default)]
    pub(crate) ignore_authors: Vec<String>,
    #[serde(default)]
    pub(crate) only_repos: Vec<String>,
    #[serde(default = "default_state_filter")]
    pub(crate) state_filter: String,
}

pub(crate) fn default_state_filter() -> String {
    "open".to_string()
}

/// Case-insensitive glob match where `*` matches any sequence of characters
/// (including `/`). No special handling for `?` or `[...]`.
pub(crate) fn glob_match(pattern: &str, s: &str) -> bool {
    let pattern_lower = pattern.to_lowercase();
    let s_lower = s.to_lowercase();

    let parts: Vec<&str> = pattern_lower.split('*').collect();

    if parts.len() == 1 {
        // No wildcards — exact match
        return s_lower == pattern_lower;
    }

    // The string must start with the first piece (prefix)
    if !s_lower.starts_with(parts[0]) {
        return false;
    }

    // The string must end with the last piece (suffix)
    let last = parts[parts.len() - 1];
    if !last.is_empty() && !s_lower.ends_with(last) {
        return false;
    }

    // Walk through the middle pieces ensuring they appear in order
    let mut pos = parts[0].len();
    for piece in &parts[1..parts.len() - 1] {
        if let Some(found) = s_lower[pos..].find(piece) {
            pos += found + piece.len();
        } else {
            return false;
        }
    }

    // Ensure the suffix doesn't overlap with what we've already consumed
    if !last.is_empty() {
        let suffix_start = s_lower.len().saturating_sub(last.len());
        if suffix_start < pos {
            return false;
        }
    }

    true
}

/// Returns true when the item should be skipped (blocked by settings).
pub(crate) fn is_ignored(settings: &GitHubSettings, repo: &str, author: &str) -> bool {
    // 1. only_repos allowlist — if non-empty, repo must match at least one pattern
    if !settings.only_repos.is_empty() {
        let allowed = settings.only_repos.iter().any(|p| glob_match(p, repo));
        if !allowed {
            return true;
        }
    }

    // 2. ignore_repos glob patterns
    if settings.ignore_repos.iter().any(|p| glob_match(p, repo)) {
        return true;
    }

    // 3. ignore_orgs — match the owner portion of "owner/repo"
    let owner = repo.split('/').next().unwrap_or("");
    if settings
        .ignore_orgs
        .iter()
        .any(|org| org.to_lowercase() == owner.to_lowercase())
    {
        return true;
    }

    // 4. ignore_authors
    if settings
        .ignore_authors
        .iter()
        .any(|a| a.to_lowercase() == author.to_lowercase())
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── glob_match tests ──────────────────────────────────────────────────────

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("exact/repo", "exact/repo"));
        assert!(!glob_match("exact/repo", "exact/other"));
    }

    #[test]
    fn glob_wildcard_org() {
        assert!(glob_match("nimbus-labs/*", "nimbus-labs/my-repo"));
    }

    #[test]
    fn glob_wildcard_org_no_match() {
        assert!(!glob_match("nimbus-labs/*", "other/my-repo"));
    }

    #[test]
    fn glob_wildcard_suffix() {
        assert!(glob_match("*/my-repo", "any-org/my-repo"));
    }

    #[test]
    fn glob_wildcard_all() {
        assert!(glob_match("*", "anything/here"));
    }

    #[test]
    fn glob_case_insensitive() {
        assert!(glob_match("NIMBUS-LABS/*", "nimbus-labs/repo"));
    }

    #[test]
    fn glob_no_false_positive() {
        assert!(!glob_match("nimbus-labs/*", "notnimbus-labs/repo"));
    }

    // ── is_ignored tests ──────────────────────────────────────────────────────

    fn make_settings(
        ignore_repos: &[&str],
        ignore_orgs: &[&str],
        ignore_authors: &[&str],
        only_repos: &[&str],
    ) -> GitHubSettings {
        GitHubSettings {
            ignore_repos: ignore_repos.iter().map(|s| s.to_string()).collect(),
            ignore_orgs: ignore_orgs.iter().map(|s| s.to_string()).collect(),
            ignore_authors: ignore_authors.iter().map(|s| s.to_string()).collect(),
            only_repos: only_repos.iter().map(|s| s.to_string()).collect(),
            state_filter: "open".to_string(),
        }
    }

    #[test]
    fn not_ignored_with_empty_settings() {
        let s = GitHubSettings::default();
        assert!(!is_ignored(&s, "any/repo", "anyone"));
    }

    #[test]
    fn ignored_by_org() {
        let s = make_settings(&[], &["nimbus-labs"], &[], &[]);
        assert!(is_ignored(&s, "nimbus-labs/social-listening", "someone"));
        assert!(!is_ignored(&s, "other-org/repo", "someone"));
    }

    #[test]
    fn ignored_by_repo_glob() {
        let s = make_settings(&["bad-org/*"], &[], &[], &[]);
        assert!(is_ignored(&s, "bad-org/any-repo", "someone"));
        assert!(!is_ignored(&s, "good-org/repo", "someone"));
    }

    #[test]
    fn ignored_by_author() {
        let s = make_settings(&[], &[], &["spambot"], &[]);
        assert!(is_ignored(&s, "any/repo", "SpamBot"));
        assert!(!is_ignored(&s, "any/repo", "legit-user"));
    }

    #[test]
    fn only_repos_allowlist() {
        let s = make_settings(&[], &[], &[], &["allowed-org/*"]);
        assert!(is_ignored(&s, "blocked-org/repo", "anyone"));
        assert!(!is_ignored(&s, "allowed-org/something", "anyone"));
    }

    #[test]
    fn only_repos_allowlist_empty_means_all() {
        let s = make_settings(&[], &[], &[], &[]);
        assert!(!is_ignored(&s, "any/repo", "anyone"));
    }
}
