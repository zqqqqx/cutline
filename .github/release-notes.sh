#!/bin/sh
# Release text from CHANGELOG.md: the version's section, below it up to three older ones collapsed
# (the app shows these too when versions are skipped). Missing section: error.
# Usage: release-notes.sh <version> [CHANGELOG.md]   (pre-release 1.2.0-rc.1 falls back to 1.2.0)
set -eu
ver="${1#v}"
file="${2:-CHANGELOG.md}"
awk -v ver="$ver" -v base="${ver%%-*}" '
{ sub(/\r$/, "") }
/^## / {
  k = $2; gsub(/[][]/, "", k); sub(/^v/, "", k)
  if (!found && (k == ver || k == base)) { found = 1; on = 1; print; next }
  if (found && k ~ /^[0-9]+\.[0-9]/ && older < 3) {
    if (!older) printf "\n<details><summary>Earlier versions · Frühere Versionen</summary>\n\n"
    older++; on = 1; print; next
  }
  on = 0; next
}
on { print }
END {
  if (!found) { print "CHANGELOG.md: no section \"## " ver "\" – add the changes first" > "/dev/stderr"; exit 1 }
  if (older) printf "\n</details>\n"
}' "$file"
