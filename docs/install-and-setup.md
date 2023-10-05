# Installation and setup


## Installation

### Download pre-built binaries for a release

There are [pre-built binaries](https://github.com/martinvonz/jj/releases/latest)
of the last released version of `jj` for Windows, Mac, or Linux (the "musl"
version should work on all distributions).

If you'd like to install a prerelease version, you'll need to use one of the
options below.


### Linux

#### Build using `cargo`

First make sure that you have the `libssl-dev`, `openssl`, `pkg-config`, and
`build-essential` packages installed by running something like this:

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

### Mac

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

#### From Source

You may need to run some or all of these:

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

### Windows

Run either:

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
`jj util completion --bash/--zsh/--fish`. Exactly how to source it
depends on your shell.

### Bash

```shell
source <(jj util completion)  # --bash is the default
```

### Zsh

```shell
autoload -U compinit
compinit
source <(jj util completion --zsh)
```

### Fish

```shell
jj util completion --fish | source
```

### Xonsh

```shell
source-bash $(jj util completion)
```
