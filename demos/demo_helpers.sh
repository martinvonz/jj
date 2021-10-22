#!/bin/bash
set -euo pipefail
BASE_DIR=$(realpath "$(dirname "$0")")

UPLOAD=false
PREVIEW=false
DEBUG=false
parse_args() {
    for arg in "$@"; do
        case "$arg" in
        -h|--help)
            echo 'Run a given demo.
Arguments:
  --preview: Preview the asciicast.
  --upload: Upload to asciinema (after previewing, if necessary).
  --debug: Show the asciicast as it is being recorded. Note that what you see
           will not be exactly the same as what is recorded.
'
            exit
            ;;
        --upload)
            UPLOAD=true
            ;;
        --preview)
            PREVIEW=true
            ;;
        --debug)
            DEBUG=true
            ;;
        *)
            echo "Unrecognized argument: $arg"
            exit 1
            ;;
        esac
    done
}

new_tmp_dir() {
    local dirname
    dirname=$(mktemp -d)
    mkdir -p "$dirname"
    cd "$dirname"
    trap "rm -rf '$dirname'" EXIT
}

run_demo() {
    local expect_script="$1"
    expect_script=$(printf "source $BASE_DIR/demo_helpers.tcl
spawn asciinema rec -c \"PS1='$ ' bash --norc\"
expect_prompt
%s
quit_and_dump_asciicast_path
" "$expect_script")
     
    if [[ "$DEBUG" == true ]]; then
        echo "$expect_script" | /usr/bin/env expect
        return
    fi

    echo "Recording demo (terminal size is $(tput cols)x$(tput lines))..."
    if [[ "$PREVIEW" == 'false' ]]; then
        echo '(Pass --preview to play the demo automatically once done)'
    fi
    local asciicast_path
    asciicast_path=$(echo "$expect_script" | /usr/bin/env expect | tail -1)
    echo "$asciicast_path"

    if [[ "$PREVIEW" == 'true' ]]; then
        asciinema play "$asciicast_path"
    fi
    if [[ "$UPLOAD" == 'true' ]]; then
        if [[ "$PREVIEW" == 'true' ]] && ! confirm "Upload?"; then
            return
        fi
        : asciinema upload "$asciicast_path"
    fi
}
