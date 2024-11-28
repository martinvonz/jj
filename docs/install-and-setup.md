# Installation and setup


## Installation

### Download pre-built binaries for a release

There are [pre-built binaries](https://github.com/martinvonz/jj/releases/latest)
of the last released version of `jj` for Windows, Mac, or Linux (the "musl"
version should work on all distributions).

If you'd like to install a prerelease version, you'll need to use one of the
options below.

#### Cargo Binstall

If you use [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall), you
can install the same binaries of the last `jj` release from GitHub as follows:

```shell
# Will put the jj binary for the latest release in ~/.cargo/bin by default
cargo binstall --strategies crate-meta-data jj-cli
```

Without the `--strategies` option, you may get equivalent binaries that should
be compiled from the same source code.


### Linux

#### From Source

First make sure that you have a Rust version >= 1.76 and that the `libssl-dev`,
`openssl`, `pkg-config`, and `build-essential` packages are installed by running
something like this:

```shell
sudo apt-get install libssl-dev openssl pkg-config build-essential
```

Now run either:

```shell
# To install the *prerelease* version from the main branch
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli
```

or:

```shell
# To install the latest release
cargo install --locked --bin jj jj-cli
```

#### Arch Linux
You can install the `jujutsu` package from the [official extra repository](https://archlinux.org/packages/extra/x86_64/jujutsu/):

```
pacman -S jujutsu
```

Or install from the [AUR repository](https://aur.archlinux.org/packages/jujutsu-git) with an [AUR Helper](https://wiki.archlinux.org/title/AUR_helpers):

```
yay -S jujutsu-git
```

#### Nix OS

If you're on Nix OS you can install a **released** version of `jj` using the
[nixpkgs `jujutsu` package](https://search.nixos.org/packages?channel=unstable&show=jujutsu).

To install a **prerelease** version, you can use the flake for this repository.
For example, if you want to run `jj` loaded from the flake, use:

```shell
nix run 'github:martinvonz/jj'
```

You can also add this flake url to your system input flakes. Or you can
install the flake to your user profile:

```shell
# Installs the prerelease version from the main branch
nix profile install 'github:martinvonz/jj'
```

#### Homebrew

If you use linuxbrew, you can run:

```shell
# Installs the latest release
brew install jj
```

#### Gentoo Linux

`dev-vcs/jj` is available in the [GURU](https://wiki.gentoo.org/wiki/Project:GURU) repository.
Details on how to enable the GURU repository can be found [here](https://wiki.gentoo.org/wiki/Project:GURU/Information_for_End_Users).

Once you have synced the GURU repository, you can install `dev-vcs/jj` via Portage:


```
emerge -av dev-vcs/jj
```

### Mac

#### From Source, Vendored OpenSSL

First make sure that you have a Rust version >= 1.76. You may also need to run:

```shell
xcode-select --install
```

Now run either:

```shell
# To install the *prerelease* version from the main branch
cargo install --git https://github.com/martinvonz/jj.git \
     --features vendored-openssl --locked --bin jj jj-cli
```

or:

```shell
# To install the latest release
cargo install --features vendored-openssl --locked --bin jj jj-cli
```

#### From Source, Homebrew OpenSSL

First make sure that you have a Rust version >= 1.76. You will also need
[Homebrew](https://brew.sh/) installed. You may then need to run some or all of
these:

```shell
xcode-select --install
brew install openssl
brew install pkg-config
export PKG_CONFIG_PATH="$(brew --prefix)/opt/openssl@3/lib/pkgconfig"
```

Now run either:

```shell
# To install the *prerelease* version from the main branch
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli
```

or:

```shell
# To install the latest release
cargo install --locked --bin jj jj-cli
```


#### Homebrew

If you use Homebrew, you can run:

```shell
# Installs the latest release
brew install jj
```

#### MacPorts

You can also install `jj` via [the MacPorts `jujutsu`
port](https://ports.macports.org/port/jujutsu/):

```shell
# Installs the latest release
sudo port install jujutsu
```

### Windows

First make sure that you have a Rust version >= 1.76. Now run either:

```shell
# To install the *prerelease* version from the main branch
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli --features vendored-openssl
```

or:

```shell
# To install the latest release
cargo install --locked --bin jj jj-cli --features vendored-openssl
```


## Initial configuration

You may want to configure your name and email so commits are made in your name.

```shell
$ jj config set --user user.name "Martin von Zweigbergk"
$ jj config set --user user.email "martinvonz@google.com"
```

## Command-line completion

To set up command-line completion, source the output of
`jj util completion bash/zsh/fish`. Exactly how to source it
depends on your shell.

Improved completions are also available. They will complete things like
bookmarks, aliases, revisions, operations and files. They can be context aware,
for example they respect the global flags `--repository` and `--at-operation` as
well as some command-specific ones like `--revision`, `--from` and `--to`. You
can activate them with the alternative "dynamic" instructions below. They should
still complete everything the static completions did, so only activate one of
them. Please let us know if you encounter any issues, so we can ensure a smooth
transition once we default to these new completions. Our initial experience
is that these new completions work best with `fish`. If you have ideas about
specific completions that could be added, please share them
[here](https://github.com/martinvonz/jj/issues/4763).

### Bash

```shell
source <(jj util completion bash)
```

dynamic:

```shell
source <(COMPLETE=bash jj)
```

### Zsh

```shell
autoload -U compinit
compinit
source <(jj util completion zsh)
```

dynamic:

```shell
source <(COMPLETE=zsh jj)
```

### Fish

```shell
jj util completion fish | source
```

dynamic:

```shell
COMPLETE=fish jj | source
```

### Nushell

```nu
jj util completion nushell | save completions-jj.nu
use completions-jj.nu *  # Or `source completions-jj.nu`
```

(dynamic completions not available yet)

### Xonsh

```shell
source-bash $(jj util completion)
```

(dynamic completions not available yet)
