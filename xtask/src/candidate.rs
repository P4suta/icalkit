// SPDX-FileCopyrightText: 2026 icalkit contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ci::{CandidateRevision, TrackedSource};

pub(super) fn inspect(
    workspace: &Path,
    requested: CandidateRevision,
    scratch: &Path,
) -> io::Result<Vec<String>> {
    const COMMIT_CHANGED: &str = "candidate commit differs from the actual checkout";
    let repository = Repository::new(workspace)?;
    let head = repository.head()?;
    if !requested.matches(head.revision) {
        return Ok(vec![COMMIT_CHANGED.to_owned()]);
    }
    let index = ScratchIndex::from_commit(scratch, &repository, &head.commit)?;
    let source = TrackedSource::from_differences(
        repository.difference(&head.commit, Comparison::WorkingFiles(&index.path))?,
        repository.difference(&head.commit, Comparison::Staged)?,
    );
    let current = repository.head()?;
    if requested.qualifies(current.revision, source) {
        return Ok(Vec::new());
    }
    Ok(vec![if requested.matches(current.revision) {
        "candidate tracked source differs from the requested commit".to_owned()
    } else {
        COMMIT_CHANGED.to_owned()
    }])
}

struct Head {
    revision: CandidateRevision,
    commit: String,
}

#[derive(Clone, Copy)]
enum Comparison<'a> {
    WorkingFiles(&'a Path),
    Staged,
}

struct Repository<'a> {
    workspace: &'a Path,
    local_environment: Vec<String>,
}

impl<'a> Repository<'a> {
    fn new(workspace: &'a Path) -> io::Result<Self> {
        let output = Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()?;
        if !output.status.success() || output.stdout.is_empty() {
            return Err(io::Error::other(
                "git could not enumerate its repository-local environment",
            ));
        }
        let variables = String::from_utf8(output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            workspace,
            local_environment: variables.lines().map(str::to_owned).collect(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command.current_dir(self.workspace);
        for variable in &self.local_environment {
            command.env_remove(variable);
        }
        command.env("GIT_OPTIONAL_LOCKS", "0").args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.ignorestat=false",
            "-c",
            "core.sparseCheckout=false",
        ]);
        command
    }

    fn head(&self) -> io::Result<Head> {
        let output = self
            .command()
            .args(["rev-parse", "--verify", "HEAD"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                "git could not identify the candidate checkout",
            ));
        }
        let commit = String::from_utf8(output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let commit = commit.trim().to_owned();
        let revision = CandidateRevision::parse(&commit)
            .ok_or_else(|| io::Error::other("git reported an invalid candidate commit"))?;
        Ok(Head { revision, commit })
    }

    fn difference(&self, commit: &str, comparison: Comparison<'_>) -> io::Result<bool> {
        let mut command = self.command();
        command.args(["diff", "--quiet", "--no-ext-diff", "--no-textconv"]);
        match comparison {
            Comparison::WorkingFiles(index) => {
                command.env("GIT_INDEX_FILE", index);
            },
            Comparison::Staged => {
                command.arg("--cached");
            },
        }
        let status = command.args([commit, "--"]).status()?;
        match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(io::Error::other(
                "git could not inspect tracked candidate source",
            )),
        }
    }
}

struct ScratchIndex {
    directory: PathBuf,
    path: PathBuf,
}

impl ScratchIndex {
    fn from_commit(parent: &Path, repository: &Repository<'_>, commit: &str) -> io::Result<Self> {
        let index = Self::create(parent)?;
        let status = repository
            .command()
            .env("GIT_INDEX_FILE", &index.path)
            .args(["read-tree", "--no-sparse-checkout", commit])
            .status()?;
        if !status.success() {
            return Err(io::Error::other("git could not read the candidate tree"));
        }
        Ok(index)
    }

    fn create(parent: &Path) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        for _ in 0..8 {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!(
                "icalkit-candidate-index-{}-{serial}",
                std::process::id()
            ));
            match create_private_directory(&directory) {
                Ok(()) => {
                    let path = directory.join("index");
                    return Ok(Self { directory, path });
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a temporary candidate index",
        ))
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("could not remove temporary candidate index: {error}");
        }
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}
