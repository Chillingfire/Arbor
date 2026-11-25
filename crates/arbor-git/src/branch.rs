use std::fmt;

use crate::command::GitError;

/// The prefix every Session branch carries.
pub(crate) const PREFIX: &str = "arbor/";

/// The longest feature slug, in bytes.
const MAX_SLUG: usize = 40;

/// The branch a Session works on: `arbor/<short-feature-slug>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBranch(String);

impl SessionBranch {
    /// Names the branch for a feature described in words, e.g. `Add OAuth login` becomes
    /// `arbor/add-oauth-login`. Only ASCII letters and digits are kept, and long descriptions
    /// are cut at a word boundary.
    pub fn for_feature(feature: &str) -> Result<Self, GitError> {
        let words: Vec<String> = feature
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let mut slug = String::new();
        for word in words {
            let separator = usize::from(!slug.is_empty());
            if slug.len() + separator + word.len() > MAX_SLUG {
                if slug.is_empty() {
                    slug = word[..MAX_SLUG].to_owned();
                }
                break;
            }
            if separator == 1 {
                slug.push('-');
            }
            slug.push_str(&word);
        }
        if slug.is_empty() {
            return Err(GitError::Refused(format!(
                "cannot name a branch after {feature:?}; describe it with letters or digits"
            )));
        }
        Ok(Self(format!("{PREFIX}{slug}")))
    }

    /// The full branch name, e.g. `arbor/add-oauth-login`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionBranch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `branch` is a Session branch, e.g. `arbor/add-oauth-login`.
pub fn is_session_branch(branch: &str) -> bool {
    branch.starts_with(PREFIX)
}
