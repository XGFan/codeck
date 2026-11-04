#!/bin/sh
# Links the repo's hooks into the git hooks directory (a global core.hooksPath dispatcher that
# forwards to <git-common-dir>/hooks finds them there too).
set -e
root=$(git rev-parse --show-toplevel)
hooks=$(git rev-parse --git-common-dir)/hooks
mkdir -p "$hooks"
for h in "$root"/scripts/git-hooks/*; do
  ln -sf "$h" "$hooks/$(basename "$h")"
  echo "installed $(basename "$h")"
done
