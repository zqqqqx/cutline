# Cutline – Deutsch
# Glossar: Frame = Einzelbild (nie „Rahmen“), Clip, Spur, Zeitleiste, Übergang,
# Keyframe, Mitteltöne (nie „Mittelgrau“), „Frei“ beim Zuschneiden (nie „Kostenlos“).

app-title = Cutline
menu = Menü
export = Exportieren
coming-in-phase = Kommt in Phase { $phase }

tab-media = Medien
tab-audio = Audio
tab-text = Text
tab-stickers = Sticker
tab-effects = Effekte
tab-transitions = Übergänge
tab-captions = Untertitel
tab-filters = Filter
tab-adjust = Anpassen
tab-plugins = Plugins

import-media = Medien importieren …
drop-hint = Dateien hierher ziehen und ablegen
media-empty = Noch keine Medien. Importiere Videos, Bilder oder Audio.
add-to-timeline = Zur Zeitleiste hinzufügen
media-probe-error = { $file } konnte nicht gelesen werden: { $error }
media-imported = { $count ->
    [one] Ein Medium importiert
   *[other] { $count } Medien importiert
}

text-add = Text hinzufügen
text-default = Standardtext
font-library = Schriftbibliothek
import-fonts = Schrift importieren …
font-scope = Gültig für
font-scope-global = Alle Projekte
font-scope-project = Nur dieses Projekt
fonts-count = { $count ->
    [one] Eine importierte Schrift
   *[other] { $count } importierte Schriften
}
system-fonts-count = { $count } Systemschriften verfügbar
font-imported = Schrift „{ $family }“ importiert
font-duplicate = Diese Schrift ist bereits in der Bibliothek
font-error = Schrift nicht importiert: { $error }
font-variable = Variabel
font-color = Farbe
font-restricted = Lizenz erlaubt keine Einbettung
font-remove = Entfernen
font-preview = Vorschau

player-title = Player – Zeitleiste 01
play = Abspielen
pause = Pause

inspector-project = Projekt
inspector-clip = Clip
project-name = Name
resolution = Auflösung
frame-rate = Bildrate
aspect = Seitenverhältnis
color-space = Farbraum
magnet = Hauptspur-Magnet
snapping = Einrasten
clip-start = Start
clip-duration = Dauer
clip-source = Quelle
clip-text = Text
clip-font = Schriftart
clip-font-system = Systemstandard
clip-size = Schriftgröße
clip-uppercase = Großbuchstaben
clip-opacity = Deckkraft
clip-volume = Lautstärke
clip-scale = Größe
position = Position

timeline-empty = Elemente hierher ziehen, um mit dem Schnitt zu beginnen
split = Teilen
delete = Löschen
undo = Rückgängig
redo = Wiederholen
history = Verlauf
history-empty = Noch keine Änderungen
source-ui = Oberfläche
source-mcp = MCP: { $client }
source-plugin = Plugin: { $name }

mcp = MCP-Endpunkt
mcp-off = MCP aus
mcp-on = MCP aktiv auf 127.0.0.1:{ $port }
mcp-token = Zugriffstoken
mcp-hint = Erlaubt KI-Assistenten, dieses Projekt zu lesen und zu bearbeiten. Jede Änderung landet im Verlauf und lässt sich rückgängig machen.

plugins-loaded = { $count ->
    [one] Ein Plugin geladen
   *[other] { $count } Plugins geladen
}
plugin-effects = { $count } Effekte, Übergänge und Filter

voiceover = Einsprechen
language = Sprache
phase0-note = Phase 0: Vorschau ohne Ton und Effekte. Schnitt, Text, Schriften, Undo und MCP sind aktiv.
