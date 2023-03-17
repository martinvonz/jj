# Tutorial

This text assumes that the reader is familiar with Git.

## Preparation

If you haven't already, make sure you
[install and configure Jujutsu](../README.md#Installation).

## Cloning a Git repo

Let's start by cloning GitHub's Hello-World repo using `jj`:
```shell script
# Note the "git" before "clone" (there is no support for cloning native jj
# repos yet)
$ jj git clone https://github.com/octocat/Hello-World
Fetching into new repo in "/tmp/tmp.O1DWMiaKd4/Hello-World"
Working copy now at: d7439b06fbef (no description set)
Added 1 files, modified 0 files, removed 0 files
$ cd Hello-World
```

Running `jj st` (short for`jj status`) now yields something like this:
```shell script
$ jj st
Parent commit: 7fd1a60b01f9 Merge pull request #6 from Spaceghost/patch-1
Working copy : d7439b06fbef (no description set)
The working copy is clean
```

We can see from the output above that our working copy is a real commit with a
commit ID (`7fd1a60b01f9` in the example). When you make a change in the working
copy, the working-copy commit gets automatically amended by the next `jj`
command.

## Creating our first change

Now let's say we want to edit the `README` file in the repo to say "Goodbye"
instead of "Hello". Let's start by describing the change (adding a
commit message) so we don't forget what we're working on:
```shell script
# This will bring up $EDITOR (or `pico` by default). Enter something like
# "Say goodbye" in the editor and then save the file and close the editor.
$ jj describe
Working copy now at: e427edcfd0ba Say goodbye
```

Now make the change in the README:
```shell script
# Adjust as necessary for compatibility with your flavor of `sed`
$ sed -i 's/Hello/Goodbye/' README
$ jj st
Parent commit: 7fd1a60b01f9 Merge pull request #6 from Spaceghost/patch-1
Working copy : 5d39e19dac36 Say goodbye
Working copy changes:
M README
```
Note that you didn't have to tell Jujutsu to add the change like you would with
`git add`. You actually don't even need to tell it when you add new files or
remove existing files. To untrack a path, add it to your `.gitignore` and run
`jj untrack <path>`.

To see the diff, run `jj diff`:
```shell script
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
is for the working-copy changes. For familiarity for user coming from other
VCSs, there is also a `jj checkout/co` command, which is practically a synonym
for `jj new` (you can specify a destination for `jj new` as well).

So, let's say we're now done with this change, so we create a new change:
```shell script
$ jj new
Working copy now at: aef4df99ea11 (no description set)
$ jj st
Parent commit: 5d39e19dac36 Say goodbye
Working copy : aef4df99ea11 (no description set)
The working copy is clean
```

If we later realize that we want to make further changes, we can make them
in the working copy and then run `jj squash`. That command squashes the changes
from a given commit into its parent commit. Like most commands, it acts on the
working-copy commit by default. When run on the working-copy commit, it behaves
very similar to `git commit --amend`, and `jj amend` is in fact an alias for
`jj squash`.

Alternatively, we can use `jj edit <commit>` to resume editing a commit in the
working copy. Any further changes in the working copy will then amend the
commit. Whether you choose to checkout-and-squash or to edit typically depends
on how done you are with the change; if the change is almost done, it makes
sense to use `jj checkout` so you can easily review your adjustments with
`jj diff` before running `jj squash`. 

## The log command and "revsets"

You're probably familiar with `git log`. Jujutsu has very similar functionality
in its `jj log` command:
```shell script
$ jj log
@  mpqrykypylvy martinvonz@google.com 2023-02-12 15:00:22.000 -08:00 aef4df99ea11
│  (empty) (no description set)
●  kntqzsqtnspv martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19dac36
│  Say goodbye
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

The `@` indicates the working-copy commit. The first ID on a line
(e.g. "mpqrykypylvy" above) is the "change ID", which is an ID that follows the
commit as it's rewritten (similar to Gerrit's Change-Id). The second ID is the
commit ID, which changes when you rewrite the commit. You can give either ID
to commands that take revisions as arguments. We will generally prefer change
IDs because they stay the same when the commit is rewritten.

By default, `jj log` lists your local commits, with some remote commits added
for context.  The `~` indicates that the commit has parents that are not
included in the graph. We can use the `-r` flag to select a different set of
revisions to list. The flag accepts a ["revset"](revsets.md), which is an
expression in a simple language for specifying revisions. For example, `@`
refers to the working-copy commit, `root` refers to the root commit,
`branches()` refers to all commits pointed to by branches. We can combine
expressions with `|` for union, `&` for intersection and `~` for difference. For
example:
```shell script
$ jj log -r '@ | root | branches()'
@  mpqrykypylvy martinvonz@google.com 2023-02-12 15:00:22.000 -08:00 aef4df99ea11
╷  (empty) (no description set)
╷ ●  kowxouwzwxmv octocat@nowhere.com 2014-06-10 15:22:26.000 -07:00 test b3cbd5bbd7e8
╭─╯  Create CONTRIBUTING.md
│ ●  tpstlustrvsn support+octocat@github.com 2018-05-10 12:55:19.000 -05:00 octocat-patch-1 b1b3f9723831
├─╯  sentence case
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
╷  (empty) Merge pull request #6 from Spaceghost/patch-1
●  zzzzzzzzzzzz 1970-01-01 00:00:00.000 +00:00 000000000000
   (empty) (no description set)
```

The `000000000000` commit (change ID `zzzzzzzzzzzz`) is a virtual commit that's
called the "root commit". It's the root commit of every repo. The `root` symbol
in the revset matches it.

There are also operators for getting the parents (`foo-`), children (`foo+`),
ancestors (`:foo`), descendants (`foo:`), DAG range (`foo:bar`, like
`git log --ancestry-path`), range (`foo..bar`, same as Git's). There are also a
few more functions, such as `heads(<set>)`, which filters out revisions in the
input set if they're ancestors of other revisions in the set.

## Conflicts

Now let's see how Jujutsu deals with merge conflicts. We'll start by making some
commits:
```shell script
# Start creating a chain of commits off of the `master` branch
$ jj new master -m A; echo a > file1
Working copy now at: 00a2aeed556a A
Added 0 files, modified 1 files, removed 0 files
$ jj new -m B1; echo b1 > file1
Working copy now at: 967d9f9fd288 B1
$ jj new -m B2; echo b2 > file1
Working copy now at: 8ebeaffa332b B2
$ jj new -m C; echo c > file2
Working copy now at: 62a3c6d315cd C
$ jj log
@  qzvqqupxlkot martinvonz@google.com 2023-02-12 15:07:41.946 -08:00 2370ddf3fa39
│  C
●  puqltuttrvzp martinvonz@google.com 2023-02-12 15:07:33.000 -08:00 daa6ffd5a09a
│  B2
●  ovknlmrokpkl martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6bd0b4
│  B1
●  nuvyytnqlquo martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f097aa9
│  A
│ ●  kntqzsqtnspv martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19dac36
├─╯  Say goodbye
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

We now have a few commits, where A, B1, and B2 modify the same file, while C
modifies a different file. Let's now rebase B2 directly onto A:
```shell script
$ jj rebase -s puqltuttrvzp -d nuvyytnqlquo
Rebased 2 commits
Working copy now at: 1978b53430cd C
Added 0 files, modified 1 files, removed 0 files
$ jj log
@  qzvqqupxlkot martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 1978b53430cd conflict
│  C
●  puqltuttrvzp martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 f7fb5943ee41 conflict
│  B2
│ ●  ovknlmrokpkl martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6bd0b4
├─╯  B1
●  nuvyytnqlquo martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f097aa9
│  A
│ ●  kntqzsqtnspv martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19dac36
├─╯  Say goodbye
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

There are several things worth noting here. First, the `jj rebase` command said
"Rebased 2 commits". That's because we asked it to rebase commit B2 with the
`-s` option, which also rebases descendants (commit C in this case). Second,
because B2 modified the same file (and word) as B1, rebasing
it resulted in conflicts, as the `jj log` output indicates. Third, the conflicts
did not prevent the rebase from completing successfully, nor did it prevent C
from getting rebased on top.

Now let's resolve the conflict in B2. We'll do that by creating a new commit on
top of B2. Once we've resolved the conflict, we'll squash the conflict
resolution into the conflicted B2. That might look like this:
```shell script
$ jj new puqltuttrvzp  # Replace the ID by what you have for B2
Working copy now at: c7068d1c23fd (no description set)
Added 0 files, modified 0 files, removed 1 files
$ jj st
Parent commit: f7fb5943ee41 B2
Working copy : c7068d1c23fd (no description set)
The working copy is clean
There are unresolved conflicts at these paths:
file1    2-sided conflict
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
Working copy now at: e3c279cc2043 (no description set)
$ jj log
@  ntxxqymrlvxu martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 e3c279cc2043
│  (empty) (no description set)
│ ●  qzvqqupxlkot martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 b9da9d28b26b
├─╯  C
●  puqltuttrvzp martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 2c7a658e2586
│  B2
│ ●  ovknlmrokpkl martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6bd0b4
├─╯  B1
●  nuvyytnqlquo martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f097aa9
│  A
│ ●  kntqzsqtnspv martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19dac36
├─╯  Say goodbye
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

Note that commit C automatically got rebased on top of the resolved B2, and that
C is also resolved (since it modified only a different file).

By the way, if we want to get rid of B1 now, we can run `jj abandon
ovknlmrokpkl`. That will hide the commit from the log output and will rebase any
descendants to its parent.

## The operation log

Jujutsu keeps a record of all changes you've made to the repo in what's called
the "operation log". Use the `jj op` (short for `jj operation`) family of
commands to interact with it. To list the operations, use `jj op log`:
```shell script
$ jj op log
@  d3b77addea49 martinvonz@vonz.svl.corp.google.com 2023-02-12 19:34:09.549 -08:00 - 2023-02-12 19:34:09.552 -08:00
│  squash commit 63874fe6c4fba405ffc38b0dd926f03b715cf7ef
│  args: jj squash
●  6fc1873c1180 martinvonz@vonz.svl.corp.google.com 2023-02-12 19:34:09.548 -08:00 - 2023-02-12 19:34:09.549 -08:00
│  snapshot working copy
●  ed91f7bcc1fb martinvonz@vonz.svl.corp.google.com 2023-02-12 19:32:46.007 -08:00 - 2023-02-12 19:32:46.008 -08:00
│  new empty commit
│  args: jj new puqltuttrvzp
●  367400773f87 martinvonz@vonz.svl.corp.google.com 2023-02-12 15:08:33.917 -08:00 - 2023-02-12 15:08:33.920 -08:00
│  rebase commit daa6ffd5a09a8a7d09a65796194e69b7ed0a566d and descendants
│  args: jj rebase -s puqltuttrvzp -d nuvyytnqlquo
[many more lines]
```

The most useful command is `jj undo` (alias for `jj op undo`), which will undo
an operation. By default, it will undo the most recent operation. Let's try it:
```shell script
$ jj undo
Working copy now at: 63874fe6c4fb (no description set)
$ jj log
@  zxoosnnpvvpn martinvonz@google.com 2023-02-12 19:34:09.000 -08:00 63874fe6c4fb
│  (no description set)
│ ●  qzvqqupxlkot martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 1978b53430cd conflict
├─╯  C
●  puqltuttrvzp martinvonz@google.com 2023-02-12 15:08:33.000 -08:00 f7fb5943ee41 conflict
│  B2
│ ●  ovknlmrokpkl martinvonz@google.com 2023-02-12 15:07:24.000 -08:00 7d7c6e6bd0b4
├─╯  B1
●  nuvyytnqlquo martinvonz@google.com 2023-02-12 15:07:05.000 -08:00 5dda2f097aa9
│  A
│ ●  kntqzsqtnspv martinvonz@google.com 2023-02-12 14:56:59.000 -08:00 5d39e19dac36
├─╯  Say goodbye
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
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
commits. These commands assume that you have `meld` installed. If you prefer
`vimdiff`, add this to your `~/.jjconfig.toml` file:
```
[ui]
diff-editor = "vimdiff"
```

We'll need some more complex content to test these commands, so let's create a
few more commits:
```shell script
$ jj new master -m abc; printf 'a\nb\nc\n' > file
Working copy now at: f94e49cf2547 abc
Added 0 files, modified 0 files, removed 1 files
$ jj new -m ABC; printf 'A\nB\nc\n' > file
Working copy now at: 6f30cd1fb351 ABC
$ jj new -m ABCD; printf 'A\nB\nC\nD\n' > file
Working copy now at: a67491542e10 ABCD
$ jj log -r master:@
@  mrxqplykzpkw martinvonz@google.com 2023-02-12 19:38:21.000 -08:00 b98c607bf87f
│  ABCD
●  kwtuwqnmqyqp martinvonz@google.com 2023-02-12 19:38:12.000 -08:00 30aecc0871ea
│  ABC
●  ztqrpvnwqqnq martinvonz@google.com 2023-02-12 19:38:03.000 -08:00 510022615871
│  abc
●  orrkosyozysx octocat@nowhere.com 2012-03-06 15:06:50.000 -08:00 master 7fd1a60b01f9
│  (empty) Merge pull request #6 from Spaceghost/patch-1
~
```

We "forgot" to capitalize "c" in the second commit when we capitalized the other
letters. We then fixed that in the third commit when we also added "D". It would
be cleaner to move the capitalization of "c" into the second commit. We can do
that by running `jj squash -i` (short for `jj squash --interactive`) on the
third commit. Remember that `jj squash` moves all the changes from one commit
into its parent. `jj squash -i` moves only part of the changes into its parent.
Now try that:
```shell script
$ jj squash -i
Using default editor 'meld'; you can change this by setting ui.diff-editor
Working copy now at: 52a6c7fda1e3 ABCD
```
That will bring up Meld with a diff of the changes in the "ABCD" commit. Modify
the right side of the diff to have the desired end state in "ABC" by removing
the "D" line. Then save the changes and close Meld. If we look at the diff of
the second commit, we now see that all three lines got capitalized:
```shell script
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
```shell script
$ jj diffedit -r @-
Using default editor 'meld'; you can change this by setting ui.diff-editor
Created 70985eaa924f ABC
Rebased 1 descendant commits
Working copy now at: 1c72cd50525d ABCD
Added 0 files, modified 1 files, removed 0 files
```
When Meld starts, edit the right side by e.g. adding something to the first
line. Then save the changes and close Meld. You can now inspect the rewritten
commit with `jj diff -r @-` again and you should see your addition to the first
line. Unlike `jj squash -i`, which left the content state of the commit
unchanged, `jj diffedit` (typically) results in a different state, which means
that descendant commits may have conflicts.

Other commands for rewriting contents of existing commits are `jj split`, `jj
unsquash -i` and `jj move -i`. Now that you've seen how `jj squash -i` and `jj
diffedit` work, you can hopefully figure out how those work (with the help of
the instructions in the diff).
