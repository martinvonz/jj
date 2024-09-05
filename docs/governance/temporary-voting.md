# Getting Community Buy-in for Working Group Proposals

## Introduction

We're introducing a temporary process to describe how we'll gain approval to
adopt permanent governance policies - basically, how we make social and
technical decisions as a community. This temporary process describes how the
governance working group can propose these policies and how community members
can influence them and vote on them. Once permanent governance policies are in
place, the temporary process will stop being used, and the permanent governance
policies will be used instead.

## Context

The governance working group was appointed by recommendation from Martin (jj's
original author and current sole maintainer), without recommendation or approval
from the broader jj community. This isn't a problem in itself - but it does
mean that the governance working group (Austin Seipp/aseipp, Waleed
Khan/arxanas, Martin von Zweigbergk/martinvonz, and Emily Shaffer/nasamuffin)
needs to get some community approval before setting policy for the entire jj
project. If we skip this step, we risk being perceived as exercising excessive
control over the project.

## Goals and Non-Goals

* This process will be used to approve things like a `governance.md` (describing
  the formal structure of governance used for this project), technical design
  approval process, and code review process.
* This is **not** a process that will be used forever. It is intended as a
  temporary process, only used to approve more permanent processes and policies
  for the project.
* This process is used to gather feedback, approval, and acceptance from
  invested jj community members. Current members of the community should be able
  to participate in voting without hardship.
  * Current community members include code committers, code reviewers, those
    providing user support, those providing quality, actionable feedback, those
    providing documentation (first-party or third-party), developers of
    jj-compatible tools and add-ons (like GUIs or IDE extensions), and those
    providing design input and feedback.
    * If you feel that you are a member of the community but do not fit into one
      of these buckets, please reach out to one of the members of the working
      group to have this list expanded.
* This process **is** the primary way for general community members to influence
  governance policies and processes. It should invite constructive feedback and
  help us form policies that are acceptable to the jj group as a whole.
  * It's intended to meet community members where they are - on GitHub and on
    Discord, where all development occurs and most support and technical
    discussion occurs.
* This is **not** a process for gaining unanimous agreement - there are too
  many of us for that to be feasible. Instead, it is a process for gaining
  widespread community approval.

## Process

### Stage 1: Advance Notice of Effort

The working group lets the community know about upcoming policy drafts they're
intending to share for approval. This must happen at least a week before
entering stage 3, and ideally should happen even earlier.

At this time, the working group should:

* Describe why the working group feels this policy is needed
* Describe the basic goals the policy should achieve
* Describe implementation details that are being considered, if any
* Create discussion thread on GitHub (and link to it from Discord). The GitHub
  discussion thread is the canonical thread for discussion and will be reused
  through the lifetime of a proposal as it moves through this process.

At this time, the community is invited to:

* Recommend additional goals, or discuss nuances of the stated goals the working
  group has already shared
* Recommend implementation details

The working group will consider these recommendations in good faith, but may
choose not to adopt them.

### Stage 2: Proposal Review Period

This stage lasts until the working group feels major concerns have been
addressed and the proposal is ready for a vote. However, **at least 72 hours**
must elapse between the proposal being published and the vote starting, to allow
community members around the globe to read and comment. Typically, this stage
should last at least one week.

At this time, the working group should:

* Share the full text of the proposal as a GitHub pull request (PR)
* Link this GitHub PR to the existing Discord notification thread and GitHub
  discussion
* Explain how the proposal meets the goals stated in Stage 1, either within the
  proposal itself or in commentary next to the proposal

At this time, the community is invited to:

* Share constructive recommendations in GitHub to modify the text of the
  proposal, or discuss nuances of the proposal's wording
* Share showstopper concerns in GitHub about the proposal, including details
  about how and why the concern is especially dire

Think of this like a code review; the goal of this stage is to build a proposal
that is representative of the community's will. Keep recommendations actionable
and constructive: "This clause discourages X; if we phrase it like "foo bar baz"
it could be less exclusive" is much more productive than "It's obvious that the
governance working group doesn't want X!"

At the discretion of the working group, but based on the outcome of the
discussion, the proposal will go to a vote **or** the proposal will be dropped.

### Stage 3: Proposal Voting Period

When the working group feels that major concerns have been addressed and is
happy with the text of the proposal, the working group will open voting on the
proposal.

* Voting occurs on GitHub using the poll feature and is advertised heavily on
  Discord during the voting period.
  * If community members want to vote but aren't able to use GitHub, they can
    message nasamuffin@ (on Discord, or nasamuffin at google dot com) with their
    vote to have it manually included. Only one working group member is listed
    in order to avoid accidental double-counting.
  * When voting against, community members should comment on the post explaining
    why and describe what change would be required for them to abstain or vote
    in favor.
  * Generally, assume that the votes may be publicly visible or may be made
    publicly visible at a later time.
* Voting is open for at least 1 week, but may be open as long as 2 weeks when
  appropriate. After that deadline, the GitHub poll will be locked.
    * The deadline must be announced at the beginning of the voting period -
      once voting has begun, the deadline cannot change.
    * The working group may set the voting period longer to encompass two
      weekends (for more participation around day jobs), for less urgent or more
      complex proposals, or to account for holidays during the voting period.
* Participants can vote in favor or against.
  * "Participants" means the group of community members as enumerated at the
    beginning of this document.

**Proposals with 2/3 or more votes in favor at the end of the voting period will
be approved.**

After voting has concluded, either:

* The proposal will be implemented (if accepted)
* The proposal may be revised and begin again at stage 2 (if rejected)
* The proposal may be abandoned (if rejected)

Deciding whether to revise or abandon is up to the discretion of the governance
working group. The working group is expected to double-check their assumption
that the goals the proposal is attempting to meet are desirable after the
proposal fails to be accepted.

### Stage 4: Implementation

Typically, implementation will look like merging the document with the policy
into the jj codebase and remembering to use that policy in conversations moving
forward.

In some cases, implementation may also involve nomination of individuals to a
group or committee. When this is necessary, expect the policy being proposed to
describe how these individuals will be nominated, both initially and moving into
the future.

It's possible (but unlikely) that during implementation, some obstacle will
arise that means the policy doesn't actually work. If this does happen, expect
the working group to be transparent with the community about the situation. We
may reuse some of all of this process to figure out how to move forward.
