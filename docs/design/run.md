# Introducing JJ run

Authors: [Philip Metzger](mailto:philipmetzger@bluewin.ch), [Martin von Zweigberk](mailto:martinvonz@google.com), [Danny Hooper](mailto:hooper@google.com), [Waleed Khan](mailto:me@waleedkhan.name)

Initial Version, 10.12.2022 (view full history [here](https://docs.google.com/document/d/14BiAoEEy_e-BRPHYpXRFjvHMfgYVKh-pKWzzTDi-v-g/edit))


**Summary:** This Document documents the design of a new `run` command for 
Jujutsu which will be used to seamlessly integrate with build systems, linters
and formatters. This is achieved by running a user-provided command or script 
across multiple revisions. For more details, read the 
[Use-Cases of jj run](#Use-Cases-of-jj-run).

## Preface

The goal of this Design Document is to specify the correct behavior of `jj run`.
The points we decide on here I (Philip Metzger) will try to implement. There 
exists some prior work in other DVCS:
* `git test`: part of [git-branchless]. Similar to this proposal for `jj run`. 
* `hg run`: Google's internal Mercurial extension. Similar to this proposal for
`jj run`.
Details not available. 
* `hg fix`: Google's open source Mercurial extension: [source code][fix-src]. A
more specialized approach to rewriting file content without full context of the
working directory. 
* `git rebase -x`: runs commands opportunistically as part of rebase. 
* `git bisect run`: run a command to determine which commit introduced a bug.

## Context and Scope

The initial need for some kind of command runner integrated in the VCS, surfaced
in a [github discussion][pre-commit]. In a [discussion on discord][hooks] about
the git-hook model, there was consensus about not repeating their mistakes.

For `jj run` there is prior art in Mercurial, git branchless and Google's 
internal Mercurial. Currently git-branchless `git test` and `hg fix` implement
some kind of command runner. The Google internal `hg run` works in 
conjunction with CitC (Clients in the Cloud) which allows it to lazily apply
the current command to any affected file. Currently no Jujutsu backend
(Git, Native) has a fancy virtual filesystem supporting it, so we 
can't apply this optimization. We could do the same once we have an 
implementation of the working copy based on a virtual file system. Until then, 
we have to run the commands in regular local-disk working copies. 

## Goals and Non-Goals

### Goals

* We should be able to apply the command to any revision, published or unpublished.
* We should be able to parallelize running the actual command, while preserving a
good console output.
* The run command should be able to work in any commit, the working-copy commit
itself or any other commit. 
* There should exist some way to signal hard failure. 
* The command should build enough infrastructure for `jj test`, `jj fix` and 
`jj format`.
* The main goal is to be good enough, as we can always expand the functionality 
in the future.

### Non-Goals

* While we should build a base for `jj test`, `jj format` and `jj fix`, we 
shouldn't mash their use-cases into `jj run`.
* The command shouldn't be too smart, as too many assumptions about workflows 
makes the command confusing for users. 
* The smart caching of outputs, as user input commands can be unpredictable.
makes the command confusing for users. 
* Avoid the smart caching of outputs, as user input commands can be 
unpredictable.
* Fine grained user facing configuration, as it's unwarranted complexity.
* A `fix` subcommand as it cuts too much design space.

## Use-Cases of jj run

**Linting and Formatting:**

- `jj run 'pre-commit run' -r $revset`
- `jj run 'cargo clippy' -r $revset`
- `jj run 'cargo +nightly fmt'`

**Large scale changes across repositories, local and remote:**

- `jj run 'sed /some/test/' -r 'mine() & ~remote_branches(exact:"origin")'`
- `jj run '$rewrite-tool' -r '$revset'`

**Build systems:**

- `jj run 'bazel build //some/target:somewhere'`
- `jj run 'ninja check-lld'`

Some of these use-cases should get a specialized command, as this allows 
further optimization. A command could be `jj format`, which runs a list of 
formatters over a subset of a file in a revision. Another command could be 
`jj fix`, which runs a command like `rustfmt --fix` or `cargo clippy --fix` over
a subset of a file in a revision.

## Design

### Base Design 

All the work will be done in the `.jj/` directory. This allows us to hide all 
complexity from the users, while preserving the user's current workspace.

We will copy the approach from git-branchless's `git test` of creating a 
temporary working copy for each parallel command. The working copies will be 
reused between `jj run` invocations. They will also be reused within `jj run` 
invocation if there are more commits to run on than there are parallel jobs.

We will leave ignored files in the temporary directory between runs. That 
enables incremental builds (e.g by letting cargo reuse its `target/` directory).
However, it also means that runs potentially become less reproducible. We will 
provide a flag for removing ignored files from the temporary working copies to
address that. 

Another problem with leaving ignored files in the temporary directories is that
they take up space. That is especially problematic in the case of cargo (the 
`target/` directory often takes up tens of GBs). The same flag for cleaning up
ignored files can be used to address that. We may want to also have a flag for 
cleaning up temporary working copies *after* running the command. 

An early version of the command will directly use [Treestate] to 
to manage the temporary working copies. That means that running `jj` inside the 
temporary working copies will not work . We can later extend that to use a full
[Workspace]. To prevent operations in the working copies from 
impacting the repo, we can use a separate [OpHeadsStore] for it.

### Modifying the Working Copy

Since the subprocesses will run in temporary working copies, they 
won't interfere with the user's working copy. The user can therefore continue
to work in it while `jj run` is running. 

We want subprocesses to be able to make changes to the repo by updating their
assigned working copy. Let's say the user runs `jj run` on just commits A and 
B, where B's parent is A. Any changes made on top of A would be squashed into 
A, forming A'. Similarly B' would be formed by squasing it into B. We can then
either do a normal rebase of B' onto A', or we can simply update its parent to
A'. The former is useful, e.g when the subprocess only makes a partial update
of the tree based on the parent commit. In addition to these two modes, we may 
want to have an option to ignore any changes made in the subprocess's working 
copy.

### Modifying the Repo

Once we give the subprocess access to a fork of the repo via separate 
[OpHeadsStore], it will be able to create new operations in its fork.
If the user runs `jj run -r foo` and the subprocess checks out another commit,
it's not clear what that should do. We should probably just verify that the 
working-copy commit's parents are unchanged after the subprocess returns. Any
operations created by the subprocess will be ignored. 

### Rewriting the revisions 

Like all commands, `jj run` will refuse to rewrite public/immutable commits.
For private/unpublished revisions, we either amend or reparent the changes, 
which are available as command options.

## Execution order/parallelism

It may be useful to execute commands in topological order. For example, 
commands with costs proportional to incremental changes, like build systems. 
There may also be other revelant heuristics, but topological order is an easy
and effective way to start. 

Parallel execution of commands on different commits may choose to schedule 
commits to still reduce incremental changes in the working copy used by each
execution slot/"thread". However, running the command on all commits 
concurrently should be possible if desired. 

Executing commands in topological order allows for more meaningful use of any 
potential features that stop execution "at the first failure". For example, 
when running tests on a chain of commits, it might be useful to proceed in 
topological/chronological order, and stop on the first failure, because it 
might imply that the remaining executions will be undesirable because they will
also fail.

## Dealing with failure

It will be useful to have multiple strategies to deal with failures on a single
or multiple revisions. The reason for these strategies is to allow customized
conflict handling. These strategies then can be exposed in the ui with a 
matching option.

**Continue:** If any subprocess fails, we will continue the work on child 
revisions. Notify the user on exit about the failed revisions. 

**Stop:** Signal a fatal failure and cancel any scheduled work that has not
yet started running, but let any already started subprocess finish. Notify the
user about the failed command and display the generated error from the 
subprocess. 

**Fatal:** Signal a fatal failure and immediately stop processing and kill any 
running processes. Notify the user that we failed to apply the command to the 
specific revision. 

We will leave any affected commit in its current state, if any subprocess fails.
This allows us provide a better user experience, as leaving revisions in an 
undesirable state, e.g partially formatted, may confuse users.

## Resource constraints

It will be useful to constrain the execution to prevent resource exhaustion. 
Relevant resources could include:
- CPU and memory available on the machine running the commands. `jj run` can
provide some simple mitigations like limiting parallelism to "number of CPUs" 
by default, and limiting parallelism by dividing "available memory" by some 
estimate or measurement of per-invocation memory use of the commands.
- External resources that are not immediately known to jj. For example, 
commands run in parallel may wish to limit the total number of connections
to a server. We might choose to defer any handling of this to the 
implementation of the command being invoked, instead of trying to 
communicate that information to jj.


## Command Options

The base command of any jj command should be usable. By default `jj run` works 
on the `@` the current working copy.
* --command, explicit name of the first argument
* -x, for git compatibility (may alias another command)
* -j, --jobs, the amount of parallelism to use
* -k, --keep-going, continue on failure (may alias another command)
* --show, display the diff for an affected revision
* --dry-run, do the command execution without doing any work, logging all 
intended files and arguments
* --rebase, rebase all parents on the consulitng diff (may alias another 
command)
* --reparent, change the parent of an effected revision to the new change 
(may alias another command)
* --clean, remove existing workspaces and remove the ignored files
* --readonly, ignore changes across multiple run invocations
* --error-strategy=`continue|stop|fatal`, see [Dealing with failure](#Dealing-with-failure)

### Integrating with other commands

`jj log`: No special handling needed
`jj diff`: No special handling needed
`jj st`: For now reprint the final output of `jj run`
`jj op log`: No special handling needed, but awaits further discussion in 
[#963][issue]
`jj undo/jj op undo`: No special handling needed


## Open Points

Should the command be working copy backend specific?  
How do we manage the Processes which the command will spawn?  
Configuration options, User and Repository Wide?

## Future possibilities

- We could rewrite the file in memory, which is a neat optimization  
- Exposing some internal state, to allow preciser resource constraints  
- Integration options for virtual filesystems, which allow them to cache the 
needed working copies.  
- A Jujutsu wide concept for a cached working copy, as they could be expensive
to materialize.  
- Customized failure messages, this maybe useful for bots, it could be similar 
to Bazel's `select(..., message = "arch not supported for $project")`.
- Make `jj run` asynchronous by spawning a `main` process, directly return to the
user and incrementally updating the output of `jj st`. 



[git-branchless]: https://github.com/arxanas/git-branchless
[issue]: https://github.com/martinvonz/jj/issues/963 
[fix-src]: https://repo.mercurial-scm.org/hg/file/tip/hgext/fix.py
[hooks]: https://discord.com/channels/968932220549103686/969829516539228222/1047958933161119795
[OpHeadsStore]: https://github.com/martinvonz/jj/blob/main/lib/src/op_heads_store.rs
[pre-commit]: https://github.com/martinvonz/jj/issues/405
[Treestate]: https://github.com/martinvonz/jj/blob/af85f552b676d66ed0e9ae0d401cd0c4ffbbeb21/lib/src/working_copy.rs#L117
[Workspace]: https://github.com/martinvonz/jj/blob/af85f552b676d66ed0e9ae0d401cd0c4ffbbeb21/lib/src/workspace.rs#L54
