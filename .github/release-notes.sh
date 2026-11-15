#!/bin/sh
# Release-Text aus CHANGELOG.md: Abschnitt der Version, darunter eingeklappt bis zu drei ältere
# (die App zeigt beim Überspringen von Versionen auch diese). Fehlt der Abschnitt: Fehler.
# Aufruf: release-notes.sh <version> [CHANGELOG.md]   (Vorabversion 1.2.0-rc.1 nimmt sonst 1.2.0)
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
  if (!found) { print "CHANGELOG.md: kein Abschnitt \"## " ver "\" – erst die Änderungen eintragen" > "/dev/stderr"; exit 1 }
  if (older) printf "\n</details>\n"
}' "$file"
