//! Git, as far as workspace isolation needs it.
//!
//! PushOS does not commit, push, merge or resolve anything. Those are
//! decisions, and decisions belong to the operator or to an agent they are
//! watching. All that is needed here is to know whether a directory is a
//! repository, what working trees it has, and how to add one.
//!
//! Driven through the process port rather than a git library: the operator's
//! own `git` is the one that knows about their configuration, their hooks and
//! their credentials, and a second implementation would disagree with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::error::ErrorClass;
use pushos_domain::ports::{ProcessRunner, ProcessSpec, Repository, RepositoryError, Worktree};
use tracing::debug;

/// How long a git command may take.
///
/// Adding a working tree copies a checkout, which on a large repository is not
/// instant, but nor should it take minutes.
const BUDGET: Duration = Duration::from_secs(90);

/// The operator's own git.
#[derive(Debug)]
pub struct GitRepository {
    processes: Arc<dyn ProcessRunner>,
    program: String,
}

impl GitRepository {
    /// Builds an adapter over the process runner.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self {
            processes,
            program: "git".to_owned(),
        }
    }

    /// Uses a particular git rather than whichever is on the path.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// Runs a git command in a directory.
    async fn run(&self, root: &Path, args: &[&str]) -> Result<String, RepositoryError> {
        // `-C` rather than a working directory, so the command says in its own
        // arguments where it ran. That is what appears in a log.
        let mut full = vec!["-C".to_owned(), root.display().to_string()];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));

        let spec = ProcessSpec::new(&self.program, full).within(BUDGET);
        let outcome = self.processes.run(&spec).await.map_err(|error| {
            RepositoryError::backend(
                format!("could not run git in `{}`", root.display()),
                error.class(),
                error,
            )
        })?;

        if outcome.succeeded() {
            return Ok(outcome.stdout_tail);
        }

        Err(RepositoryError::backend(
            format!(
                "`git {}` failed in `{}`: {}",
                args.join(" "),
                root.display(),
                outcome.stderr_tail.trim()
            ),
            ErrorClass::ComponentFailure,
            std::io::Error::other(outcome.stderr_tail),
        ))
    }
}

#[async_trait]
impl Repository for GitRepository {
    async fn is_repository(&self, root: &Path) -> bool {
        self.run(root, &["rev-parse", "--git-dir"]).await.is_ok()
    }

    async fn worktrees(&self, root: &Path) -> Result<Vec<Worktree>, RepositoryError> {
        let listing = self.run(root, &["worktree", "list", "--porcelain"]).await?;
        let main = self.run(root, &["rev-parse", "--show-toplevel"]).await?;
        Ok(parse_worktrees(&listing, Path::new(main.trim())))
    }

    async fn add_worktree(
        &self,
        root: &Path,
        path: &Path,
        branch: &str,
    ) -> Result<Worktree, RepositoryError> {
        let target = path.display().to_string();
        let reference = format!("refs/heads/{branch}");
        // A branch left from a tree removed earlier still holds what that role
        // committed, so it is picked up again rather than refused.
        if self
            .run(root, &["rev-parse", "--verify", "--quiet", &reference])
            .await
            .is_ok()
        {
            self.run(root, &["worktree", "add", &target, branch])
                .await?;
        } else {
            self.run(root, &["worktree", "add", "-b", branch, &target])
                .await?;
        }

        debug!(root = %root.display(), path = %target, branch, "added a working tree");
        Ok(Worktree {
            path: path.to_path_buf(),
            branch: Some(branch.to_owned()),
            is_main: false,
        })
    }

    async fn remove_worktree(&self, root: &Path, tree: &Worktree) -> Result<bool, RepositoryError> {
        if tree.is_main {
            return Ok(false);
        }
        match self.uncommitted(&tree.path).await {
            Some(true) => return Ok(false),
            Some(false) => {
                // Without `--force`: git checks again, and refuses anything
                // that changed since.
                let target = tree.path.display().to_string();
                self.run(root, &["worktree", "remove", &target]).await?;
            }
            // Already deleted by hand; git only needs to forget it.
            None => {
                self.run(root, &["worktree", "prune"]).await?;
            }
        }
        if let Some(branch) = &tree.branch {
            // `-d`, never `-D`: refused unless everything on it is merged.
            let _ = self.run(root, &["branch", "-d", branch]).await;
        }
        debug!(root = %root.display(), path = %tree.path.display(), "removed a working tree");
        Ok(true)
    }
}

impl GitRepository {
    /// Whether a working tree holds anything uncommitted: changed files, or new
    /// ones git is not told to ignore. Build output that is ignored does not
    /// count; it is what removing a tree is meant to clear.
    ///
    /// `None` when the tree's directory is gone.
    async fn uncommitted(&self, tree: &Path) -> Option<bool> {
        if !tree.is_dir() {
            return None;
        }
        match self.run(tree, &["status", "--porcelain"]).await {
            Ok(status) => Some(!status.trim().is_empty()),
            // Unreadable is treated as holding work: nothing is removed on a
            // guess.
            Err(_) => Some(true),
        }
    }
}

/// Reads `git worktree list --porcelain`.
///
/// Blocks are separated by a blank line and start with `worktree <path>`. A
/// block without that line is skipped rather than guessed at, because the
/// output arrives as a tail and a repository with a great many working trees
/// could have its first block cut in half.
fn parse_worktrees(listing: &str, main: &Path) -> Vec<Worktree> {
    let mut found = Vec::new();

    for block in listing.split("\n\n") {
        let mut path: Option<PathBuf> = None;
        let mut branch = None;

        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(rest.trim()));
            } else if let Some(rest) = line.strip_prefix("branch ") {
                // `refs/heads/main` is how git says it; `main` is how a person
                // does.
                branch = Some(
                    rest.trim()
                        .strip_prefix("refs/heads/")
                        .unwrap_or(rest.trim())
                        .to_owned(),
                );
            }
        }

        if let Some(path) = path {
            // Compared by path rather than taken from the order, because the
            // order is only reliable when nothing was truncated.
            let is_main = path == main;
            found.push(Worktree {
                path,
                branch,
                is_main,
            });
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    const LISTING: &str = "\
worktree /Users/x/Projects/pushos
HEAD 0000000000000000000000000000000000000000
branch refs/heads/main

worktree /Users/x/.local/share/pushos/worktrees/pushos/term-abc
HEAD 1111111111111111111111111111111111111111
branch refs/heads/pushos/term-abc

worktree /Users/x/detached
HEAD 2222222222222222222222222222222222222222
detached
";

    #[test]
    fn every_working_tree_in_the_listing_is_read() {
        let trees = parse_worktrees(LISTING, Path::new("/Users/x/Projects/pushos"));
        assert_eq!(trees.len(), 3);
        assert_eq!(trees[0].branch.as_deref(), Some("main"));
        assert_eq!(trees[1].branch.as_deref(), Some("pushos/term-abc"));
        assert_eq!(trees[2].branch, None, "a detached tree is on no branch");
    }

    #[test]
    fn the_repository_itself_is_told_apart_from_the_trees_added_to_it() {
        let trees = parse_worktrees(LISTING, Path::new("/Users/x/Projects/pushos"));
        assert!(trees[0].is_main);
        assert!(!trees[1].is_main);
    }

    #[test]
    fn a_block_cut_in_half_by_truncation_is_skipped_rather_than_guessed_at() {
        // The output arrives as a tail, so the first block can be a fragment.
        let truncated = "ranch refs/heads/main\n\nworktree /Users/x/second\nbranch refs/heads/x\n";
        let trees = parse_worktrees(truncated, Path::new("/Users/x/first"));
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].path, PathBuf::from("/Users/x/second"));
    }

    #[test]
    fn an_empty_listing_yields_nothing_rather_than_a_phantom_tree() {
        assert!(parse_worktrees("", Path::new("/tmp")).is_empty());
    }

    #[tokio::test]
    async fn a_directory_that_is_not_a_repository_says_so() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(128);
        let git = GitRepository::new(Arc::new(processes));

        assert!(!git.is_repository(Path::new("/tmp")).await);
    }

    #[tokio::test]
    async fn git_is_told_where_to_run_in_its_own_arguments() {
        // So that what ran is what appears in a log, rather than depending on
        // a working directory nobody recorded.
        let processes = FakeProcesses::new();
        let git = GitRepository::new(Arc::new(processes.clone()));
        git.is_repository(Path::new("/tmp/project")).await;

        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].program, "git");
        assert_eq!(spawned[0].args[0], "-C");
        assert_eq!(spawned[0].args[1], "/tmp/project");
    }

    /// A fake git that answers whether a branch exists as it is told.
    fn git_where_branch(exists: bool) -> (GitRepository, FakeProcesses) {
        let processes = FakeProcesses::new();
        processes.reply_with(move |spec| {
            let checking = spec.args.iter().any(|arg| arg == "--verify");
            pushos_domain::ports::ProcessOutcome {
                exit_code: Some(i32::from(checking && !exists)),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
            }
        });
        (GitRepository::new(Arc::new(processes.clone())), processes)
    }

    #[tokio::test]
    async fn adding_a_tree_creates_the_branch_it_was_given() {
        let (git, processes) = git_where_branch(false);

        let tree = git
            .add_worktree(
                Path::new("/tmp/project"),
                Path::new("/tmp/trees/one"),
                "pushos/one",
            )
            .await
            .expect("the fake succeeds by default");

        assert_eq!(tree.branch.as_deref(), Some("pushos/one"));
        assert!(!tree.is_main);
        let args = &processes.spawned()[1].args;
        assert!(args.contains(&"-b".to_owned()), "{args:?}");
        assert!(args.contains(&"pushos/one".to_owned()), "{args:?}");
        assert!(args.contains(&"/tmp/trees/one".to_owned()), "{args:?}");
    }

    #[tokio::test]
    async fn a_branch_left_by_a_removed_tree_is_picked_up_again_rather_than_refused() {
        // Its tree was removed and its commits kept; asking git to create it
        // again would fail, and starting over would hide that work.
        let (git, processes) = git_where_branch(true);
        git.add_worktree(
            Path::new("/tmp/project"),
            Path::new("/tmp/trees/one"),
            "pushos/one",
        )
        .await
        .expect("added");

        let args = &processes.spawned()[1].args;
        assert!(!args.contains(&"-b".to_owned()), "{args:?}");
        assert_eq!(&args[args.len() - 2..], ["/tmp/trees/one", "pushos/one"]);
    }

    #[tokio::test]
    async fn a_tree_whose_directory_is_gone_is_only_forgotten() {
        let processes = FakeProcesses::new();
        let git = GitRepository::new(Arc::new(processes.clone()));
        let gone = Worktree {
            path: PathBuf::from("/definitely/not/a/tree"),
            branch: Some("pushos/one".to_owned()),
            is_main: false,
        };

        assert!(
            git.remove_worktree(Path::new("/tmp/project"), &gone)
                .await
                .expect("ok")
        );
        let spawned = processes.spawned();
        assert!(
            spawned[0].args.contains(&"prune".to_owned()),
            "{:?}",
            spawned[0].args
        );
        assert!(
            !spawned
                .iter()
                .any(|spec| spec.args.contains(&"-D".to_owned())),
            "a branch is never deleted by force"
        );
    }

    #[tokio::test]
    async fn the_repository_itself_is_never_removed() {
        let processes = FakeProcesses::new();
        let git = GitRepository::new(Arc::new(processes.clone()));
        let main = Worktree {
            path: PathBuf::from("/tmp/project"),
            branch: Some("main".to_owned()),
            is_main: true,
        };
        assert!(
            !git.remove_worktree(Path::new("/tmp/project"), &main)
                .await
                .expect("ok")
        );
        assert!(processes.spawned().is_empty());
    }

    /// Against a real git, in a repository made for the test.
    #[tokio::test]
    async fn a_real_tree_goes_with_its_build_output_and_one_with_uncommitted_work_stays() {
        let root = std::env::temp_dir().join(format!("pushos-worktrees-{}", std::process::id()));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).expect("writable");
        let sh = |script: &str| {
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .current_dir(&repository)
                .output()
                .expect("runs")
        };
        let made = sh(
            "git init -q && printf 'target/\\n' > .gitignore && echo a > a.txt \
             && git add . && git -c user.email=t@t -c user.name=t commit -qm init",
        );
        if !made.status.success() {
            eprintln!("git is not usable here; skipping");
            std::fs::remove_dir_all(&root).ok();
            return;
        }

        let git = GitRepository::new(Arc::new(pushos_macos::SystemProcessRunner::new()));
        let clean = git
            .add_worktree(&repository, &root.join("clean"), "pushos/clean")
            .await
            .expect("added");
        std::fs::create_dir_all(root.join("clean/target")).expect("writable");
        std::fs::write(root.join("clean/target/build"), vec![0_u8; 1024]).expect("writable");

        let working = git
            .add_worktree(&repository, &root.join("working"), "pushos/working")
            .await
            .expect("added");
        std::fs::write(root.join("working/a.txt"), "changed").expect("writable");

        assert!(git.remove_worktree(&repository, &clean).await.expect("ok"));
        assert!(
            !root.join("clean").exists(),
            "the tree and its build output went"
        );
        assert!(
            !git.remove_worktree(&repository, &working)
                .await
                .expect("ok")
        );
        assert!(
            root.join("working/a.txt").exists(),
            "uncommitted work stayed"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_failed_git_command_says_what_it_was_and_where() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(1);
        let git = GitRepository::new(Arc::new(processes));

        let error = git
            .add_worktree(Path::new("/tmp/project"), Path::new("/tmp/one"), "b")
            .await
            .expect_err("git failed");

        let message = error.to_string();
        assert!(message.contains("worktree add"), "{message}");
        assert!(message.contains("/tmp/project"), "{message}");
        assert_eq!(error.class(), ErrorClass::ComponentFailure);
    }
}
