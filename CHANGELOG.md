# Changelog

<!-- Format: one section per version `## X.Y.Z – YYYY-MM-DD` with `### English` and `### Deutsch`, groups `**New**`/`**Improved**`/`**Fixed**` (`**Neu**`/`**Verbessert**`/`**Behoben**`), entries `- **Title.** Sentence.` or `- Sentence.`
     Add changes users notice under `## Unreleased`; a release renames it to `## X.Y.Z – date`. Cutline and the GitHub release notes read exactly this layout. -->

## Unreleased

### English

**New**

**Improved**

**Fixed**

### Deutsch

**Neu**

**Verbessert**

**Behoben**

## 0.11.1 – 2026-11-15

### English

**New**
- **Hide content packs.** Under Settings → Content, the eye button hides a pack from all tabs while keeping it active – projects that use it keep working. Also works for the built-in essentials.
- **Sound effects as compact tiles.** More sounds per page, each with a small waveform. Click to listen; the waveform fills as it plays. Waveforms are there instantly.
- **Audio preview in the Media tab.** Audio files show their waveform and play on click.
- **Your own order.** Rearrange favourites and your own folders with the pencil button. In built-in folders, right-click → "Pin to top".
- **Save media as element or sound.** Right-click an image, GIF or audio file in the Media tab and save it to a folder of the Elements or Sounds tab – or create a new folder right there.
- **Paste goes where you expect.** Pasting lands in the track of the copied clip, or in the track you clicked afterwards, at the playhead. Later clips in that track move to make room.
- **Chapters are optional.** Chapter export is off by default (Settings → Projects → Export). The hint about chapters shorter than 10 s can be dismissed.

**Improved**
- The preview now shows transitions the way the export does – also after masked, zoomed or effect-heavy clips – and plays through them more smoothly.
- Text templates are readable in the light theme.
- A double-click inserts only once. Undo also removes a track that was created together with a clip. Esc leaves the reorder mode.
- Many loud sounds at once no longer clip in the exported file.

**Fixed**
- The project name in the Project panel can be edited again.
- "Add to library" now accepts every format the import supports (e.g. OGV, MTS, WMV).
- Several cut-off names and small display glitches.

### Deutsch

**Neu**
- **Pakete ausblenden.** Unter Einstellungen → Inhalte blendet das Auge ein Paket aus allen Tabs aus, es bleibt aber aktiv – Projekte, die es nutzen, funktionieren weiter. Geht auch für die Grundausstattung.
- **Soundeffekte als kompakte Kacheln.** Mehr Sounds auf einer Seite, jede Kachel mit kleiner Wellenform. Klick zum Anhören; die Wellenform füllt sich beim Abspielen. Die Wellenformen sind sofort da.
- **Audiovorschau im Medien-Tab.** Ton-Dateien zeigen ihre Wellenform und spielen per Klick.
- **Eigene Reihenfolge.** Favoriten und eigene Ordner lassen sich über den Stift umsortieren. In vorhandenen Ordnern: Rechtsklick → „Oben anpinnen“.
- **Medien als Element oder Sound speichern.** Rechtsklick auf ein Bild, GIF oder eine Ton-Datei im Medien-Tab speichert sie in einen Ordner des Elemente- oder Sounds-Tabs – auf Wunsch gleich in einen neuen.
- **Einfügen landet, wo man es erwartet.** Eingefügt wird in der Spur des kopierten Clips bzw. in der danach angeklickten Spur, an der Playhead-Position. Spätere Clips dieser Spur rücken zur Seite.
- **Kapitel nur auf Wunsch.** Der Kapitel-Export ist standardmäßig aus (Einstellungen → Projekte → Exportieren). Der Hinweis zu Kapiteln unter 10 s lässt sich wegklicken.

**Verbessert**
- Die Vorschau zeigt Übergänge wie der Export – auch nach maskierten, gezoomten oder effektlastigen Clips – und läuft dabei flüssiger.
- Textvorlagen sind im hellen Design lesbar.
- Ein Doppelklick fügt nur noch einmal ein. Rückgängig entfernt auch eine Spur, die zusammen mit einem Clip entstanden ist. Esc beendet den Bearbeiten-Modus.
- Viele laute Sounds gleichzeitig übersteuern in der exportierten Datei nicht mehr.

**Behoben**
- Der Projektname im Projekt-Panel lässt sich wieder ändern.
- „Zur Bibliothek hinzufügen“ nimmt jetzt jedes Format, das der Import kennt (z. B. OGV, MTS, WMV).
- Mehrere abgeschnittene Namen und kleine Darstellungsfehler.

## 0.11.0 – 2026-11-15

### English

**New**
- **Content packs.** One catalog for all effects, filters, transitions, sounds and elements. Switch packs on and off under Settings → Content, with sources and licenses for everything.
- **Elements tab.** The Stickers tab is now called Elements, with new folders such as calls to action, reactions, speech bubbles, frames, shapes and particles – plus 34 animated elements and 138 emoji.
- **Shader transitions.** 31 new transitions rendered on the graphics card. They look the same in the preview and in the exported video.
- **More effects, filters and sounds.** 19 new effects, 15 colour looks and 85 new sound effects.
- **Text animations.** 16 new in and out animations, 9 combined animations, 8 caption styles, 17 text templates and 8 fonts.
- **Animated images.** GIF and APNG files play as image clips – in the preview and in the export.

**Improved**
- The preview shows built-in transitions exactly like the export.
- Playback starts smoothly: pressing play no longer freezes the window for a moment.
- Cutline uses less processor time and memory while you hover, scrub and switch projects.
- Settings can be reset per section, and plugins now live in the settings.
- A visible focus frame when you use the keyboard.

**Fixed**
- Exports of very short projects, empty ranges and unusual values in project files no longer fail.
- Subtitle import reads more VTT files (without hours, with cue settings, Latin-1).
- A damaged video no longer stalls the creation of preview files.
- Many cut-off names, missing translations and small display glitches.

### Deutsch

**Neu**
- **Inhaltspakete.** Ein Katalog für alle Effekte, Filter, Übergänge, Sounds und Elemente. Pakete schaltest du unter Einstellungen → Inhalte ein und aus – mit Quellen und Lizenzen zu allem.
- **Elemente-Tab.** Der Sticker-Tab heißt jetzt Elemente, mit neuen Ordnern wie Aufrufe, Reaktionen, Sprechblasen, Rahmen, Formen und Partikel – dazu 34 bewegte Elemente und 138 Emoji.
- **Shader-Übergänge.** 31 neue Übergänge, berechnet auf der Grafikkarte. Sie sehen in der Vorschau genauso aus wie im fertigen Video.
- **Mehr Effekte, Filter und Sounds.** 19 neue Effekte, 15 Farblooks und 85 neue Soundeffekte.
- **Text-Animationen.** 16 neue Ein- und Ausblendungen, 9 Kombi-Animationen, 8 Untertitel-Stile, 17 Textvorlagen und 8 Schriften.
- **Bewegte Bilder.** GIF- und APNG-Dateien laufen als Bild-Clip – in der Vorschau und im Export.

**Verbessert**
- Die Vorschau zeigt eingebaute Übergänge genau wie der Export.
- Die Wiedergabe startet flüssig: Abspielen hält das Fenster nicht mehr kurz an.
- Cutline braucht weniger Rechenzeit und Speicher beim Zeigen, Scrubben und Projektwechsel.
- Einstellungen lassen sich je Bereich zurücksetzen, Plugins stehen jetzt in den Einstellungen.
- Ein sichtbarer Fokusrahmen bei Bedienung mit der Tastatur.

**Behoben**
- Exporte sehr kurzer Projekte, leerer Bereiche und ungewöhnlicher Werte in Projektdateien schlagen nicht mehr fehl.
- Der Untertitel-Import liest mehr VTT-Dateien (ohne Stunden, mit Cue-Einstellungen, Latin-1).
- Ein beschädigtes Video hält das Erzeugen der Vorschaudateien nicht mehr an.
- Viele abgeschnittene Namen, fehlende Übersetzungen und kleine Darstellungsfehler.

## 0.10.0 – 2026-11-15

### English

**New**
- **Arrange your workspace.** Swap the panels, put the timeline on top, place the media icon bar at the side or on top – and lock the layout.
- **Live preview while exporting.** The export dialog shows the frame being rendered.

**Improved**
- Hint texts and status colours are readable in all four themes.
- All dialogs look and behave the same; Esc closes every one of them.
- Tooltips show your own key bindings, and undo steps have names.

### Deutsch

**Neu**
- **Arbeitsfläche anordnen.** Bereiche tauschen, Zeitleiste nach oben, Medien-Symbolleiste seitlich oder oben – und das Layout sperren.
- **Live-Vorschau beim Export.** Der Export-Dialog zeigt das gerade berechnete Bild.

**Verbessert**
- Hinweistexte und Statusfarben sind in allen vier Designs lesbar.
- Alle Dialoge sehen gleich aus und verhalten sich gleich; Esc schließt jeden.
- Tooltips zeigen deine eigenen Tastenkürzel, Rückgängig-Schritte haben Namen.

## 0.9.1 – 2026-11-14

### English

**Improved**
- AV1 export works with the FFmpeg that comes with Cutline.

**Fixed**
- Export: AV1 bitrate, side files with an In/Out range, loudness of very loud audio and speeds down to 0.01×. Enter no longer skips the overwrite question.
- Shorts and templates keep overlays as overlays; Quick Actions handle reversed clips and speed curves.

### Deutsch

**Verbessert**
- AV1-Export klappt mit dem FFmpeg, das Cutline mitbringt.

**Behoben**
- Export: AV1-Bitrate, Begleitdateien mit In/Out-Bereich, Lautheit bei sehr lautem Ton und Tempo bis 0,01×. Enter überspringt die Überschreiben-Frage nicht mehr.
- Shorts und Vorlagen lassen Overlays Overlays bleiben; Quick Actions können rückwärts laufende Clips und Tempo-Kurven.

## 0.9.0 – 2026-11-14

### English

**New**
- **Move everything.** Ripple edits can move all tracks and markers together.
- **Quick Actions** with their own settings and a list of results.
- **Highlights to Shorts.** Turn highlights into Shorts; clips are refitted when the format changes, with a safe area in the player.
- **Ready for YouTube.** Loudness to −14 LUFS, YouTube bitrates, chapters and VTT captions on export.
- **Templates in open projects** and multicam audio from one main source.

### Deutsch

**Neu**
- **Alles mitziehen.** Ripple-Schnitte verschieben auf Wunsch alle Spuren und Marker mit.
- **Quick Actions** mit eigenen Einstellungen und einer Trefferliste.
- **Highlights als Shorts.** Highlights werden zu Shorts; Clips werden beim Formatwechsel neu eingepasst, mit Safe-Area im Player.
- **Bereit für YouTube.** Lautheit auf −14 LUFS, YouTube-Bitraten, Kapitel und VTT-Untertitel beim Export.
- **Vorlagen ins offene Projekt** einfügen und Multicam-Ton von einer Hauptquelle.
