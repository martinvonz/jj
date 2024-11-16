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

Once `dotslash` is somewhere in your `$PATH`, add these files to your `$PATH`
too:

```bash
export PATH=$(jj root)/tools/bin:$PATH
```

Then tools like `buck2`, `reindeer`, and `diffedit3` will work with a small
one-time startup penalty to download the executable.

## Upgrading dotslash files

To be written...
