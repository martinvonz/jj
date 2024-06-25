# Buck2 builds

> [!TIP]
> This document is primarily of interest to developers. See also [Contributing]
> for more information on how to contribute in general.

There is experimental support for building `jj` with [`buck2`][Buck2] as an
alternative to `cargo`. Buck2 is a hermetic and reproducible build system
designed for multiple programming languages.

- If you're wondering "Why?", please read the section below titled
  "[Why Buck2](#why-buck2)"
- If you're interested in using Buck2 for development, please read the section
  below titled
  "[Step 1: Please please please install Dotslash](#step-1-please-please-please-install-dotslash)"

> [!WARNING]
> Buck2 support is a work in progress, and is not yet complete; writing patches
> still requires `cargo` in practice, and so it is not recommended for primary
> development use. It may never be recommended for primary development use or
> merged into the main tree.

## Current support & feature parity

Some notes about build compatibility are included below.

Legend:

- ✅: Supported
- ⚠️: Partial support/WIP
- ❌: Not supported
- ❓: Status unknown/needs testing
- ⛔: Unsupported

| Major items               | Status         |
| ------------------------- | -------------- |
| `rust-analyzer`           | ✅             |
| CI setup (GHA)            | ✅             |
| Cargo (re)synchronization | ✅<sup>1</sup> |

| Unique features  | Status          |
| --------------------- | --------------- |
| Hermetic toolchain    | ⚠️              |
| RBE/GHA `ActionCache` | ❌              |
| Auto `gen-protos`     | ✅ <sup>2</sup> |

| Support matrix        | Cargo | Buck2             |
| --------------------- | ----- | ----------------- |
| Fully working build   | ✅    | ✅               |
| Debug/Release configs | ✅    | ✅               |
| Full test suite       | ✅    |️ ❌               |
| Release-able binaries | ✅    |️ ❌<sup>3,4</sup> |
| Supports Nix devShell | ✅    |️ ⚠️<sup>5,6</sup> |

1. `Cargo.toml` files remain the source of truth for Rust dependency info, and a
   tool to resynchronize `BUILD` files with `Cargo.toml` is provided.
2. `gen-protos` rebuilds `.proto` files automatically if they change, so there
   is no need to use the committed `.rs` files.
3. macOS and Windows binaries are theoretically usable and distributable (no 3rd
   party shared object dependencies), except for being untested.
4. Linux binaries are working but we can't yet produce `musl` builds, which
   makes them less useful for distribution. However, glibc builds will often be
   faster (faster malloc and faster memcpy/string routines), so it may be good
   to support both.
5. Works fine on Linux, not macOS.
6. It is unclear whether Nix+Buck2 will be a supported combination in the long
   run

### Platform support

| OS      | Architecture | Status         |
| ------- | ------------ | -------------- |
| Linux   | x86_64       | ✅             |
|         | aarch64      | ✅<sup>1</sup> |
| macOS   | x86_64       | ❌             |
|         | aarch64      | ✅             |
| Windows | x86_64       | ✅             |
|         | aarch64      | ❌<sup>2</sup> |

1. `aarch64-linux` requires [`bindgen`][bindgen] in `$PATH`
2. Entirely theoretical at this point because many other tools need to support
   it, but a logical conclusion to all the other supported builds.

[bindgen]: https://rust-lang.github.io/rust-bindgen/command-line-usage.html

### Fixed bugs and related issues

The Buck2 build is known to fix at least the following bugs, though they may all
have alternative solutions to varying degrees:

- https://github.com/martinvonz/jj/issues/3984
  - libssh2 is built correctly by Buck2 on a fresh Windows system
- https://github.com/martinvonz/jj/issues/3322
  - BoringSSL enables ed25519 keys on all platforms in all builds
- https://github.com/martinvonz/jj/pull/3554
  - BoringSSL builds do not require perl/make
- https://github.com/martinvonz/jj/issues/4005
  - Buck2-built `jj` binaries have a statically built CRT on Windows
  - Fixed in `main` by https://github.com/martinvonz/jj/pull/4096

## Step 1: Please please please install Dotslash

Hermetic builds require using consistent build tools across developers; a major
selling point of solutions like Nix, Bazel, or Buck2 is that they do this for
us. But then how do we "bootstrap" the world with a consistent version of Buck2
to start the process?

Answer: We use [Dotslash] to manage Buck2 versions in a way that's consistent
across all developers and amenable to version control. In short, `dotslash` is
an interpreter for "dotslash files", and a Dotslash file is merely a JSON file
that lists a binary that should be downloaded and run, e.g. download binary
`example.com/aarch64.tar.gz` on `aarch64-linux`, and run the binary `bin/foo`
inside.

By marking these JSON files as `+x` executable, and using Dotslash as the
"interpreter" for them, we can transparently download and run the correct
version of Buck2 for the current platform. Most importantly, these JSON files
are very small, easy to read, and can be recorded in version control history.
That means you'll always get a consistent build even when checking out
historical versions or when working on a different machine.

You can install Dotslash binaries by following the instructions at:

- <https://dotslash-cli.com/docs/installation/>

Or, if you have Rust installed, you can install Dotslash by running:

```sh
cargo install dotslash
```

Or, if you have Nix, you can install that way as well:

```sh
nix profile install 'nixpkgs#dotslash'
```

> [!TIP]
> Check out the [Dotslash documentation](https://dotslash-cli.com/docs/),
> including the "Motivation" section, for more information about the design and
> use of Dotslash.

## Step 2: Building `jj` with Buck2

> [!IMPORTANT]
> You currently must have `rustc` installed, see <https://rustup.rs> for
> installation.

After installing `dotslash` into your `$PATH`, you can build `jj` with the
included `buck2` file under `./tools/bin`:

```sh
# Linux/macOS
export $PATH="$(jj root)/tools/bin:$PATH"
buck2 run cli -- version
```

```powershell
# Windows
dotslash ./tools/bin/buck2 run cli -- version
```

Dotslash will transparently run the correct version of `buck2`, and `buck2` will
build and run `jj version` on your behalf.

---

## Why Buck2

Today, Cargo suits Jujutsu developers quite well, as the entire project is
written in Rust. But as time goes on and we look to grow, certain limitations
will start to be felt acutely.

### Multi-language and project support

The most glaring limitation of Cargo is that, like all other language-specific
build tools, its build graph has dependencies between "targets", but it only has
a Rust-specific notion of what a "target" is. Extending its dependency graph
beyond that is nearly impossible, resulting in the need for extra tools that can
only express coarse-grained dependencies between multiple larger targets.

This has practical and pragmatic consequences. For Jujutsu, three of them in
particular are relevant to the developers today: usage of C, and usage of
JavaScript, and usage of Python.

<details>
<summary>Case 1: C dependencies</summary>

Jujutsu is written in Rust, but it currently has 3 major C libraries as
dependencies:

- `libgit2` for Git support, which needs
- `libssh2` for SSH support, which needs
- `openssl` for cryptography (e.g. TLS transport for `https` clones and ed25519
  support in libssh2)

Currently, all of these are managed on each platform by `cargo` through the use
of `build.rs` scripts that are opaque and have effectively unlimited power.
However, this has some unfortunate consequences for multi-platform support.

The most notable is that `openssl` is complicated handle on Windows due to the
requirements for Make and Perl that are needed to build it; that means it isn't
enough to just have the source code, MSVC, and the Rust compiler, but often you
will need a third party toolchain like vcpkg to provide prebuilt `.lib` files,
or you must use other tools like `msys` to provide a Bash shell with working
`perl`/`make`. (On Linux and macOS, OpenSSL support is often easily available in
some form provided by the operating system.)

To make this simpler, we _do_ have the option to refrain from `libssh2` using
OpenSSL on Windows, instead using the Windows Cryptography Next Gen (NCG)
library. This is the default when compiling from source with `cargo`.

But this gives a poorer user experience for our Windows users who compile from
source to report bugs upstream, or fix issues. For example,
[#3322](https://github.com/martinvonz/jj/issues/3322) describes a bug where a
user can no longer clone a repository because NCG does not support Ed25519 host
keys, which are offered by GitHub (requiring an extra `ssh-keyscan` step to
fix). A fix to always use OpenSSL on Windows was proposed in
[#3554](https://github.com/martinvonz/jj/pull/3554), which "vendors" OpenSSL as
part of building the `openssl` Rust crate, but returns us back to the world of
Make and Perl, which is not a great experience for users to figure out, and
seemingly requires a significant amount of platform-specific details to use the
right tools from MSYS2 or vcpkg.

This also results in a poor feedback loop: Windows users may build binaries that
are silently different from the ones they install from upstream, e.g. a user
installs an `.exe` from our release page, then builds a copy of `jj` on their
own computer, then finds the two behave differently. There is also no clear way
to know what the differences are or alert the user to them, as of today.

In contrast, the Buck2 build of Jujutsu builds exactly one version of each of
its C dependencies, and has chosen [BoringSSL] as its cryptography library on
all platforms, by shimming it into the Rust build process. BoringSSL is built
manually with our own `BUILD` files. This results in a build of `libssh2` with
identical cryptographic support, including Ed25519 keys, for all users on all
platforms. This means that Windows users can build Jujutsu with nothing more
than MSVC, the Rust compiler, and Buck2, and everything will work handily.

In the future, it may be possible to replace all these libraries with Rust
equivalents, negating the complex C build process factors. But ultimately, C or
Rust, this is an example of how a dependency you rely on and ship is ultimately
your responsibility to handle in the end. Even if the problem doesn't exist
immediately in your own codebase, it can still be a major source of confusion
and frustration for your users.

</details>
<br/>
<details>
<summary>Case 2: JavaScript usage</summary>

We would like to implement an equivalent to Sapling's `sl web` command, and
perhaps even share the code for this with a project like [`gg`][gg] and package
Tauri apps inside the main repository. There has also been discussion of
extensions for VSCode. These all require use of JavaScript, and in practice
without extreme diligence will effectively require us to integrate tools like
`pnpm` or `yarn` into the build process. Even without those tools, it will
require our build graph to ultimately have knowledge of JavaScript in some way.

A concrete example of this problem is in the [`diffedit3`][diffedit3] package by
Jujutsu contributor Ilya Grigoriev. We may even integrate `diffedit` into the
Jujutsu repository in the future. The source code repository currently is a
mixture of Cargo and npm packages, and due to the inability accurately track
changes between them, it is expected that the developers run `cargo build` and
`npm run build` in sequence and then commit the output `.js` file to the
repository (under `./webapp/dist/assets`). Not only does this bloat repository
sizes, it's unauditable too because there's no clear way to know what exactly
produced the `.js` file. Even doing such updates automatically with trusted
infrastructure (e.g. CI tools) would already require even further bespoke
tooling to be written, so the problem still exists.

</details>
<br/>
<details>
<summary>Case 3: Python usage</summary>

TODO: Currently Python is used to build our website. Explain how this is another
manifestation of the same problems above.

</details>
<br/>

The common refrain at this point is to use something like `cargo xtask` or
`make` in order to represent the dependency graph of the entire project. A
common belief is that doing so is low-cost because it does not "introduce new
dependencies" due to their ubiquity; for instance, `make` is probably installed
on Unicies while `xtask` is already common in Rust. However, the cost of a
solution has to consider adoption as well as ongoing costs. And we do not need
`xtask` and `make` today, so adding them really is adding a new dependency, even
if they're common.

Ultimately, the solutions that arise from tacking `xtask` or `make` onto an
existing group of tools all run afoul of the same fundamental problems described
in Peter Miller's important 1997 paper
["Recursive Make Considered Harmful"][rmch], including build graphs that are too
conservative (because finer dependencies can't be expressed, so you must be
safe) and are fundamentally incomplete (because the build system can't see the
whole picture of input and output files).

[BoringSSL]: https://github.com/google/boringssl
[gg]: https://github.com/gulbanana/gg
[diffedit3]: https://github.com/ilyagr/diffedit3/
[rmch]: https://aegis.sourceforge.io/auug97.pdf

### Hermetic, safe, scalable builds

> [!IMPORTANT]
> Buck2 builds of `jj` are not yet hermetically sound. In particular,
> unrestricted access to the filesystem is allowed at build time and we do not
> yet provide hermetic toolchains.

Buck ultimately treats the build process as a series of _pure functions_ of
their inputs, including all the compilers and tools and source code needed.
Given the same inputs to a tool, you always get the same outputs, down to the
same binary bits. As a result of this, the build graph that Buck constructs will
be "complete" and will fully capture the relationships between all inputs,
commands, and their outputs. That means the build is fundamentally _hermetic_.

Hermetic builds becomre more valuable as a project grows in size, because the
probability of introducing arbitrary untracked side effects into the build
process approaches 1 as time goes on, often with unintended consequences. The
most ideal outcome of this is a simple failure to compile; more dangerous
results are flaky builds, silent miscompilations, and non-deterministic build
outputs based on internal implementation details e.g. an unstable sort.

Hermetic builds are also essential for _security_, because they help ensure that
builds are repeatable given a known "ground truth". Scenarios like the [xz utils
backdoor] have many complex factors involved, but an easy to understand one is
that the backdoor relied on the build process being non-hermetic; the backdoor
was inserted under a specific set of trigger criteria that then changed the
taken actions, which could have been detected more easily had there been a known
reproducible output to compare against. Hermetic builds derived from source code
mean that backdoors often have to be inserted in-band _into the code itself_ and
cannot be inserted out-of-band into the build process so easily.

Finally, hermeticity is an essential feature for _build performance_ at scale
because it is required to allow sensible remote execution, avoid overly
conservative rules, and enable safe caching between computers. The relationships
Buck captures are ultimately as fine grained as desired, down to individual
files and commands, across any language. Such fine detail can only be achieved
with a very complete understanding of the inputs.

[xz utils backdoor]: https://en.wikipedia.org/wiki/XZ_Utils_backdoor

### Remote cache support

Because Buck can see the entire build graph, and the input/output relationship
between every file and command, it is possible to cache the intermediate results
of every build command and every file that is produced, and then download them
transparently on another (compatible) machine. This can be assumed safe under
some basic assumptions (one being that the build is hermetic.)

The most common case of this is between the CI system and the developer; every
change must pass CI to be merged, and when a change is merged the results of
that build are put in a public cache. A developer may go to sleep for the night
and something gets merged during their slumber. When they wake up, then can
update to the new `main` branch, run `buck2 build`, and will instantly get a
cached build instead of recompiling.

### "Telescopic" Continuous Integration

Over the past 15 years, originally with systems like Travis CI and now
GitHub Actions, adoption of continuous integration and continuous builds have
transformed how the typical open source project develops and operates. Overall,
this is a good thing but not without consequence; widely available CI platforms
like GitHub Actions provide economies of scale that allow average developers to
test and deliver their projects more effectively.

The average modern open source project is, at a high level, defined by two
separate components when talking about health:

- The build system, whatever that might be. And,

- The continuous integration system, which sets up an "environment", then runs
  the build system in a controlled way. For example, the environment might include
  a C++ compiler of a particular kind, which is needed.

And, as an addendum:

- Typically this will also run some kind of testing strategy in one of the
  two phases (normally the second one as some external platform dependencies
  may be needed for "full coverage")

The job of a build system is to execute programs and produce resulting artifacts
for the purpose of using it (presumably). The job of a CI system is to execute
programs and produce artifacts, but on a remote server provided to you, for
the purpose of shipping software. These are actually quite similar in scope and
requirements, but traditionally the dichotomy of build systems vs build machines
has blurred that a little. And not without reason.

However, today, modern CI is wonderfully complex, and often strangely
fragile. For example, it often requires you to do things like implement build
pipelines in configuration like YAML, or certain features are gated behind
non-programmable components. Components in the build graph are often implemented
by 3rd parties which are not much different than ordinary dependencies.

Take GitHub Actions: "actions" are pre-canned steps written in TypeScript or
reusable YAML, like `actions/checkout` or `actions/upload-artifact`. The thing
is, if you have *any third party actions at all* in your workflow, then you
suddenly require the entire GHA system in order to test things. You could write
your entire CI system as a single bash script, but then you have to reimplement
all the logic behind those other components so that it works standalone. You
either add a big dependency on the overall system behavior or end up redoing
everything. There isn't a good trade off between complexity and reuse.

Another way of thinking of it is that you have the build graph as understood by
the build system, but then your CI system has to implement a *separate* "build
graph" that orchestrates a bunch of work. The graph in a GHA Worfklow page is
not much different from the graph that a tool like `make` or `buck2` builds, at
the end of the day.

> See also: *[Modern CI is too complex and misdirected][modern-ci]*, by Gregory
Szorc

[modern-ci]: https://gregoryszorc.com/blog/2021/04/07/modern-ci-is-too-complex-and-misdirected/

Instead, in a fully hermetic build system, the build system controls the
*entire* build graph, and all components are considered to be part of that
graph: including all constraints like required compilers, libraries, and all
dependent inputs. There is only *one* build graph that controls everything
and it is available at all times.

A practical consequence of this is that *nothing is needed but the code
repository* to run all phases of the build. Stages that would traditionally be
relegated to CI like `lint` or `check-dependency-license` scripts are instead
*tests* that are run on every commit, inside the system. There is no distinction
between a "build machine" and "your machine" or any other machine. Anything a CI system
can do, you can reproduce (within reason; such as GPU access.)

Another practical consequence is that this approach gives you independence from
the underlying CI system. Migrating from GHA to GitLab or other providers is made
far easier because *all* build and test logic resides within Buck's build graph,
not within YAML files describing a particular vendor's approach to running code as
a service.

This "Buck does everything" approach is what some might call a "Rampant Layering
Violation", but another way of thinking of it is that a system like Buck can be
used to [telescope the infinite series][bonwick-telescope], making the problems
introduced by two separate systems disappear by collapsing all of the
intermediate layers into one.

[bonwick-telescope]: https://web.archive.org/web/20200427030808/https://blogs.oracle.com/bonwick/rampant-layering-violation

### Dogfooding deeper VCS+build integration

A long-term goal for Jujutsu is to actively explore and develop integrations
with the build system and file system layers, but the biggest benefits can
only be realized when they are all tightly woven together. Buck can give us
an opportunity to explore these paths as it is designed with such scenarios
in mind.

As an example, integration between the version control system and the build
system allows you to ask a question like:

- What was the last commit ABC that produced an artifact with hash XYZ?

Further integration between the build system and the filesystem allows you
to ask:

- What is the hash of file XYZ, *without* demanding the contents of the file?
  For example, XYZ may be large, or there may be millions of "XYZs" to
  account for in ultra large codebases.

In the first case, this lets you immediately deduce which commit introduced any
result, which is not only useful for bug hunting but also historical queries
("why did this binary get really big?")

The second question allows you to do things like build large amounts of the
codebase *without* materializing anything. If you change a file that is an input
to a deeply nested dependency, perhaps a dependency to 1 million subsequent
input files/build targets, Buck will be able to run the majority of the
build *without* materializing the intermediate files: it will use the remote
execution framework to re-run commands, which will have those files available
already. Some amount of actions will be executed locally, too, to better utilize
available resources, but this will often be a very small proportion of overall
targets.

These kinds of integrations are key for extremely large repositories to build in
small amounts of time and an active topic of interest. Not only can switching to
Buck provide us faster builds, but it gives us a keen first-party opportunity to
use and explore this design space.

### Integrated visibility of autogenerated or 3rd party dependencies

Because Buck wants a complete dependency graph for all components in the system,
from source code to final binary, we are given the opportunity to have deeper
visibility into our 3rd party components and their code.

For example, all input files to all dependencies, even if downloaded from
remote URLs or from `.crate` files, or checked into the repository normally,
are all accessible and queryable within the Buck build graph through tools
like BXL. This means that we can do things like index *all* of the `jj` the
source code with a tool like [Zoekt], including third-party code *and* even all
auto-generated code, because the steps are visible within Buck.

Not only does this allow us to run things like search/indexing over the whole
set of source files, it also opens the way to things like vulnerability scanning
tools or large-scale code analysis; Buck integration with [OSV.dev] is already
actively being explored, and it provides an opportunity for us to move away from
3rd party intermediates like GitHub to provide such services.

[Zoekt]: https://github.com/sourcegraph/zoekt
[OSV.dev]: https://osv.dev/

### Early movers advantage

Given the fact that Cargo currently works well for our needs, why should we
investigate Buck2 now? Wouldn't it be better to wait until much later on? Are
there any major benefits to using it before that point?

Most people think of large-scale build tools like Nix or Bazel as necessary only
once the project has begin growing out of control. But by that point, there is
often [strong inertia against such a change][xkcd1172] and large amounts of
technical debt in the way, making such migration difficult and costly.

[xkcd1172]: https://xkcd.com/1172/

In a twist, the easiest time to adopt hermetic and scalable build systems is
_early on_ in a project's lifecycle, even when the benefits are not fully
realized, because this is when the cost is lowest, and early introduction can
help prevent impedance mismatches from being introduced that would otherwise
sour the attempt.

Furthermore, executing early on this means that we are not blocked on
compromises like handling JavaScript, in the case of `diffedit3` &mdash; and so
we may be able to execute on certain plans _earlier_ than we otherwise would
have. This is not the same as a free lunch; rather the _path_ to achieving
difficult things is unblocked, even if the road to get there still requires
work. For more information on this, see the section below titled "[Future
endeavours](#future-endeavours)".

---

## Buck2 crash course

The following is an extremely minimal crash course in Buck2 concepts and how to
use it.

### Target names

For users, Buck2 is used to build **targets**, that exists in **packages**,
which are part of a **cell**. Targets are defined in `BUILD` files (discussed
more in a moment) and a single `BUILD` file may have many targets defined
within.

Targets may have dependencies on other targets, and so all targets collectively
form a directed acyclic graph (DAG) of dependencies, which we call the **target
graph**.

The most explicit syntax for referring to a target is the following:

```text
cell//path/to/package:target-name
```

A cell is a short name that maps to a directory in the code repository. A
package is a subdirectory underneath the cell that contains the build rules for
the targets. A target is a buildable unit of code, like a binary or a library,
named in the `BUILD` file inside that package (more on that soon).

`buck2 build` requires at least one target name, like the one above. The above
is an example of a "fully qualified" target name which is an unambiguous
reference.

A fully-qualified reference to a target works anywhere in the source code tree,
so you can build or test any component no matter what directory you're in.

So, given a cell named `foobar//` located underneath `code/foobar`, and a
package `bar/baz` in that cell, leads to a file

```text
code/foobar/bar/baz/BUILD
```

Which contains the targets that can be built.

There are several shorthands for a target:

- NIH.

### `BUILD` files

If we consider tools like `make` and `cargo` to exist at different points in a
spectrum, then Buck is actually much closer to `make` in spirit than most of the
others.

As noted previously, a `BUILD` file (also sometimes named `BUCK` or `TARGETS`)
for a package lists targets, which specify dependencies on other targets,
forming a directed acyclic graph (DAG) of dependencies called the **target
graph** (which at a very high level sounds similar to a `Makefile`.)

A `BUILD` file generally looks like this:

```bazel
cxx_rule(name = 'foo', ...)

rust_rule(name = 'bar', deps = [ ":foo" ], ...)

java_rule(name = 'baz', deps = [ ":foo", ":bar" ], ...)
```

In this example, `foo` is a C++ binary, `bar` is a Rust binary that depends on
`foo`, and `baz` is a Java binary that depends on both `foo` and `bar`. (It is
easy to see how this is somewhat spritually similar to a Makefile.)

A target is created by applying a rule, such as `cxx_rule` or `rust_rule`, and
assigning it a `name`. There can only be one target with a given name in a
package, but you can use the same rule multiple times with different names.

Unlike Make, Buck requires that the body of a rule, its "implementation", must
be defined separately from where the rule is used. A rule can not be defined in
`BUILD` files, but only applied to arguments and bound to a name.

It is important to note that these rules have no evaluation order defined. You
are allowed to write `cxx_rule` at the bottom of the file in the above example.
The name of the target is what matters, not the order in which the targets are
written. `BUILD` files only describe a graph, not a sequence of operations.

More generally, a rule is just a function, a target is just the application of a
function to arguments, and the `name` field is a special argument that defines a
"bound name" for the result of the function call. So a `BUILD` file is just a
series of function calls, that might depend on one another. In a more "ordinary"
language, the above example might look like this:

```bazel
bar = rust_rule(deps = [ foo ], ...)

baz = java_rule(deps = [ foo, bar ], ...)

foo = cxx_rule(...)
```

While this is a deeper topic, ultimately, the syntax Buck2 uses is a pragmatic
compromise, given the semantics of existing `BUILD` files. We can't change that,
but the "function application" metaphor is a very useful one to keep in mind.

### Abstract targets & action graphs

NIH.

### Target visibility

Every target can have an associated _visibility list_, which restricts who is
capable of depending on the target. There are two types of visibility:

- `visibility` - The list of targets that can see and depend on this target.
- `within_view` - The list of targets that this target can see and depend on.

Visibility is a practical and powerful tool for avoiding accidental
dependencies. For example, an experimental crate can have its `visibility`
prevent general usage, except by specific other targets that are testing it
before committing to a full migration.

### Package files

In a package, there can exist a `PACKAGE` file alongside every `BUILD` file. The
package file can specifie metadata about the package, and also control the
default visibility of targets in the package.

### Mode files

In order to support concepts like debug and release builds, we use the concept
of "mode files" in Buck2. These are files that contain a list of command line
options to apply to a build to achieve the desired effect.

For example, to build in debug mode, you can simply include the contents of the
file `mode//debug` (using cell syntax) onto the command line. This can
conveniently be done with "at-file" syntax when invoking `buck2`:

```sh
buck2 build cli @mode//debug
buck2 build cli @mode//release
```

Where `@path/to/file` is the at-file syntax for including the contents of a file
on the command line. This syntax supports `cell//` references to Buck cells, as
well.

In short, `buck2 build @mode//file` will apply the contents of `file` to your
invocation. We keep a convenient set of these files maintained under the
`mode//` cell, located under [`./buck/mode`](../buck/mode).

#### At-file syntax

The `buck2` CLI supports a convenient modern feature called "at-file" syntax,
where the invocation `buck2 @path/to/file` is effectively equivalent to the
bash-ism `buck2 $(cat path/to/file)`, where each line of the file is a single
command line entry, in a consistent and portable way that doesn't have any limit
to the size of the underlying file.

For example, assuming the file `foo/bar` contained the contents

```text
--foo=1
--bar=false
```

Then `buck2 --test @foo/bar` and `buck2 --test --foo=1 --bar=false` are
equivalent.

### Buck Extension Language (BXL)

NIH.

## Examples

Some examples are included below.

<details>
<summary>Run the <code>jj</code> CLI</summary>

The following shorthand is equivalent to the full target `root//cli:cli`:

```sh
buck2 run //cli
```

This works anywhere in the source tree. It can be shortened to `buck2 run cli`
if you're already in the root of the repository.

</details>

<details>
<summary>Run BoringSSL <code>bssl speed</code> tests</summary>

```sh
buck2 run third-party//bssl @mode//release -- speed
```

</details>

<details>
<summary>Build all Rust dependencies</summary>

```sh
buck2 build third-party//rust
```

</details>

<details>
<summary>Download all <code>http_archive</code> dependencies</summary>

Useful for downloading all dependencies, then testing clean build times
afterwards.

```sh
buck2 build $(buck2 uquery "kind('http_archive', deps('//...'))" | grep third-party//)
```

</details>

---

## Future endeavours

NIH

---

## Development notes

Notes for `jj` developers using Buck2.

### Build mode reference

You can pass these to any `build` or `run` invocation.

- `@mode//debug`
- `@mode//release`

### Cargo dependency management

Although Buck2 downloads and runs `rustc` on its own to build crate
dependencies, our `Cargo.toml` build files act as the source of truth for
dependency information in both Cargo and Buck2.

Updating the dependency graph for Cargo-based projects typically comes in one of
two forms:

- Updating a dependency version in the top-level workspace `Cargo.toml` file
- Adding a newly required dependency to `[dependencies]` in the `Cargo.toml`
  file for a crate

After doing either of these actions, you can synchronize the Buck2 dependencies
with the Cargo dependencies with the following command:

```bash
buck2 -v0 run third-party//rust:sync.py
```

This must be run from the root of the repository. Eyeball the output of
`jj diff` and make sure it looks fine, then test, before committing the changes.

This step will re-synchronize all `third-party//rust` crates with the versions
in the workspace Cargo file, and then also update the `BUILD` files in the
source code with any newly added build dependencies that were added or removed
(not just updated).

### `rust-analyzer` support

> [!IMPORTANT]
> You **MUST** have `rustc` nightly 1.82+ installed for this to work, as it
> requires the `discoverConfig` option in `rust-analyzer`, which may not
> otherwise be available to you. If your editor or IDE integration pre-packages
> `rust-analyzer`, you may not need to do this.

As of 2024-08-27, there is rather robust support for `rust-analyzer` in your
IDE, thanks to the `rust-project` integration for Buck2, located under
[`integrations/rust-project`](https://github.com/facebook/buck2/tree/main/integrations/rust-project)
in the Buck2 source.

Prebuilt binaries will come later, but for now, you can build `rust-project` and
a compatible version of `buck2` from the source repository:

```sh
jj git clone --colocate https://github.com/facebook/buck2 ~/buck2
cd ~/buck2
cargo build --release --bin buck2 --bin rust-project
```

You should have `buck` and `rust-project` underneath the `target/release` build
directory. Now add them to your `$PATH` and try to build `jj`:

```sh
cd ~/src/jj
export PATH="$HOME/buck2/target/release:$PATH"
buck2 build cli
```

If this works, then the compatible `rust-project` will be available in your
`$PATH`, and you can now launch your editor with the proper configuration to use
`rust-project`.

> [!IMPORTANT]
> Your editor must inherit the `$PATH` environment variable from your shell in
> this example. You need to configure it otherwise, if it does not.

#### Visual Studio Code

There is a preconfigured `.vscode/settings.json` file in the repository that
should do the work, so make sure you launch a new window with the `jj`
repository open and the new `buck2` and `rust-project` binaries in your `$PATH`:

```sh
code .
```

Now open an `.rs` file and your typical features like `Right Click -> Go To
Definition` should work.

#### Zed

TBD.

#### Helix

TBD.

#### Neovim

TBD.

---

## TODO + known Buck2 bugs

TODO list:

- [ ] `rust-analyzer` working OOTB
  - Working, but buggy.
  - More details below
- [ ] Build time improvements
  - Clean from scratch build is still about 2x slower than `cargo`
  - Incremental rebuilds are quite comparable, though
- [ ] hermetic toolchain
  - [x] ~~system bootstrap python via
        <https://github.com/indygreg/python-build-standalone>~~
  - [ ] rustc
  - [ ] clang/lld
- [ ] remote caching
- [ ] remote execution
- macOS:
  - [ ] x86_64: get build working
    - [ ] requires fixes to `smoltar`
    - ~~mostly due to lack of an available x86_64 macOS machine~~ `macos-13`
      seems to be available
  - [ ] get working in nix devShell, somehow
    - linking `libiconv` is an issue, as usual
    - requires the right shell settings, I assume
- Linux
  - [x] ~~aarch64-linux: get `bssl-sys` working with bindgen~~
  - [ ] remove workaround: aarch64-linux requires `bindgen` in `$PATH`, for now
- Windows
  - [ ] Is hermetic MSVC possible?
  - [ ] [windows_shim for DotSlash](https://dotslash-cli.com/docs/windows/),
        improving Windows ergonomics
    - Requires committing binary `.exe` files to the repo, so optimized size is
      critical
    - Currently does not exist upstream; TBA

Miscellaneous things:

- [ ] Why does `buck2 build @mode//release` and then `buck2 build @mode//debug`
      cause a redownload of `.crate` files?
  - Only happens when switching modes; incremental builds with the same mode are
    fine
  - Early cutoff kicks in so this only incurs a few seconds of extra time
    typically, because once Buck sees that the `.crate` files haven't actually
    changed it can quit early.

Upstream bugs:

- RE/AC support:
  - [ ] Missing `ActionCache` support <https://github.com/facebook/buck2/pull/477>
  - [x] ~~File size logic bugs <https://github.com/facebook/buck2/pull/639>~~
  - [x] ~~Buggy concurrency limiter <https://github.com/facebook/buck2/pull/642>~~
  - [ ] Failure diagonstics <https://github.com/facebook/buck2/pull/656>
- `rust-analyzer` + `rust-project`
  - [x] ~~`linked_projects` support <https://github.com/rust-lang/rust-analyzer/pull/17246>~~
  - [x] ~~Unbreak OSS use of `rust-project` <https://github.com/facebook/buck2/pull/659>~~
  - [x] ~~`--sysroot-mode` support for `rust-project` <https://github.com/facebook/buck2/pull/745>~~
  - [x] ~~`rust-project check` is broken <https://github.com/facebook/buck2/pull/754>~~
  - [x] ~~Invalid dep graph due to lack of `sysroot_src`~~
    - <https://github.com/facebook/buck2/issues/747>
    - <https://github.com/facebook/buck2/pull/756>
  - [ ] Prebuilt `rust-project` binaries + dotslash
- Miscellaneous
  - [x] ~~`buck2` aarch64-linux binaries don't with 16k page size <https://github.com/facebook/buck2/pull/693>~~
  - [ ] Aggressively annoying download warnings <https://github.com/facebook/buck2/issues/316>
  - [ ] Distributing log files <https://github.com/facebook/buck2/issues/441>
    - Buck2 logs are included in CI artifacts, but not published anywhere

<!-- References -->

[Contributing]: https://martinvonz.github.io/jj/latest/contributing/
[Buck2]: https://buck2.build/
[Dotslash]: https://dotslash-cli.com/
