# Tutorial

This text assumes that the reader is familiar with Git.

## Preparation

If you haven't already, make sure you
[install and configure Jujutsu](../README.md#Installation).

## Cloning a Git repo

Let's start by cloning the Jujutsu Git repo using `jj`:
```shell script
# Note the "git" before "clone" (there is no support for cloning native jj
# repos yet)
$ jj git clone https://github.com/martinvonz/jj.git
Fetching into new repo in "<dir>/jj"
Working copy now at: 265ecf5cab2d
Added 98 files, modified 0 files, removed 0 files
$ cd jj
```

Running `jj st` (short for`jj status`) now yields something like this:
```shell script
$ jj st
Parent commit: 723ebb380971 cleanup: restructure escaped newlines to make new rustc happy
Working copy : 265ecf5cab2d
The working copy is clean
```

We can see from the output above that our working copy has a commit ID
(`265ecf5cab2d` in the example).

Let's check out a particular commit, so we get more predicable output:
```shell script
$ jj co 080a9b37ff7e
Working copy now at: 608c179a60df
Added 7 files, modified 65 files, removed 21 files
$ jj st
Parent commit: 080a9b37ff7e cli: make `jj st` show parent commit before working copy commit
Working copy : 608c179a60df
The working copy is clean
```

You might have noticed that even though we asked to check out some commit
(`080a9b37ff7e`), our working copy commit ended being another commit
(`608c179a60df`). That is because `jj co` (short for `jj checkout`) creates a
new commit on top of the commit you asked it to check out. The new commit is for
the working copy changes. (There's some more nuance to this. We'll go through
that in a bit.)

## Creating our first change

Now let's say we want to edit the `README.md` file in the repo (i.e. what you're
reading right now) to say that Jujutsu is ready for use. Let's start by
describing the change (adding a commit message) so we don't forget what we're
working on:
```shell script
# This will bring up $EDITOR (or `pico` by default). Enter something like
# "Jujutsu is ready!" in the editor and then close it.
$ jj describe
Working copy now at: b2985d68096d Jujutsu is ready!
```

Now make the change in the README:
```shell script
# Adjust as necessary for compatibility with your flavor of `sed`
$ sed -i 's/not ready/ready/' README.md
$ jj st
Parent commit: 080a9b37ff7e cli: make `jj st` show parent commit before working copy commit
Working copy : 5f80190c44b9
Working copy changes:
M README.md
```
Note that you didn't have to tell Jujutsu to add the change like you would with
`git add`. You actually don't even need to tell it when you add new files or
remove existing files. However, the flip side of that is that you need to be
careful to keep your `.gitignore` up to date since there's currently no easy way
to say that you want an already added file to not be tracked
(https://github.com/martinvonz/jj/issues/14).

To see the diff, run `jj diff`:
```shell script
$ jj diff
Modified regular file README.md:
    ...
   4    4: ## Disclaimer
   5    5:
   6    6: This is not a Google product. It is an experimental version-control system
   7    7: (VCS). It is not ready for use. It was written by me, Martin von Zweigbergk
   8    8: (martinvonz@google.com). It is my personal hobby project. It does not indicate
   9    9: any commitment or direction from Google.
  10   10:
    ...
```
Jujutsu's diff format currently only has inline coloring of the diff (like
`git diff --color-words`), which makes the diff impossible to see in the
un-colorized output above (the "not" in "not ready" is red).

As you may have noticed, the working copy commit's ID changed both when we
edited the description and when we edited the README. However, the parent commit
stayed the same. Each change to the working copy commit amends the previous
version. So how do we tell Jujutsu that we are done amending the working copy
commit? The answer is that we need to "close" the commit. When we close a
commit, we indicate that we're done making changes to the commit. As described
earlier, when we check out a commit, a new working copy commit is created on
top. However, that is only true for closed commits. If the commit is open, then
that commit itself will be checked out instead.

So, let's say we're now done with this commit, so we close it:
```shell script
$ jj close
Working copy now at: 192b456b024b
$ jj st
Parent commit: fb563a4c6d26 Jujutsu is ready!
Working copy : 192b456b024b
The working copy is clean
```

Note that a commit ID printed in green indicates an open commit and blue
indicates a closed commit.

If we later realize that we want to make further changes, we can make them
in the working copy and then run `jj squash`. That command squashes the changes
from a given commit into its parent commit. Like most commands, it acts on the
working copy commit by default.

## The log command, "revsets", and aliases

You're probably familiar with `git log`. Jujutsu has very similar functionality
in its `jj log` command. It produces hundreds of lines of output, so let's pipe
its output into `head`:
```shell script
$ jj log | head
@ 192b456b024b f39aeb1a0200 martinvonz@google.com 2021-05-23 23:10:27.000 -07:00
|
o fb563a4c6d26 f63e76f175b9 martinvonz@google.com 2021-05-23 22:13:45.000 -07:00
| Jujutsu is ready!
o 080a9b37ff7e 6a91b4ba16c7 martinvonz@google.com 2021-05-23 22:08:37.000 -07:00 main
| cli: make `jj st` show parent commit before working copy commit
o ba8ff31e32fd 302257bdb7e5 martinvonz@google.com 2021-05-23 22:08:12.000 -07:00
| cli: make the working copy changes in `jj status` clearer
o dcfc888f50b3 7eddf8dfc70d martinvonz@google.com 2021-05-23 22:07:40.000 -07:00
| cli: remove "Done" message at end of git clone
```

The `@` indicates the working copy commit. The first hash on a line is the
commit ID. The second hash is a "change ID", which is an ID that follows the
commit as it's rewritten (similar to Gerrit's Change-Id). You can give either
hash to commands that take revisions as arguments. We will generally prefer
change ids because they stay the same when the commit is rewritten.

By default, `jj log` lists all revisions (commits) in the repo that have not
been rewritten (roughly speaking). We can use the `-r` flag to restrict which
revisions we want to list. The flag accepts a "revset", which is an expression
in a simple language for specifying revision. For example, `@` refers to the
working copy commit, `root` refers to the root commit, `branches()` refers to
all commits pointed to by branches. We can combine expression with `|` for
union, `&` for intersection and `-` for difference. For example:
```shell script
$ jj log -r '@ | root | branches()'
@ 192b456b024b f39aeb1a0200 martinvonz@google.com 2021-05-23 23:10:27.000 -07:00
:
o 080a9b37ff7e 6a91b4ba16c7 martinvonz@google.com 2021-05-23 22:08:37.000 -07:00 main
: cli: make `jj st` show parent commit before working copy commit
o 000000000000 000000000000  1970-01-01 00:00:00.000 +00:00
```

(The `000000000000` commit is a virtual commit that's called the "root commit".
It's the root commit of every repo. The `root` symbol in the revset matches it.)

There are also operators for getting the parents (`:foo`), children `foo:`,
ancestors (`,,foo`), descendants (`foo,,`), DAG range (`foo,,bar`, like
`git log --ancestry-path`), range (`foo,,,bar`, like Git's `foo..bar`). There
are also a few more functions, such as `heads(<set>)`, which filters out
revisions in the input set if they're ancestors of other revisions in the set.
Let's define an alias based on that by adding the following to `~/.jjconfig`:
```
[alias]
l = ["log", "-r", "(heads(remote_branches()),,,@),,"]
```

The alias lets us run `jj l` to see the commits we have created between public
heads (exclusive) and the working copy (inclusive), as well as their
descendants:
```shell script
$ jj l
@ 192b456b024b f39aeb1a0200 martinvonz@google.com 2021-05-23 23:10:27.000 -07:00
|
o fb563a4c6d26 f63e76f175b9 martinvonz@google.com 2021-05-23 22:13:45.000 -07:00
~ Jujutsu is ready!
``` 

## Conflicts

Now let's see how Jujutsu deals with merge conflicts. We'll start by making some
commits:
```shell script
# Check out the grandparent of the working copy
$ jj co ::@
Working copy now at: 9164f1d6a011
Added 0 files, modified 1 files, removed 0 files
$ echo a > file1; jj close -m A
Working copy now at: 5be91b2b5b69
$ echo b1 > file1; jj close -m B1
Working copy now at: a0331f1eeece
$ echo b2 > file1; jj close -m B2
Working copy now at: fd571967346e
$ echo c > file2; jj close -m C
Working copy now at: 4ae1e0587eef
$ jj l
@ 4ae1e0587eef 47684978bf4b martinvonz@google.com 2021-05-26 12:39:56.000 -07:00
|
o 1769bdaa8d6d 8e6178b84ffb martinvonz@google.com 2021-05-26 12:39:35.000 -07:00
| C
o de5690380f40 5548374c0794 martinvonz@google.com 2021-05-26 12:39:30.000 -07:00
| B2
o 47e336632333 ce619d39bd96 martinvonz@google.com 2021-05-26 12:39:20.000 -07:00
| B1
o 661432c51c08 cf49e6bec410 martinvonz@google.com 2021-05-26 12:39:12.000 -07:00
~ A
```

We now have a few commits, where A, B1, and B2 modify the same file, while C
modifies a different file. Let's now rebase B2 directly onto A:
```shell script
$ jj rebase -s 5548374c0794 -d cf49e6bec410
Rebased 3 commits
Working copy now at: 9195b6d2e8dc
Added 0 files, modified 1 files, removed 0 files
$ jj l
@ 9195b6d2e8dc 47684978bf4b martinvonz@google.com 2021-05-26 12:39:56.000 -07:00 conflict
|
o 66274d5a7d2d 8e6178b84ffb martinvonz@google.com 2021-05-26 12:39:35.000 -07:00  conflict
| C
o 0c305a9e6b27 5548374c0794 martinvonz@google.com 2021-05-26 12:39:30.000 -07:00  conflict
| B2
| o 47e336632333 ce619d39bd96 martinvonz@google.com 2021-05-26 12:39:20.000 -07:00
|/  B1
o 661432c51c08 cf49e6bec410 martinvonz@google.com 2021-05-26 12:39:12.000 -07:00
~ A
```

There are several things worth noting here. First, the `jj rebase` command said
"Rebased 3 commits". That's because we asked it to rebase commit B2 with the
`-s` option, which also rebases descendants (commit C and the working copy in
this case). Second, because B2 modified the same file (and word) as B1, rebasing
it resulted in conflicts, as the `jj l` output indicates. Third, the conflicts
did not prevent the rebase from completing successfully, nor did it prevent C
and the working copy from getting rebased on top.

Now let's resolve the conflict in B2. We'll do that by checking out B2, which
will create a new commit on top because B2 is closed. Once we've resolved the
conflict, we'll squash the conflict resolution into the conflicted B2. That
might look like this:
```shell script
$ jj co 5548374c0794  # Replace the hash by what you have for B2
Working copy now at: 619f58d8a988
Added 0 files, modified 1 files, removed 0 files
$ jj st
Parent commit: 5548374c0794 B2
Working copy : 619f58d8a988
The working copy is clean
There are unresolved conflicts at these paths:
file1
$ cat file1
<<<<<<<
-------
+++++++
-b1
+a
+++++++
b2
>>>>>>>
$ echo resolved > file1
$ jj squash
Rebased 1 descendant commits
Working copy now at: e659edc4a9fc
$ jj l
@ e659edc4a9fc 461f38324592 martinvonz@google.com 2021-05-26 12:53:08.000 -07:00
|
| o 69dbcf76642a 8e6178b84ffb martinvonz@google.com 2021-05-26 12:39:35.000 -07:00
|/  C
o 576d647acf36 5548374c0794 martinvonz@google.com 2021-05-26 12:39:30.000 -07:00
| B2
| o 47e336632333 ce619d39bd96 martinvonz@google.com 2021-05-26 12:39:20.000 -07:00
|/  B1
o 661432c51c08 cf49e6bec410 martinvonz@google.com 2021-05-26 12:39:12.000 -07:00
~ A
```

Note that commit C automatically got rebased on top of the resolved B2, and that
C is also resolved (since it modified only a different file).

By the way, if we want to get rid of B1 now, we can run `jj abandon
47e336632333`. That will hide the commit from the log output and will rebase any
descendants to its parent.

## The operation log

Jujutsu keeps a record of all changes you've made to the repo in what's called
the "operation log". Use the `jj op` (short for `jj operation`) family of
commands to interact with it. To list the operations, use `jj op log`:
```shell script
$ jj op log
@ 5bd384507342 martinvonz@<hostname> 2021-05-26 12:53:08.339 -07:00 - 2021-05-26 12:53:08.350 -07:00
| squash commit 41f0d2289b568bfcdcf35f73d4f70f3ab6696398
| args: jj squash
o 2fd266a8a2e0 martinvonz@<hostname> 2021-05-26 12:53:08.335 -07:00 - 2021-05-26 12:53:08.338 -07:00
| commit working copy
o 1e6dd15305a3 martinvonz@<hostname> 2021-05-26 12:52:39.374 -07:00 - 2021-05-26 12:52:39.382 -07:00
| check out commit 0c305a9e6b274bc09b2bca85635299dcfdc6811c
| args: jj co 0c305a9e6b27
o 401652a2f61e martinvonz@<hostname> 2021-05-26 12:44:51.872 -07:00 - 2021-05-26 12:44:51.882 -07:00
| rebase commit de5690380f40f3f7fc6b7d66d43a4f68ee606228 and descendants
| args: jj rebase -s de5690380f40 -d 661432c51c08
[many more lines]
```

The most useful command is `jj undo` (alias for `jj op undo`), which will undo
an operation. By default, it will undo the most recent operation. Let's try it:
```shell script
$ jj undo
Working copy now at: 41f0d2289b56
$ jj l
@ 41f0d2289b56 b1e3a4afde5e martinvonz@google.com 2021-05-26 12:52:39.000 -07:00
|
| o 66274d5a7d2d 8e6178b84ffb martinvonz@google.com 2021-05-26 12:39:35.000 -07:00  conflict
|/  C
o 0c305a9e6b27 5548374c0794 martinvonz@google.com 2021-05-26 12:39:30.000 -07:00  conflict
| B2
| o 47e336632333 ce619d39bd96 martinvonz@google.com 2021-05-26 12:39:20.000 -07:00
|/  B1
o 661432c51c08 cf49e6bec410 martinvonz@google.com 2021-05-26 12:39:12.000 -07:00
~ A
```
As you can perhaps see, that undid the `jj squash` invocation we used for
squashing the conflict resolution into commit B2 earlier. Notice that it also
updated the working copy.

You can also view the repo the way it looked after some earlier operation. For
example, if you want to see `jj l` output right after the `jj rebase` operation,
try `jj l --at-op=401652a2f61e` but use the hash from your own `jj op log`.

## Moving content changes between commits

You have already seen how `jj squash` can combine the changes from two commits
into one. There are several other commands for changing the contents of existing
commits. These commands assume that you have `meld` installed. If you prefer
`vimdiff`, add this to your `~/.jjconfig` file:
```
[ui]
diff-editor = "vimdiff"
```

We'll need some more complex content to test these commands, so let's create a
few more commits:
```shell script
$ jj co origin/main
Working copy now at: 61b0efa09dbe 
Added 0 files, modified 0 files, removed 1 files
$ printf 'a\nb\nc\n' > file; jj close -m abc
Working copy now at: f9147a088c0d 
$ printf 'A\nB\nc\n' > file; jj close -m ABC
Working copy now at: 9d97c5018b23 
$ printf 'A\nB\nC\nD\n' > file; jj close -m ABCD
Working copy now at: c5a985bc3f41 
$ jj l
@ c5a985bc3f41 3568f6e332d5 martinvonz@google.com 2021-05-26 14:36:46.000 -07:00 
| 
o 687009839bae 874f2d307594 martinvonz@google.com 2021-05-26 14:36:38.000 -07:00 
| ABCD
o ad9b1ce3b5d0 2bbc0c1eb382 martinvonz@google.com 2021-05-26 14:36:26.000 -07:00 
| ABC
o a355fb177b21 3680117711f5 martinvonz@google.com 2021-05-26 14:36:05.000 -07:00 
~ abc
```

We "forgot" to capitalize "c" in the second commit when we capitalized the other
letters. We then fixed that in the third commit when we also added "D". It would
be cleaner to move the capitalization of "c" into the second commit. We can do
that by running `jj squash -i` (short for `jj squash --interactive`) on the
third commit. Remember that `jj squash` moves all the changes from one commit
into its parent. `jj squash -i` moves only part of the changes into its parent.
Now try that:
```shell script
$ jj squash -i -r :@
Rebased 1 descendant commits
Working copy now at: 4b4c714b36aa 
```
That will bring up Meld with a diff of the changes in the "ABCD" commit. Modify
the right side of the diff to have the desired end state in "ABC" by removing
the "D" line. Then close Meld. If we look the diff of the second commit, we
now see that all three lines got capitalized:
```shell script
$ jj diff -r ::@
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
command is `jj edit`, which lets you edit the contents of a commit without
checking it out.
```shell script
$ jj edit -r ::@
Created 2423c134ea70 ABC
Rebased 2 descendant commits
Working copy now at: d31c52e8ca41 
Added 0 files, modified 1 files, removed 0 files
```
When Meld starts, edit the right side by e.g. adding something to the first
line. Then close Meld. You can now inspect the rewritten commit with
`jj diff -r ::@` again and you should see your addition to the first line.
Unlike `jj squash -i`, which left the content state of the commit unchanged,
`jj edit` (typically) results in a different state, which means that descendant
commits may have conflicts.

Other commands for rewriting contents of existing commits are `jj restore -i`,
`jj split`, `jj unsquash -i`. Now that you've seen how `jj squash -i` and
`jj edit` work, you can hopefully figure out how those work (with the help of
the instructions in the diff).
