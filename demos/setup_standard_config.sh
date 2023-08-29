# This script is meant to be run with `source` from a fresh bash
# shell. Thus no shebang.
# shellcheck shell=bash
set -euo pipefail

JJ_CONFIG=$(mktemp --tmpdir jjconfig-XXXX.toml)
export JJ_CONFIG
cat <<'EOF' > "$JJ_CONFIG"
[user]
name = "JJ Fan"
email = "jjfan@example.com"

[operation]
hostname = "jujube"
username = "jjfan"

[ui]
color="always"
paginate="never"
log-word-wrap=true  # Need to set COLUMNS for this to work
EOF

GIT_CONFIG_GLOBAL=$(mktemp --tmpdir gitconfig-XXXX)
export GIT_CONFIG_GLOBAL
cat <<'EOF' > "$GIT_CONFIG_GLOBAL"
[color]
ui=always
EOF
