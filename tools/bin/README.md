# `dotslash` executables

This directory contains various [DotSlash](https://github.com/facebook/dotslash)
executables: portable executables that are downloaded on demand.

DotSlash lets us have our tools available on any platform without the need to
install them, in a version-controllable and repeatable way that doesn't bloat
repositories. This helps make sure developers can have consistent environments,
if they wish to opt in.

- [Install DotSlash](https://dotslash-cli.com/docs/installation/)
  - TL;DR cargo users: `cargo install dotslash`
  - TL;DR nix users: `nix profile install nixpkgs#dotslash`
  - TL;DR everyone else: [Download the latest release](https://github.com/facebook/dotslash/releases)

> [!TIP]
> DotSlash files are most useful for cross-platform tools we want to provide
> _developers_ on _all_ platforms &mdash; including Windows! Some other tools
> may also be provided by e.g. Nix or Cargo.

Once `dotslash` is somewhere in your `$PATH`, add these files to your `$PATH`
too:

```bash
export PATH=$(jj root)/tools/bin:$PATH
```

Then tools like `diffedit3` will work with a small one-time startup penalty to
download the executable.

If you're curious, just open any of the DotSlash files in this directory in
your EDITOR; they are merely simple JSON files.

## Windows users

Windows users need to invoke the path to the dotslash file with the `dotslash`
command:

```
dotslash .\tools\bin\diffedit3
```

This is a technical limitation that will be alleivated in the future (once the
`windows_shim` for DotSlash is released.)