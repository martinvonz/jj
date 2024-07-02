# Buck2 builds

> [!TIP]
> This document is primarily of interest to developers. See also [Contributing]
> for more information on how to contribute in general.

There is experimental support for building `jj` with [`buck2`][Buck2] as an
alternative to `cargo`. Buck2 is a hermetic and reproducible build system
designed for multiple programming languages.

- If you're wondering "Why?", please read the section below titled "[Why
  Buck2](#why-buck2)"
- If you're interested in using Buck2 for development, please read the section
  below titled "[Step 1: Please please please install
  Dotslash](#step-1-please-please-please-install-dotslash)"

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

### Overall status

| Feature                | Status |
|------------------------|--------|
| `rust-analyzer`        | ⚠️      |
| CI setup (GHA)         | ✅     |
| Cargo (re)synchronization  | ✅<sup>1</sup>     |

1. `Cargo.toml` files remain the source of truth for Rust dependency info, and a
tool to resynchronize `BUILD` files with `Cargo.toml` is provided.

| Unique features        | Status |
|------------------------|--------|
| Hermetic toolchain     | ⚠️      |
| RBE/GHA `ActionCache`  | ❌     |
| Auto `gen-protos`      | ✅ <sup>1</sup>     |

1. `gen-protos` rebuilds `.proto` files automatically if they change, so there
   is no need to use the committed `.rs` files.

### vs Cargo

| Feature                | Cargo | Buck2 |
|------------------------|-------|-------|
| `rust-analyzer`        | ✅    | ⚠️    |
| Fully working build    | ✅    | ✅    |
| Debug/Release configs  | ✅    | ✅    |
| Full test suite        | ✅    |️ ❌    |
| Release-able binaries  | ✅    |️ ❌<sup>1,2</sup>    |
| Supports Nix devShell  | ✅    |️ ⚠️<sup>3,4</sup>     |

1. macOS and Windows binaries are theoretically usable and distributable (no 3rd
party shared object dependencies), except for being untested.
2. Linux binaries are working but we can't yet produce `musl` builds, which
makes them less useful for distribution. However, glibc builds will often be
faster (faster malloc and faster memcpy/string routines), so it may be good to
support both.
3. Works fine on Linux, not macOS.
4. It is unclear whether Nix+Buck2 will be a supported combination in the long run

### Platform support

| OS      | Architecture | Status |
|---------|--------------|--------|
| Linux   | x86_64       | ✅     |
|         | aarch64      | ✅<sup>1</sup> |
| macOS   | x86_64       | ❌     |
|         | aarch64      | ✅     |
| Windows | x86_64       | ✅     |
|         | aarch64      | ❌<sup>2</sup> |

1. `aarch64-linux` requires [`bindgen`][bindgen] in `$PATH`
2. Entirely theoretical at this point because many other tools need to support
   it, but a logical conclusion to all the other supported builds.

[bindgen]: https://rust-lang.github.io/rust-bindgen/command-line-usage.html

### Fixed and related bugs

The Buck2 build is known to fix at least the following bugs, though they may all
have alternative solutions to varying degrees:

- https://github.com/martinvonz/jj/issues/3984
  - libssh2 is built correctly by Buck2 on a fresh Windows system
- https://github.com/martinvonz/jj/issues/3322
  - BoringSSL enables ed25519 keys on all platforms in all builds
- https://github.com/martinvonz/jj/pull/3554
  - BoringSSL builds do not require perl/make

## Step 1: Please please please install Dotslash

Hermetic builds require using consistent tools across developers, and this is a
big selling point of solutions like Nix, Bazel, or Buck2. These tools can
download and manage consistent versions on our behalf. But then how do we
"bootstrap" the world with a consistent version of Buck2 to start the process?

Answer: We use [Dotslash] to manage Buck2 versions in a way that's consistent
across all developers and amenable to version control. In short, a Dotslash file
is merely a JSON file that lists a binary that should be downloaded and run,
e.g. download binary `example.com/aarch64.tar.gz` on `aarch64-linux`, and run
the binary `bin/foo` inside.

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

Currently Cargo suits the needs of the Jujutsu develoeprs quite well, as the
repository is almost entirely written in Rust. Despite that, certain limitations
exist, and as we look to grow and expand the project with new functionality some
of those become more apparent and difficult to handle.

### Multi-language and project support

The most glaring limitation of Cargo is that, like all other language-specific
build tools, its view of a build graph has dependencies between "targets", but
it has a limited language-specific notion of what a "target" is. Extending its
dependency graph beyond that is nearly impossible, resulting in extra tools
needed that express only coarse-grained dependencies between multiple large
tools.

This has practical and pragmatic consequences. For Jujutsu, three of them in
particular are relevant to the developers today: usage of C, and usage of
JavaScript, and usage of Python.

<details>
<summary>Case 1: C dependencies</summary>

Jujutsu is written in Rust, but it currently has 3 major C libraries as
dependencies:

- `libgit2` for Git support, which needs
- `libssh2` for SSH support, which needs
- `openssl` for cryptographic support (like Ed25519 keys)

Currently, all of these are managed on each platform by `cargo` through the use
of `build.rs` scripts that are opaque and have effectively unlimited power.
However, this has some unfortunate consequences for multi-platform support.

The most notable is that `openssl` is complicated handle on Windows due to the
requirements for Make and Perl that are needed to build it; that means it isn't
enough to just have the source code, MSVC, and the Rust compiler, but often you
will need a third party toolchain like vcpkg to provide prebuilt `.lib` files.
(On Linux and macOS, OpenSSL support is often easily available in some form
provided by the operating system.)

In order to make this simpler, we *do* have the option to refrain from `libssh2`
using OpenSSL on Windows, instead using the Windows Cryptography Next Gen (NCG)
library, which is the default when compiling from source with `cargo`.

But this gives a poorer user experience for our Windows users who compile from
source to report bugs upstream, or fix issues. For example,
[#3322](https://github.com/martinvonz/jj/issues/3322) describes a bug where a
user can no longer clone a repository because NCG does not support Ed25519 host
keys, which are offered by GitHub (requiring an extra `ssh-keyscan` step to
fix). A fix to always use OpenSSL on Windows was proposed in
[#3554](https://github.com/martinvonz/jj/pull/3554), which "vendors" OpenSSL as
part of building the `openssl` Rust crate, but once again in turn requires both
Make and Perl to build, which is not a great experience for users to figure out,
and seemingly requires a significant amount of platform-specific details to use
the right tools from MSYS2 or vcpkg.

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
solution has to consider not just adoption costs but ongoing costs to the
system. And ultimately we do not need `xtask` and `make` *today*, and so
requiring them really *is* adding new dependencies, even if they're common ones,
meaning we need to support and debug and maintain them like any other.

Ultimately, the solutions that arise from tacking `xtask` or `make` onto an
existing group of tools all run afoul of the same fundamental problems described
in Peter Miller's important 1997 paper ["Recursive Make Considered
Harmful"][rmch], including build graphs that are too conservative (because finer
dependencies can't be expressed, so you must be safe) and are fundamentally
incomplete (because the build system can't see the whole picture of input and
output files).

[BoringSSL]: https://github.com/google/boringssl
[gg]: https://github.com/gulbanana/gg
[diffedit3]: https://github.com/ilyagr/diffedit3/
[rmch]: https://aegis.sourceforge.io/auug97.pdf

### Hermetic, safe, scalable builds

> [!IMPORTANT]
> Buck2 builds of `jj` are not yet hermetically sound. In particular,
> unrestricted access to the filesystem is allowed at build time and we do not
> yet provide hermetic toolchains.

Buck ultimately wants the build process to be a *pure function* of its inputs,
including all the compilers and tools and source code needed. Given the same
inputs, you always get the same outputs, down to the same binary bits. As a
result of this, the build graph that Buck constructs will be "complete" and will
fully capture the relationships between all inputs, commands, and outputs. That
means the build is fundamentally *hermetic*.

Hermetic builds are essential as any project grows in size, because the
probability of introducing arbitrary untracked side effects into the build
process approaches 1 as time goes on, often with unintended consequences. The
most ideal outcome of this is a simple failure to compile; more dangerous
results are flaky builds, silent miscompilations, and non-deterministic build
outputs based on internal implementation details (e.g. an unstable sort).

Hermetic builds are also essential for *security*, because they help ensure that
builds are repeatable given a known "ground truth". Scenarios like the [xz utils
backdoor] have many complex factors involved, but an easy to understand one is
that the backdoor relied on the build process being non-hermetic; the backdoor
was inserted under a specific set of trigger criteria that modified the build
system actions, which could have been detected more easily had there been a
known reproducible output to compare against. Hermetic builds derived from
source code mean that backdoors often have to be inserted in-band *into the code
itself* and cannot be inserted out-of-band into the build process so easily.

Finally, hermeticity is an essential feature for *build performance* at scale
because it is required to avoid scalable remote execution, overly conservative
rules, safe caching. The relationships Buck captures are ultimately as fine
grained as desired, down to individual files and commands, across any language.
Such fine detail can only be achieved with a very complete understanding of
the inputs.

[xz utils backdoor]: https://en.wikipedia.org/wiki/XZ_Utils_backdoor

### Remote cache support

Because Buck can see the entire build graph, and the input/output relationship
between every file and command, it is possible to cache the results of every
build command and every file that is produced, and then download them
transparently on another (compatible) machine.

The most common case of this is between the CI system and the developer; every
change must pass CI to be merged, and when a change is merged the results of
that build are put in a public cache. A developer may go to sleep for the night
and something gets merged during their slumber. When they wake up, then can
update to the new `main` branch, run `buck2 build`, and will instantly get a
cached build instead of recompiling.

### Early movers advantage

Given the fact that Cargo currently works well for our needs, why should we
investigate Buck2 now? Wouldn't it be better to wait until much later on when
it's needed? The reality is that the easiest time to adopt hermetic and scalable
build systems is *early on* in a project's lifecycle, because by the time it's
"needed" it implies an accumulation of technical debt that is hurting you, which
will simultaneously make migration expensive and difficult at the same time.

Furthermore, executing early on this means that we are not blocked on
compromises like handling JavaScript, meaning we may be able to execute on
certain plans *earlier* than we otherwise would have been able to had we stuck
with Cargo. In other words, the *path* to achieving difficult things is
unblocked, even if the road to get there still requires work. For more
information on this, see the section below titled "[Future endeavours](#future-endeavours)".

---

## Buck2 crash course

The following is an extremely minimal crash course in Buck2 concepts and how to
use it.

### `BUILD` files

### Target names

Buck2 is used to build **targets**, that exists in **packages**, which are part
of a **cell**. The most explicit syntax for referring to a target is the
following:

```text
cell//path/to/package:target-name
```

A cell is a short name that maps to a directory in the code repository. A
package is a subdirectory underneath the cell that contains the build rules for
the targets. A target is a buildable unit of code, like a binary or a library,
named in the `BUILD` file inside that package.

`buck2 build` works by giving it a target name, like the one above. The above is
an example of a "fully qualified" target name which is an unambiguous reference.

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

### Abstract targets & action graphs

NIH.

### Target visibility

Every target can have an associated *visibility list*, which restricts who is
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

Useful for downloading all dependencies, then testing clean build times afterwards.

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

This must be run from the root of the repository. Eyeball the output of `jj
diff` and make sure it looks fine, then test, before committing the changes.

This step will re-synchronize all `third-party//rust` crates with the versions
in the workspace Cargo file, and then also update the `BUILD` files in the
source code with any newly added build dependencies that were added or removed
(not just updated).

### `rust-analyzer` support

Coming soon.

---

## TODO + known Buck2 bugs

TODO list:

- [ ] Build time improvements
  - Clean from scratch build is still about 2x slower than `cargo`
  - Incremental rebuilds are quite comparable, though
- [ ] Investigate `rust-analyzer` support
  - nightly `rust-analyzer` with
    <https://github.com/rust-lang/rust-analyzer/pull/17246> required
  - some experiments have worked, and support is relatively close
- [ ] hermetic toolchain
  - [x] ~~system bootstrap python via <https://github.com/indygreg/python-build-standalone>~~
  - [ ] clang/lld
  - [ ] rustc
- [ ] remote caching
- [ ] remote execution
- macOS:
  - [ ] x86_64: get build working
    - mostly due to lack of an available x86_64 macOS machine
    - GHA x86_64 runners seem to be slow and have limited availability? 
  - [ ] get working in nix devShell, somehow
    - linking `libiconv` is an issue, as usual
    - requires the right shell settings, I assume
- Linux
  - [x] ~~aarch64-linux: get `bssl-sys` working with bindgen~~
    - workaround: aarch64-linux requires `bindgen` in `$PATH`, for now
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
  - Only happens when switching modes; incremental builds with the same mode
    are fine
  - Early cutoff kicks in so this only incurs a few seconds of extra time
    typically, because once Buck sees that the `.crate` files haven't actually
    changed it can quit early.

Upstream buck2 bugs:

- [x] ~~`buck2` aarch64-linux binaries don't with 16k page size <https://github.com/facebook/buck2/pull/693>~~
- [ ] Aggressively annoying download warnings <https://github.com/facebook/buck2/issues/316>
- RE/AC support:
  - [ ] Missing `ActionCache` support <https://github.com/facebook/buck2/pull/477>
  - [ ] File size logic bugs <https://github.com/facebook/buck2/pull/639>
  - [ ] Buggy concurrency limiter <https://github.com/facebook/buck2/pull/642>
  - [ ] Failure diagonstics <https://github.com/facebook/buck2/pull/656>
- `rust-analyzer`
  - [ ] Unbreak OSS usage of `rust-project` <https://github.com/facebook/buck2/pull/659>
- Miscellaneous
  - [ ] Distributing log files <https://github.com/facebook/buck2/issues/441>
    - Buck2 logs are included in CI artifacts, but not published anywhere

<!-- References -->

[Contributing]: https://martinvonz.github.io/jj/latest/contributing/
[Buck2]: https://buck2.build/
[Dotslash]: https://dotslash-cli.com/
