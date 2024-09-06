#!/bin/bash
set -euo pipefail
. "$(dirname "$0")"/setup_standard_config.sh

# NOTE ON TOOLS: the only two nice tools I'm aware of that support our use-case
# (non-animated svgs) are https://github.com/slowli/term-transcript and
# https://github.com/FHPythonUtils/AnsiToImg. They have slightly different
# limitations and advantages. If one of them stops being developed, we could
# look at the other one.
#
# AnsiToImg can also generate PNGs, but that is currently harder to setup than
# `magick`. `magick` supports different backends. I do not completely
# understand them. On Debian, it did not work well without `sudo apt install
# inkscape`. It's unclear to me whether `magick` used Inkscape or one of its
# dependencies. Inkscape can be also used manually for SVG -> PNG conversion.
which term-transcript > /dev/null \
  || (echo '`term-transcript` must be installed with e.g.'\
           '`cargo binstall term-transcript-cli`.' \
           'See also https://github.com/slowli/term-transcript' >&2;
      false)
which magick > /dev/null \
  || echo '`magick` from ImageMagick needs to be installed to create pngs.' \
          'Only svgs will be created.' >&2

echo "jj --version: (set PATH to change)"
jj --version

# Make `jj` wrap text as opposed to `term-transcript`. `term-transcript` wraps
# at 80 columns. Also, 80 seems to be the maximum number of columns that's
# somewhat readable on mobile devices.
#
# Note that `bash` likes to reset the value of $COLUMNS, so we use a different
# variable here that is interpreted by `run_command()` in `helpers.sh`.
RUN_COMMAND_COLUMNS=80
export RUN_COMMAND_COLUMNS

run_script_through_term_transcript_and_pipe_result_to_stderr() {
  script="$1"
  script_base="${script%.sh}"
  script_base="${script_base#demo_}"
  outfile=$(mktemp --tmpdir "$script_base"-output-XXXX.ansi)
  # We use `term-transcript capture` instead of `term-transcript exec` so that
  # we can show the output of the script via `tee`.
  (bash "$script" || (echo "SCRIPT FAILED WITH EXIT CODE $?"; false)) 2>&1 | \
    tee "$outfile"
  term-transcript capture \
      --no-inputs --pure-svg --palette powershell \
      --font "Fira Code, Liberation Mono, SFMono-Regular, Consolas, Menlo" \
      --out "$script_base".svg "$script_base" < "$outfile"
  # The default font choice term-transcript would make is:
  #     SFMono-Regular, Consolas, Liberation Mono, Menlo
  # We add the fonts that were checked and seem to contain all the relevant
  # unicode in front.
  rm "$outfile"
}

for script in "$@"; do
  run_script_through_term_transcript_and_pipe_result_to_stderr "$script" 2>&1
  # By default, 1 SVG unit becomes 1 pixel. The term-transcript output
  # defaults to 720 SVG units width.
  #
  # `-background black` is important because the SVGs use transparency to make
  # rounded corners, and the transparent portion becomes white by default.
  # TODO(ilyagr): Figure out if `magick` can make PNGs have transparency.
  #
  # `-resize 100%` is a no-op. `-resize 700x10000`` would make the width 700 px
  # and preserve aspect ratio.
  which magick > /dev/null \
    && magick "$script_base".svg \
            -colors 63 -background black -resize 100%  \
            "$script_base".png \
    || true
  # TODO/FIXME: The above command doesn't seem to work properly;
  # the PNG files end up larger than they should be and are RGB
  # as opposed to expected indexed 63-color. This is caused by
  # https://github.com/martinvonz/jj/commit/6d573ef6d7a45151495de18b6f4c5063ce39f6bd
  # and
  # https://github.com/martinvonz/jj/commit/42dee7d08ce8e362cf9d44f844b25e001b6ac94f
  # and needs debugging of ImageMagick invocations.
done
