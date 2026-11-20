#!/bin/sh
# changes.json from CHANGELOG.md: changes of the version and all earlier ones, both languages,
# without Markdown (the app reads it for "What's new" instead of the release text). Same shape as
# `cl_update::changelog::Notes`; `format` only goes up on incompatible changes:
#   {"format": 1, "versions": [{"version": "0.12.1", "date": "2026-11-17",
#     "en": [{"group": "Fixed", "items": [{"title": "Title.", "text": "Sentence."}]}], "de": [...]}]}
# Parsed like `changelog::parse` (test `changes_json_script_matches_the_changelog`).
# Usage: changes-json.sh <version> [CHANGELOG.md]   (pre-release 1.2.0-rc.1 falls back to 1.2.0)
set -eu
ver="${1#v}"
file="${2:-CHANGELOG.md}"
awk -v ver="$ver" -v base="${ver%%-*}" '
function trim(s) { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
# JSON string; char by char instead of gsub (backslashes in gsub behave differently per awk)
function q(s,   o, i, c) {
  o = ""
  for (i = 1; i <= length(s); i++) {
    c = substr(s, i, 1)
    if (c == "\\") o = o "\\\\"
    else if (c == "\"") o = o "\\\""
    else if (c == "\t") o = o "\\t"
    else o = o c
  }
  return "\"" o "\""
}
function flush_entry() {
  if (!epend) return
  gitems = gitems (gitems == "" ? "" : ",") "\n    {\"title\": " q(etitle) ", \"text\": " q(etext) "}"
  epend = 0
}
function flush_group() {
  flush_entry()
  if (!gopen) return
  g[lang] = g[lang] (g[lang] == "" ? "" : ",") "\n   {\"group\": " q(gtitle) ", \"items\": [" gitems (gitems == "" ? "" : "\n   ") "]}"
  gopen = 0; gitems = ""
}
function flush_version() {
  flush_group()
  if (!vopen) return
  if (!nv) printf "{\"format\": 1, \"versions\": ["
  printf "%s\n {\"version\": %s, \"date\": %s,\n  \"en\": [%s],\n  \"de\": [%s]}", (nv++ ? "," : ""), q(vname), q(vdate), g["en"], g["de"]
  vopen = 0
}
{ sub(/\r$/, ""); raw = $0; t = trim($0) }
t ~ /^## / {
  flush_version()
  n = split(trim(substr(t, 4)), w, /[ \t]+/)
  k = w[1]; gsub(/[][]/, "", k); sub(/^[vV]/, "", k)
  if (!found && (k == ver || k == base)) found = 1
  if (found && k ~ /^[0-9]+\.[0-9]/) {
    vopen = 1; vname = k; vdate = ""; g["en"] = ""; g["de"] = ""; lang = "en"
    for (i = 1; i <= n; i++) if (w[i] ~ /^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$/) { vdate = w[i]; break }
  }
  next
}
!vopen { next }
t ~ /^### / {
  flush_group()
  h = tolower(trim(substr(t, 5)))
  lang = (h == "deutsch" || h == "de" || h == "german") ? "de" : "en"
  next
}
# Group: **New** or #### New
(length(t) >= 4 && t ~ /^\*\*.*\*\*$/ && index(substr(t, 3, length(t) - 4), "**") == 0) || t ~ /^#### / {
  x = (t ~ /^#### /) ? substr(t, 6) : substr(t, 3, length(t) - 4)
  x = trim(x); sub(/:+$/, "", x); x = trim(x)
  if (x != "") { flush_group(); gopen = 1; gtitle = x; next }
}
t ~ /^[-*] / {
  flush_entry()
  if (!gopen) { gopen = 1; gtitle = "" }
  it = trim(substr(t, 3)); etitle = ""; etext = it
  if (substr(it, 1, 2) == "**") {
    r = substr(it, 3); p = index(r, "**")
    if (p) { etitle = trim(substr(r, 1, p - 1)); etext = trim(substr(r, p + 2)) }
  }
  epend = 1
  next
}
# indented continuation of the last entry
t != "" && raw ~ /^[ \t]/ && epend { etext = trim(etext " " t) }
END {
  if (!found) { print "CHANGELOG.md: no section \"## " ver "\" – add the changes first" > "/dev/stderr"; exit 1 }
  flush_version()
  print "\n]}"
}' "$file"
