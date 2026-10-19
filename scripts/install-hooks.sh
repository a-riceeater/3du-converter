#!/bin/sh
# Points Git at the repository's hooks so the private-data check runs before every commit.
set -eu
root=$(git rev-parse --show-toplevel)
git -C "$root" config core.hooksPath .githooks
chmod +x "$root/.githooks/pre-commit" "$root/scripts/check-private-data.sh" 2>/dev/null || true
echo "Installed: pre-commit now runs scripts/check-private-data.sh"
