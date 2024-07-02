# Jujutsu from first principles (without Git)

This document describes what Jujutsu's core priniciples are and explores some
possibilities which a native backend could encompass.

## Preface

Why does Jujutsu exist and which problems does it solve? This document tries to
answer both of these questions while expanding on the design in a user-friendly
way.

At its core Jujutsu is [Version Control System][vcs] which scales to huge 
repositories at [Google scale][billion-lines]. Many design choices are 
influenced by the concurrent commits happening in Googles Monorepo, as there 
are always multiple people working on the same file(s) at the same time.

## Core Tenets

Jujutsu's core tenets are:

 * User-friendliness: Making the  working copy a commit is simpler. This is 
 how the project started.
 * The "repository", so the commit graph is the source of truth. The working 
 copy is just one way of editing commits.
 * All operations must be able to scale to Google-scale repos (lots of commits
 , lots of files): Laziness is important, must avoid accessing data 
 unnecessarily.
 * Having as few states as possible.
 * Make it incredibily hard to lose work in your repository.
 * Allow concurrent edits on any commit, pending or finished.
 * Make a "stacked diffs" workflow as easy as possible.
 * Git-interop: Git is everywhere. We need to have good interop to be adapted.
 * Pluggable storage: Must be easy to integrate with different commit storage,
 virtual file systems and more.

## Base design

The initial base design is to be a conceptually simpler Mercurial, as 
automatically snapshotting the working copy simplifies the UX of the 
command-line interface by a huge amount and avoids many bad states.

By also choosing to operate by default on the history of the repository (
just called the "the Graph" from now on) instead of files, all history 
modifying commands can be done at any point. This is a major improvement on 
other version control systems as they need to re-apply a single patch on each 
new ancestor before finishing the Graph rewrite. Since the Graph can be changed
at any point, the working copy cannot contain any state depending on it, thus 
we have the working-copy commit, which just is another commit from the Graph's
point of view. 


### Change-IDs and Changes

Since Jujutsu is oriented around a "stacked diffs" kind of workflow, which 
primarily work on individually versioned patch sets, some kind of container is 
needed, this is what a Change is. They are provided with a unique id to address
them easily. This mechanism is also customizable so a custom backend could add
a new scheme, which is a major win for tool integrations such as codereview. 
And since each change can be addressed individually it simplifies the 
commandline.


### Operation store

The operation store is a abstraction for synchronizing multiple clients to a 
common state which allows Jujutsu to seamlessly work across multiple 
workstations and laptops. This may show up in the native backend in the future
but there are no guarantees. 


[billion-lines]: https://www.youtube.com/watch?v=W7*TkUbdqE&t=327s
[vcs]: https://en.wikipedia.org/wiki/Version_control 
