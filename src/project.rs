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

/// What git says about a package's branch in a clone, so that a report on a
/// package need not be taken on trust.
pub struct BranchFacts {
    /// Short hash and subject of the branch's last commit; none if the branch does not exist here.
    pub head: Option<String>,
    /// The branch is on origin with the same commit, as far as this clone knows.
    pub pushed: bool,
    /// Changed files that are not committed.
    pub uncommitted: Vec<String>,
    /// The last commits of the branch, newest first.
    pub commits: Vec<String>,
}

pub fn branch_facts(dir: &Path, branch: &str) -> BranchFacts {
    let local = git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]);
    let remote = git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{branch}")]);
    let lines = |out: Option<String>| out.map(|o| o.lines().map(|l| l.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    BranchFacts {
        head: local.as_ref().and_then(|_| git(dir, &["log", "-1", "--format=%h %s", branch])),
        pushed: local.is_some() && local == remote,
        uncommitted: lines(git(dir, &["status", "--short"])).into_iter().take(8).collect(),
        commits: if local.is_some() { lines(git(dir, &["log", "-4", "--format=%h %s", branch])) } else { vec![] },
    }
}

/// One line for a report that says its package is done.
pub fn check_line(who: &str, branch: &str, f: &BranchFacts) -> String {
    let branch_part = match (&f.head, f.pushed) {
        (None, _) => format!("there is no branch `{branch}` in its clone, so nothing was committed for this package"),
        (Some(head), true) => format!("`{branch}` is at {head} and pushed"),
        (Some(head), false) => format!("`{branch}` is at {head} and not pushed: it exists on that machine only"),
    };
    let rest = match f.uncommitted.len() {
        0 => String::new(),
        1 => "; 1 changed file in that clone is not committed".to_string(),
        n => format!("; {n} changed files in that clone are not committed"),
    };
    format!("Checked on {who}: {branch_part}{rest}.")
}

/// What the taker of a package is told about the state its first owner left it in.
pub fn handover_note(who: &str, branch: &str, f: &BranchFacts) -> String {
    let mut note = check_line(who, branch, f);
    if f.commits.len() > 1 {
        note.push_str(&format!(" Its last commits: {}.", f.commits.join("; ")));
    }
    if !f.uncommitted.is_empty() {
        note.push_str(&format!(" Not committed, and so not available to anyone else: {}.", f.uncommitted.join(", ")));
    }
    note
}

/// Fetches one branch from origin, giving up after 20 seconds and never asking for a password.
fn fetch(dir: &Path, branch: &str) {
    use std::process::Stdio;
    let child = Command::new("git").arg("-C").arg(dir).args(["fetch", "-q", "origin", branch])
        .env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = child else { return };
    for _ in 0..200 {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The line of a commit message that says what its author meant to do next.
fn next_line(body: &str) -> Option<String> {
    body.lines().rev().find_map(|l| l.trim().strip_prefix("Next:").map(|n| n.trim().to_string())).filter(|n| !n.is_empty())
}

/// What the taker of a package can fetch: the state of its branch on origin,
/// read on the taker's machine, so it does not depend on the peer that stopped.
pub fn origin_note(dir: &Path, branch: &str) -> String {
    fetch(dir, branch);
    let remote = format!("refs/remotes/origin/{branch}");
    if git(dir, &["rev-parse", "--verify", "--quiet", &remote]).is_none() {
        return format!("Fetched here: origin has no branch `{branch}`, so nothing of this package was pushed. Start it from the plan.");
    }
    let commits = git(dir, &["log", "-4", "--format=%h %s", &remote]).map(|o| o.lines().map(|l| l.trim().to_string()).collect::<Vec<_>>().join("; ")).unwrap_or_default();
    let next = match git(dir, &["log", "-1", "--format=%b", &remote]).as_deref().and_then(next_line) {
        Some(n) => format!(" Its last commit says what was to come next: {n}."),
        None => String::new(),
    };
    format!("Fetched here: origin has `{branch}` with these last commits: {commits}.{next} Go on from that; whatever else the first owner did is not there.")
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    #[test]
    fn the_next_line_of_a_commit_is_found() {
        assert_eq!(next_line("Did the parser.\n\nNext: the tests for it"), Some("the tests for it".to_string()));
        assert_eq!(next_line("Did the parser."), None);
        assert_eq!(next_line("Next:"), None);
    }

    #[test]
    fn a_report_says_what_git_says() {
        let none = BranchFacts { head: None, pushed: false, uncommitted: vec![], commits: vec![] };
        assert!(check_line("Mac", "agora/r/mac", &none).contains("there is no branch `agora/r/mac`"));
        let local = BranchFacts { head: Some("abc1234 Add it".into()), pushed: false, uncommitted: vec![" M a.rs".into()], commits: vec!["abc1234 Add it".into(), "def5678 Start".into()] };
        let line = check_line("Mac", "agora/r/mac", &local);
        assert!(line.contains("is at abc1234 Add it and not pushed") && line.contains("; 1 changed file in that clone is not committed."));
        let note = handover_note("Mac", "agora/r/mac", &local);
        assert!(note.contains("Its last commits: abc1234 Add it; def5678 Start.") && note.contains("not available to anyone else:  M a.rs."));
        let pushed = BranchFacts { head: Some("abc1234 Add it".into()), pushed: true, uncommitted: vec![], commits: vec!["abc1234 Add it".into()] };
        assert_eq!(check_line("Mac", "b", &pushed), "Checked on Mac: `b` is at abc1234 Add it and pushed.");
    }
}
