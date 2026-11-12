# Cutline – English

app-title = Cutline
menu = Menu
export = Export
coming-in-phase = Coming in phase { $phase }

tab-media = Media
tab-audio = Audio
tab-text = Text
tab-stickers = Stickers
tab-effects = Effects
tab-transitions = Transitions
tab-captions = Captions
tab-filters = Filters
tab-adjust = Adjust
tab-plugins = Plugins

import-media = Import media …
drop-hint = Drag and drop files here
media-empty = No media yet. Import videos, images or audio.
add-to-timeline = Add to timeline
media-probe-error = Could not read { $file }: { $error }
media-imported = { $count ->
    [one] Imported one item
   *[other] Imported { $count } items
}

text-add = Add text
text-default = Default text
font-library = Font library
import-fonts = Import font …
font-scope = Available in
font-scope-global = All projects
font-scope-project = This project only
fonts-count = { $count ->
    [one] One imported font
   *[other] { $count } imported fonts
}
system-fonts-count = { $count } system fonts available
font-imported = Imported font “{ $family }”
font-duplicate = This font is already in the library
font-error = Font not imported: { $error }
font-variable = Variable
font-color = Color
font-restricted = License does not allow embedding
font-remove = Remove
font-preview = Preview

player-title = Player – Timeline 01
play = Play
pause = Pause

inspector-project = Project
inspector-clip = Clip
project-name = Name
resolution = Resolution
frame-rate = Frame rate
aspect = Aspect ratio
color-space = Color space
magnet = Main track magnet
snapping = Snapping
clip-start = Start
clip-duration = Duration
clip-source = Source
clip-text = Text
clip-font = Font
clip-font-system = System default
clip-size = Font size
clip-uppercase = Uppercase
clip-opacity = Opacity
clip-volume = Volume
clip-scale = Scale
position = Position

timeline-empty = Drag items here to start editing
split = Split
delete = Delete
undo = Undo
redo = Redo
history = History
history-empty = No changes yet
source-ui = Interface
source-mcp = MCP: { $client }
source-plugin = Plugin: { $name }

mcp = MCP endpoint
mcp-off = MCP off
mcp-on = MCP active on 127.0.0.1:{ $port }
mcp-token = Access token
mcp-hint = Lets AI assistants read and edit this project. Every change is recorded in the history and can be undone.

plugins-loaded = { $count ->
    [one] One plugin loaded
   *[other] { $count } plugins loaded
}
plugin-effects = { $count } effects, transitions and filters

voiceover = Voiceover
language = Language
phase0-note = Phase 0: preview without sound and effects. Editing, text, fonts, undo and MCP are active.
