# Screenshots with demos and scripts to generate them

The `demo_*.sh` scripts in this directory demo
various features of `jj`.

The `run_scripts.sh` script can be used to:

- Run them inside a standardized environment instead of the user's local
  environment.
- Generate SVG and PNG images for the scripts.

The PNG images in the repo may be slightly older, as they take up more space in
the repo.

The SVG images have human-readable diffs, but may look different on different
computers with different fonts installed.

## Running `run_scripts.sh`

This requires ImageMagick and `term-transcript-cli` to be installed. See
`run_scripts.sh`'s error messages for some more details. On Debian Linux, it
also seems helpful to `sudo apt install inkscape`; ImageMagick seems to use
either Inkscape itself or some dependency of it.

One way to make all the images and check the output is:

```shell
cd demos
./run_scripts.sh demo_*.sh |less
```

### A note on fonts

The exact PNG output depends on the fonts you have installed on your system.

The screenshots are usually generated on a Debian Linux system and use the "Fira
Code" font. It can be installed with `sudo apt install fonts-firacode`. It seems
to include all relevant Unicode symbols and be a little bolder and thus more
readable than the "Liberation Mono" font, which is used if Fira Code is not
installed. That font also works OK. See the CSS font specification in
`run_scripts.sh` for other fonts tried (especially when viewing SVGs on the
web). If none apply, the default `monospace` font will be used.

`convert -list Fonts` will list the fonts ImageMagick is aware of.
