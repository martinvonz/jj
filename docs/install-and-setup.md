# Installation and setup


## Installation

See below for how to build from source. There are also
[pre-built binaries](https://github.com/martinvonz/jj/releases) for Windows,
Mac, or Linux (musl).

### Linux

On most distributions, you'll need to build from source using `cargo` directly.

#### Build using `cargo`

First make sure that you have the `libssl-dev`, `openssl`, and `pkg-config`
packages installed by running something like this:

```shell script
sudo apt-get install libssl-dev openssl pkg-config
```

Now run:

```shell script
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli
```

#### Nix OS

If you're on Nix OS you can use the flake for this repository.
For example, if you want to run `jj` loaded from the flake, use:

```shell script
nix run 'github:martinvonz/jj'
```

You can also add this flake url to your system input flakes. Or you can
install the flake to your user profile:

```shell script
nix profile install 'github:martinvonz/jj'
```

#### Homebrew

If you use linuxbrew, you can run:

```shell script
brew install jj
```

### Mac

#### Homebrew

If you use Homebrew, you can run:

```shell script
brew install jj
```

#### MacPorts

You can also install `jj` via [MacPorts](https://www.macports.org) (as
the `jujutsu` port):

```shell script
sudo port install jujutsu
```

([port page](https://ports.macports.org/port/jujutsu/))

#### From Source

You may need to run some or all of these:

```shell script
xcode-select --install
brew install openssl
brew install pkg-config
export PKG_CONFIG_PATH="$(brew --prefix)/opt/openssl@3/lib/pkgconfig"
```

Now run:

```shell script
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli
```

### Windows

Run:

```shell script
cargo install --git https://github.com/martinvonz/jj.git --locked --bin jj jj-cli --features vendored-openssl
```

## Initial configuration

You may want to configure your name and email so commits are made in your name.

```shell script
$ jj config set --user user.name "Martin von Zweigbergk"
$ jj config set --user user.email "martinvonz@google.com"
```

## Command-line completion

To set up command-line completion, source the output of
`jj util completion --bash/--zsh/--fish` (called `jj debug completion` in
jj <= 0.7.0). Exactly how to source it depends on your shell.

### Bash

```shell script
source <(jj util completion)  # --bash is the default
```

Or, with jj <= 0.7.0:

```shell script
source <(jj debug completion)  # --bash is the default
```

### Zsh

```shell script
autoload -U compinit
compinit
source <(jj util completion --zsh)
```

Or, with jj <= 0.7.0:

```shell script
autoload -U compinit
compinit
source <(jj debug completion --zsh)
```

### Fish

```shell script
jj util completion --fish | source
```

Or, with jj <= 0.7.0:

```shell script
jj debug completion --fish | source
```

### Xonsh

```shell script
source-bash $(jj util completion)
```

Or, with jj <= 0.7.0:

```shell script
source-bash $(jj debug completion)
```

