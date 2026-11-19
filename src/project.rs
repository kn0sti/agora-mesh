//! Which local directory belongs to a room's project.
//!
//! A room names its project by the git remote URL. Each machine keeps a list of
//! local clones and starts the room session in the one whose `origin` matches.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// The repository `start` lies in: normalized origin URL and top-level directory.
pub fn locate(start: &Path) -> Option<(String, PathBuf)> {
    let top = git(start, &["rev-parse", "--show-toplevel"])?;
    let url = git(Path::new(&top), &["remote", "get-url", "origin"])?;
    Some((normalize(&url), PathBuf::from(top)))
}

/// Reduces the usual spellings of a remote URL to `host/owner/repo`, so that
/// `git@github.com:Owner/Repo.git` and `https://github.com/owner/repo` match.
pub fn normalize(url: &str) -> String {
    let mut s = url.trim().to_lowercase();
    let mut had_scheme = false;
    for scheme in ["ssh://", "https://", "http://", "git://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
            had_scheme = true;
            break;
        }
    }
    // Drop `user@` in front of the host.
    if let Some(at) = s.find('@') {
        if !s[..at].contains('/') {
            s = s[at + 1..].to_string();
        }
    }
    // scp-like `host:owner/repo`
    if !had_scheme {
        if let Some(colon) = s.find(':') {
            if !s[..colon].contains('/') {
                s.replace_range(colon..colon + 1, "/");
            }
        }
    }
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_string()
}

/// The registered local clone of `url`, if this machine has one.
pub fn resolve(projects: &[String], url: &str) -> Option<PathBuf> {
    let wanted = normalize(url);
    projects.iter().map(PathBuf::from).find(|p| locate(p).is_some_and(|(u, _)| u == wanted))
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn spellings_of_one_remote_match() {
        let want = "github.com/owner/repo";
        for url in [
            "git@github.com:Owner/Repo.git",
            "https://github.com/owner/repo",
            "https://github.com/owner/repo.git/",
            "ssh://git@github.com/owner/repo.git",
            "https://user@github.com/owner/repo",
            "  git://github.com/owner/repo.git\n",
        ] {
            assert_eq!(normalize(url), want, "{url}");
        }
        assert_ne!(normalize("git@github.com:owner/other.git"), want);
    }
}
