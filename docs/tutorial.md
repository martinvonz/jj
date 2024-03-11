# Tutorial

This text assumes that the reader is familiar with Git.

## Preparation

If you haven't already, make sure you
[install and configure Jujutsu](install-and-setup.md).

## Cloning a Git repo

Let's start by cloning GitHub's Hello-World repo using `jj`:

```shell
# Note the "git" before "clone" (there is no support for cloning native jj
# repos yet)
$ jj git clone https://github.com/octocat/Hello-World
Fetching into new repo in "/tmp/tmp.O1DWMiaKd4/Hello-World"
Working copy now at: kntqzsqt d7439b06 (empty) (no description set)
Parent commit      : orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
Added 1 files, modified 0 files, removed 0 files
$ cd Hello-World
```

Running `jj st` (short for `jj status`) now yields something like this:

```shell
$ jj st
The working copy is clean
Working copy : kntqzsqt d7439b06 (empty) (no description set)
Parent commit: orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
```

We can see from the output above that our working copy is a real commit with a
commit ID (`d7439b06` in the example). When you make a change in the working
copy, the working-copy commit gets automatically amended by the next `jj`
command.

## Creating our first change

Now let's say we want to edit the `README` file in the repo to say "Goodbye"
instead of "Hello". Let's start by describing the change (adding a
commit message) so we don't forget what we're working on:

```shell
# This will bring up $EDITOR (or `pico` or `Notepad` by default). Enter
# something like "Say goodbye" in the editor and then save the file and close
# the editor.
$ jj describe
Working copy now at: kntqzsqt e427edcf (empty) Say goodbye
Parent commit      : orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
```

Now make the change in the README:

```shell
# Adjust as necessary for compatibility with your flavor of `sed`
$ sed -i 's/Hello/Goodbye/' README
$ jj st
Working copy changes:
M README
Working copy : kntqzsqt 5d39e19d Say goodbye
Parent commit: orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
```

Note that you didn't have to tell Jujutsu to add the change like you would with
`git add`. You actually don't even need to tell it when you add new files or
remove existing files. To untrack a path, add it to your `.gitignore` and run
`jj untrack <path>`.

To see the diff, run `jj diff`:

```shell
$ jj diff --git  # Feel free to skip the `--git` flag
diff --git a/README b/README
index 980a0d5f19...1ce3f81130 100644
--- a/README
+++ b/README
@@ -1,1 +1,1 @@
-Hello World!
+Goodbye World!
```

Jujutsu's diff format currently defaults to inline coloring of the diff (like
`git diff --color-words`), so we used `--git` above to make the diff readable in
this tutorial.

As you may have noticed, the working-copy commit's ID changed both when we
edited the description and when we edited the README. However, the parent commit
stayed the same. Each change to the working-copy commit amends the previous
version. So how do we tell Jujutsu that we are done amending the current change
and want to start working on a new one? That is what `jj new` is for. That will
create a new commit on top of your current working-copy commit. The new commit
is for the working-copy changes.

So, let's say we're now done with this change, so we create a new change:

```shell
$ jj new
Working copy now at: mpqrykyp aef4df99 (empty) (no description set)
Parent commit      : kntqzsqt 5d39e19d Say goodbye
$ jj st
The working copy is clean
Working copy : mpqrykyp aef4df99 (empty) (no description set)
Parent commit: kntqzsqt 5d39e19d Say goodbye
```

If we later realize that we want to make further changes, we can make them
in the working copy and then run `jj squash`. That command squashes the changes
from a given commit into its parent commit. Like most commands, it acts on the
working-copy commit by default. When run on the working-copy commit, it behaves
very similar to `git commit --amend`, and `jj amend` is in fact an alias for
`jj squash`.

Alternatively, we can use `jj edit <commit>` to resume editing a commit in the
working copy. Any further changes in the working copy will then amend the
commit. Whether you choose to create a new change and squash, or to edit,
typically depends on how done you are with the change; if the change is almost
done, it makes sense to use `jj new` so you can easily review your adjustments
with `jj diff` before running `jj squash`. 

## The log command and "revsets"

You're probably familiar with `git log`. Jujutsu has very similar functionality
in its `jj log` command:

```shell
$ jj log
@  mpqrykyp martinvonz@google.com 2023-02-12 15:00:22.000 -08:00 aef4df99
│  (empty) (no description set)
◉  kntqzsqt martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19d
│  Say goodbye
│ ◉  tpstlust support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1@origin b1b3f972
├─╯  sentence case
│ ◉  kowxouwz octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test@origin b3cbd5bb
├─╯  Create CONTRIBUTING.md
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

The `@` indicates the working-copy commit. The first ID on a line
(e.g. "mpqrykyp" above) is the "change ID", which is an ID that follows the
commit as it's rewritten (similar to Gerrit's Change-Id). The second ID is the
commit ID, which changes when you rewrite the commit. You can give either ID
to commands that take revisions as arguments. We will generally prefer change
IDs because they stay the same when the commit is rewritten.

By default, `jj log` lists your local commits, with some remote commits added
for context.  The `~` indicates that the commit has parents that are not
included in the graph. We can use the `--revisions`/`-r` flag to select a
different set of revisions to list. The flag accepts a ["revset"](revsets.md),
which is an expression in a simple language for specifying revisions. For
example, `@` refers to the working-copy commit, `root()` refers to the root
commit, `branches()` refers to all commits pointed to by branches. We can
combine expressions with `|` for union, `&` for intersection and `~` for
difference. For example:

```shell
$ jj log -r '@ | root() | branches()'
@  mpqrykyp martinvonz@google.com 2023-02-12 15:00:22.000 -08:00 aef4df99
╷  (empty) (no description set)
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
╷  (empty) Merge pull request #6 from Spaceghost/patch-1
◉  zzzzzzzz root() 00000000
```

The `00000000` commit (change ID `zzzzzzzz`) is a virtual commit that's
called the "root commit". It's the root commit of every repo. The `root()`
function in the revset matches it.

There are also operators for getting the parents (`foo-`), children (`foo+`),
ancestors (`::foo`), descendants (`foo::`), DAG range (`foo::bar`, like
`git log --ancestry-path`), range (`foo..bar`, same as Git's). See
[the revset documentation](revsets.md) for all revset operators and functions.

## Conflicts

Now let's see how Jujutsu deals with merge conflicts. We'll start by making some
commits. We use `jj new` with the `--message`/`-m` option to set change
descriptions (commit messages) right away.

```shell
# Start creating a chain of commits off of the `master` branch
$ jj new master -m A; echo a > file1
Working copy now at: nuvyytnq 00a2aeed (empty) A
Parent commit      : orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
Added 0 files, modified 1 files, removed 0 files
$ jj new -m B1; echo b1 > file1
Working copy now at: ovknlmro 967d9f9f (empty) B1
Parent commit      : nuvyytnq 5dda2f09 A
$ jj new -m B2; echo b2 > file1
Working copy now at: puqltutt 8ebeaffa (empty) B2
Parent commit      : ovknlmro 7d7c6e6b B1
$ jj new -m C; echo c > file2
Working copy now at: qzvqqupx 62a3c6d3 (empty) C
Parent commit      : puqltutt daa6ffd5 B2
$ jj log
@  qzvqqupx martinvonz@google.com 2023-02-12 15:07:41.946 -08:00 2370ddf3
│  C
◉  puqltutt martinvonz@google.com 2023-02-12 15:07:33.000 -08:00 daa6ffd5
│  B2
◉  ovknlmro martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6b
│  B1
◉  nuvyytnq martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f09
│  A
│ ◉  kntqzsqt martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19d
├─╯  Say goodbye
│ ◉  tpstlust support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1@origin b1b3f972
├─╯  sentence case
│ ◉  kowxouwz octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test@origin b3cbd5bb
├─╯  Create CONTRIBUTING.md
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

We now have a few commits, where A, B1, and B2 modify the same file, while C
modifies a different file. Let's now rebase B2 directly onto A. We use the
`--source`/`-s` option on the change ID of B2, and `--destination`/`-d` option
on A.

```shell
$ jj rebase -s puqltutt -d nuvyytnq  # Replace the IDs by what you have for B2 and A
Rebased 2 commits
New conflicts appeared in these commits:
  qzvqqupx 1978b534 (conflict) C
  puqltutt f7fb5943 (conflict) B2
To resolve the conflicts, start by updating to the first one:
  jj new puqltuttzvly
Then use `jj resolve`, or edit the conflict markers in the file directly.
Once the conflicts are resolved, you may want inspect the result with `jj diff`.
Then run `jj squash` to move the resolution into the conflicted commit.
Working copy now at: qzvqqupx 1978b534 (conflict) C
Parent commit      : puqltutt f7fb5943 (conflict) B2
Added 0 files, modified 1 files, removed 0 files
$ jj log
@  qzvqqupx martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 1978b534 conflict
│  C
◉  puqltutt martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 f7fb5943 conflict
│  B2
│ ◉  ovknlmro martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6b
├─╯  B1
◉  nuvyytnq martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f09
│  A
│ ◉  kntqzsqt martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19d
├─╯  Say goodbye
│ ◉  tpstlust support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1@origin b1b3f972
├─╯  sentence case
│ ◉  kowxouwz octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test@origin b3cbd5bb
├─╯  Create CONTRIBUTING.md
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

There are several things worth noting here. First, the `jj rebase` command said
"Rebased 2 commits". That's because we asked it to rebase commit B2 with the
`-s` option, which also rebases descendants (commit C in this case). Second,
because B2 modified the same file (and word) as B1, rebasing
it resulted in conflicts, as the output indicates. Third, the conflicts
did not prevent the rebase from completing successfully, nor did it prevent C
from getting rebased on top.

Now let's resolve the conflict in B2. We'll do that by creating a new commit on
top of B2. Once we've resolved the conflict, we'll squash the conflict
resolution into the conflicted B2. That might look like this:

```shell
$ jj new puqltutt  # Replace the ID by what you have for B2
Working copy now at: zxoosnnp c7068d1c (conflict) (empty) (no description set)
Parent commit      : puqltutt f7fb5943 (conflict) B2
Added 0 files, modified 0 files, removed 1 files
$ jj st
The working copy is clean
There are unresolved conflicts at these paths:
file1    2-sided conflict
Working copy : zxoosnnp c7068d1c (conflict) (empty) (no description set)
Parent commit: puqltutt f7fb5943 (conflict) B2
$ cat file1
<<<<<<<
%%%%%%%
-b1
+a
+++++++
b2
>>>>>>>
$ echo resolved > file1
$ jj squash
Rebased 1 descendant commits
Existing conflicts were resolved or abandoned from these commits:
  qzvqqupx hidden 1978b534 (conflict) C
  puqltutt hidden f7fb5943 (conflict) B2
Working copy now at: ntxxqymr e3c279cc (empty) (no description set)
Parent commit      : puqltutt 2c7a658e B2
$ jj log
@  ntxxqymr martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 e3c279cc
│  (empty) (no description set)
│ ◉  qzvqqupx martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 b9da9d28
├─╯  C
◉  puqltutt martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 2c7a658e
│  B2
│ ◉  ovknlmro martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6b
├─╯  B1
◉  nuvyytnq martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f09
│  A
│ ◉  kntqzsqt martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19d
├─╯  Say goodbye
│ ◉  tpstlust support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1@origin b1b3f972
├─╯  sentence case
│ ◉  kowxouwz octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test@origin b3cbd5bb
├─╯  Create CONTRIBUTING.md
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

Note that commit C automatically got rebased on top of the resolved B2, and that
C is also resolved (since it modified only a different file).

By the way, if we want to get rid of B1 now, we can run `jj abandon
ovknlmro`. That will hide the commit from the log output and will rebase any
descendants to its parent.

## The operation log

Jujutsu keeps a record of all changes you've made to the repo in what's called
the "operation log". Use the `jj op` (short for `jj operation`) family of
commands to interact with it. To list the operations, use `jj op log`:

```shell
$ jj op log
@  d3b77addea49 martinvonz@vonz.svl.corp.google.com 3 minutes ago, lasted 3 milliseconds
│  squash commit 63874fe6c4fba405ffc38b0dd926f03b715cf7ef
│  args: jj squash
◉  6fc1873c1180 martinvonz@vonz.svl.corp.google.com 3 minutes ago, lasted 1 milliseconds
│  snapshot working copy
│  args: jj squash
◉  ed91f7bcc1fb martinvonz@vonz.svl.corp.google.com 6 minutes ago, lasted 1 milliseconds
│  new empty commit
│  args: jj new puqltutt
◉  367400773f87 martinvonz@vonz.svl.corp.google.com 12 minutes ago, lasted 3 milliseconds
│  rebase commit daa6ffd5a09a8a7d09a65796194e69b7ed0a566d and descendants
│  args: jj rebase -s puqltutt -d nuvyytnq
[many more lines]
```

The most useful command is `jj undo` (alias for `jj op undo`), which will undo
an operation. By default, it will undo the most recent operation. Let's try it:

```shell
$ jj undo
New conflicts appeared in these commits:
  qzvqqupx 1978b534 (conflict) C
  puqltutt f7fb5943 (conflict) B2
To resolve the conflicts, start by updating to the first one:
  jj new puqltuttzvly
Then use `jj resolve`, or edit the conflict markers in the file directly.
Once the conflicts are resolved, you may want inspect the result with `jj diff`.
Then run `jj squash` to move the resolution into the conflicted commit.
Working copy now at: zxoosnnp 63874fe6 (no description set)
Parent commit      : puqltutt f7fb5943 (conflict) B2
$ jj log
@  zxoosnnp martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 63874fe6
│  (no description set)
│ ◉  qzvqqupx martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 1978b534 conflict
├─╯  C
◉  puqltutt martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 f7fb5943 conflict
│  B2
│ ◉  ovknlmro martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6b
├─╯  B1
◉  nuvyytnq martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f09
│  A
│ ◉  kntqzsqt martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19d
├─╯  Say goodbye
│ ◉  tpstlust support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1@origin b1b3f972
├─╯  sentence case
│ ◉  kowxouwz octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test@origin b3cbd5bb
├─╯  Create CONTRIBUTING.md
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

As you can perhaps see, that undid the `jj squash` invocation we used for
squashing the conflict resolution into commit B2 earlier. Notice that it also
updated the working copy.

You can also view the repo the way it looked after some earlier operation. For
example, if you want to see `jj log` output right after the `jj rebase` operation,
try `jj log --at-op=367400773f87` but use the hash from your own `jj op log`.

## Moving content changes between commits

You have already seen how `jj squash` can combine the changes from two commits
into one. There are several other commands for changing the contents of existing
commits.

We'll need some more complex content to test these commands, so let's create a
few more commits:

```shell
$ jj new master -m abc; printf 'a\nb\nc\n' > file
Working copy now at: ztqrpvnw f94e49cf (empty) abc
Parent commit      : orrkosyo 7fd1a60b master | (empty) Merge pull request #6 from Spaceghost/patch-1
Added 0 files, modified 0 files, removed 1 files
$ jj new -m ABC; printf 'A\nB\nc\n' > file
Working copy now at: kwtuwqnm 6f30cd1f (empty) ABC
Parent commit      : ztqrpvnw 51002261 ab
$ jj new -m ABCD; printf 'A\nB\nC\nD\n' > file
Working copy now at: mrxqplyk a6749154 (empty) ABCD
Parent commit      : kwtuwqnm 30aecc08 ABC
$ jj log -r master::@
@  mrxqplyk martinvonz@google.com 2023-02-12 19:38:21.000 -08:00 b98c607b
│  ABCD
◉  kwtuwqnm martinvonz@google.com 2023-02-12 19:38:12.000 -08:00 30aecc08
│  ABC
◉  ztqrpvnw martinvonz@google.com 2023-02-12 19:38:03.000 -08:00 51002261
│  abc
◉  orrkosyo octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

We "forgot" to capitalize "c" in the second commit when we capitalized the other
letters. We then fixed that in the third commit when we also added "D". It would
be cleaner to move the capitalization of "c" into the second commit. We can do
that by running `jj squash` with the `--interactive`/`-i` option on the
third commit. Remember that `jj squash` moves all the changes from one commit
into its parent. `jj squash -i` moves only part of the changes into its parent.
Now try that:

```shell
$ jj squash -i
Using default editor ':builtin'; you can change this by setting ui.diff-editor
Working copy now at: mrxqplyk 52a6c7fd ABCD
Parent commit      : kwtuwqnm 643061ac ABC
```

That will bring up the built-in diff editor with a diff of the changes in
the "ABCD" commit. If you prefer another diff editor, you can configure
[`ui.diff-editor`](config.md#editing-diffs) instead. Modify the right side of
the diff to have the desired end state in "ABC" by removing the "D" line. Then
save the changes and close Meld. If we look at the diff of the second commit, we
now see that all three lines got capitalized:

```shell
$ jj diff -r @-
Modified regular file file:
   1    1: aA
   2    2: bB
   3    3: cC
```

The child change ("ABCD" in our case) will have the same content *state* after
the `jj squash` command. That means that you can move any changes you want into
the parent change, even if they touch the same word, and it won't cause any
conflicts.

Let's try one final command for changing the contents of an exiting commit. That
command is `jj diffedit`, which lets you edit the contents of a commit without
checking it out.

```shell
$ jj diffedit -r @-
Using default editor ':builtin'; you can change this by setting ui.diff-editor
Created kwtuwqnm 70985eaa (empty) ABC
Rebased 1 descendant commits
New conflicts appeared in these commits:
  mrxqplyk 1c72cd50 (conflict) ABCD
To resolve the conflicts, start by updating to it:
  jj new mrxqplykmyqv
Then use `jj resolve`, or edit the conflict markers in the file directly.
Once the conflicts are resolved, you may want inspect the result with `jj diff`.
Then run `jj squash` to move the resolution into the conflicted commit.
Working copy now at: mrxqplyk 1c72cd50 (conflict) ABCD
Parent commit      : kwtuwqnm 70985eaa (empty) ABC
Added 0 files, modified 1 files, removed 0 files
```

In the diff editor, edit the right side by e.g. adding something to the first
line. Press 'c' to save the changes and close it. You can now inspect the rewritten
commit with `jj diff -r @-` again and you should see your addition to the first
line. Unlike `jj squash -i`, which left the content state of the commit
unchanged, `jj diffedit` (typically) results in a different state, which means
that descendant commits may have conflicts.

Other commands for rewriting contents of existing commits are `jj split`, `jj
unsquash -i`. Now that you've seen how `jj squash -i` and `jj diffedit` work,
you can hopefully figure out how those work (with the help of the instructions
in the diff).
