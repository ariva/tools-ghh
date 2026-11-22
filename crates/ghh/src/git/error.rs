//! Typed errors for everything the `git` layer can fail on.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),

    /// A `git` CLI step exited non-zero; `what` names the step.
    #[error("{what} failed: {stderr}")]
    Command { what: &'static str, stderr: String },
    /// A `git` CLI call failed and git's own message is the whole story.
    #[error("{0}")]
    Cli(String),
    #[error("Cannot read rev-list output")]
    NoRevListOutput,

    #[error("Commit not found: {0}")]
    CommitNotFound(String),
    #[error("Commit '{0}' not found in the current branch history")]
    NotInBranchHistory(String),
    #[error("invalid commit hash '{hash}': {source}")]
    InvalidHash { hash: String, source: git2::Error },
    #[error("commit object unavailable: {0}")]
    CommitUnavailable(#[source] git2::Error),
    #[error("HEAD is not a direct reference")]
    HeadNotDirect,
    #[error("Cannot get HEAD ref name")]
    HeadRefName,
    #[error("No working directory")]
    NoWorkdir,
    #[error(
        "Could not parse date '{0}'. Supported: YYYY-MM-DD, 'YYYY-MM-DD HH:MM', 'YYYY-MM-DD HH:MM:SS', or Unix timestamp"
    )]
    InvalidDate(String),

    #[error("No commits selected for squashing")]
    NothingToSquash,
    #[error("Base commit '{0}' not found")]
    SquashBaseNotFound(String),
    #[error("Cannot find squash commit '{hash}': {source}")]
    SquashCommitNotFound { hash: String, source: Box<Error> },
    #[error("Cannot squash the base commit into itself")]
    SquashBaseIntoItself,
    #[error(
        "Some selected commits are not in the current branch history (found {found}/{selected} selected commits)"
    )]
    SquashNotInHistory { found: usize, selected: usize },
    /// Carries the short hash.
    #[error("Commit '{0}' is a merge commit — squashing merge commits is not supported")]
    SquashMerge(String),
    #[error(
        "Commit '{hash}' ({subject}) lies between selected commits but is not marked — only a contiguous range is supported"
    )]
    SquashGap { hash: String, subject: String },

    #[error("Only commits above the newest merge on the current branch can be moved")]
    NotMovable,
    #[error("A rebase is already in progress")]
    RebaseInProgress,
    #[error("Working tree has uncommitted changes — commit or stash them before moving commits")]
    DirtyWorkTree,
    #[error("stash list changed outside the app — press r to reload")]
    StashListChanged,
}

impl Error {
    pub(crate) fn command(what: &'static str, stderr: &[u8]) -> Self {
        Error::Command {
            what,
            stderr: String::from_utf8_lossy(stderr).trim().to_string(),
        }
    }

    pub(crate) fn cli(stderr: &[u8]) -> Self {
        Error::Cli(String::from_utf8_lossy(stderr).trim().to_string())
    }
}
