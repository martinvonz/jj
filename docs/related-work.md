# Related work

Similar tools:

* [git-branchless](https://github.com/arxanas/git-branchless): Helps you use a
  branchless workflow in your Git repo. Supports anonymous branching, undo,
  and faster rebase (`git move`). Under heavy development and quickly gaining
  new features.
* [Sapling](https://sapling-scm.com/): A heavily modified fork of
  [Mercurial](https://www.mercurial-scm.org/) developed and used at Meta. It
  is compatible with Git, has undo functionality, and
  [a graphical interface](https://sapling-scm.com/docs/addons/isl).
  See [how it is different from Jujutsu](sapling-comparison.md).
* [GitUp](https://gitup.co/): A Mac-only GUI for Git. Like Jujutsu, supports
  undo and restoring the repo to an earlier snapshot. Backed by its
  [GitUpKit library](https://github.com/git-up/GitUp#gitupkit).
* [Gitless](https://gitless.com/): Another attempt at providing a simpler
  interface for Git. Like Jujutsu, does not have an "index"/"staging area"
  concept. Also doesn't move the working-copy changes between branches (which
  we do simply as a consequence of making the working copy a commit).
* [Pijul](https://pijul.org/): Architecturally quite different from Jujutsu,
  but its "first-class conflicts" feature seems quite similar to ours.
* [Breezy](https://www.breezy-vcs.org/): Another VCS that's similar in that it
  has multiple storage backends, including its own format as well as .git
  support.
* [Sturdy](https://getsturdy.com/): A Git backed GUI that eliminates local and
  remote as well as the idea of an "index"/"staging area".
