//! Reading git/GitHub credentials the same way `git` itself would: from the
//! OS credential store (osxkeychain / wincred / manager-core / the plaintext
//! `store` helper, whichever `git config credential.helper` names), via
//! `gix`'s own implementation of the `git credential` helper protocol. No
//! system `git` binary is invoked.

/// A token for `https://github.com`, for the GitHub REST API and
/// `codeload.github.com` tarball downloads.
///
/// Checks `GITHUB_PAT`, then `GITHUB_TOKEN` (CI-friendly, and the precedence
/// `gh`/`usethis`/`gitcreds` use), then falls back to the system git
/// credential store. Returns `None`, not an error, when nothing is
/// configured -- unauthenticated access is still valid for public repos.
pub fn github_token() -> Option<String> {
    if let Ok(tok) = std::env::var("GITHUB_PAT") {
        if !tok.is_empty() {
            return Some(tok);
        }
    }
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        if !tok.is_empty() {
            return Some(tok);
        }
    }

    let action = gix::credentials::helper::Action::get_for_url("https://github.com");
    let outcome = gix::credentials::builtin(action).ok()??;
    let identity = outcome.identity;
    if !identity.password.is_empty() {
        Some(identity.password)
    } else if !identity.username.is_empty() {
        Some(identity.username)
    } else {
        None
    }
}

/// Wire the system git credential store into a `gix` clone, so a `git::`
/// source on a private host authenticates the same way `git clone` would.
pub fn configure_gix_clone(prepare: gix::clone::PrepareFetch) -> gix::clone::PrepareFetch {
    prepare.configure_connection(|conn| {
        conn.set_credentials(gix::credentials::builtin);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GITHUB_PAT` and `GITHUB_TOKEN` are process-global, so both cases live
    /// in one test rather than racing against a parallel test that also sets
    /// them (see the similar note in `pkgsource::git::tests`).
    #[test]
    fn env_vars_take_precedence_over_each_other_in_order() {
        std::env::remove_var("GITHUB_PAT");
        std::env::remove_var("GITHUB_TOKEN");

        std::env::set_var("GITHUB_TOKEN", "from-token-var");
        assert_eq!(github_token().as_deref(), Some("from-token-var"));

        std::env::set_var("GITHUB_PAT", "from-pat-var");
        assert_eq!(github_token().as_deref(), Some("from-pat-var"));

        std::env::remove_var("GITHUB_PAT");
        std::env::remove_var("GITHUB_TOKEN");
    }
}
