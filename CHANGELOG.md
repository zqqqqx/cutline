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

## 0.12.1 – 2026-11-17

### English

**Fixed**
- Dragging files in from the file explorer now scrolls the timeline at its edges, like dragging clips does.
- Esc in a value field only discards the typed value; the clip stays selected.
- A stock or Epidemic Sound download that was still running when you switched projects is no longer inserted into the other project.

### Deutsch

**Behoben**
- Beim Ziehen von Dateien aus dem Explorer rollt die Zeitleiste am Rand mit, wie beim Ziehen von Clips.
- Esc in einem Wertfeld verwirft nur die Eingabe; der Clip bleibt ausgewählt.
- Ein Stock- oder Epidemic-Sound-Download, der beim Projektwechsel noch lief, wird nicht mehr in das andere Projekt eingefügt.

## 0.12.0 – 2026-11-17

### English

**New**
- **Rebuilt player.** Playback starts right away, picture and sound stay in sync, and cutting while it plays no longer stalls the picture. If the computer gets busy, the preview quality drops automatically (shown as ½ or ¼ in the player) and comes back when you stop. Can be switched off under Settings → Performance.
- **Proxies in pieces.** A proxy is usable seconds after the import – the part at the playhead is built first, a thin line at the bottom of the clip shows what is ready. Building continues after a restart instead of starting over.
- **Drop files straight into the timeline.** Drag videos, images and audio from the file explorer onto the timeline; a preview shows where they land. Several files line up one after another, as one undo step.
- **Guided import for large projects.** Importing a CapCut or Final Cut project shows each step with a counter and can be cancelled. At the end you see what was imported and what was not. Clips whose file is missing are kept and can be relinked. Nested (compound) clips are resolved.
- **Audio track per clip.** For files with several audio tracks, pick the one to play in the inspector. CapCut projects with separated original sound and muted tracks now import with the right sound.
- **Export of very large projects.** Projects with thousands of clips are exported in sections and joined at the end – before, the export could fail to start.
- **Tracks.** Dropping a clip on a track of the wrong kind creates a matching track right there. Reorder tracks by dragging the track header. The "+" above the tracks adds one.
- **Zoom out further.** The whole project fits with free room at both ends; the ruler goes up to hours.
- **Log export.** Settings → About exports the log of the last 24 hours for troubleshooting – without project names, file paths or keys.
- **What's new.** The update window shows the changes of each version in plain words.
- **Hardware encoding on more computers.** Besides NVIDIA, Intel and AMD encoders, Cutline can now use the encoder built into Windows, so older graphics cards encode in hardware again. Settings → Performance → "Hardware encoding" picks the way: automatic, one of the encoders found, or software.

**Improved**
- Opening, saving and autosaving large projects run in the background – no more frozen window. Leaving a project waits until it is really saved.
- The timeline draws far faster with hundreds of clips; clips glide to their new place, zoom and scrolling are smooth.
- Track names of text and effect tracks line up with the others.
- Media tiles show their picture first; the scrub strip follows.
- Epidemic Sound: downloaded tracks are kept and reused in other projects; clearer messages when the sign-in expired or a limit is reached.
- Whisper subtitles work when the user folder contains an apostrophe.
- One-time note when hardware encoding is only unavailable because the graphics driver is outdated.
- Subtitles: choose the speech recognition (automatic, Parakeet or Whisper).
- Numbers follow the language: a decimal comma in German, also when typing values.
- Whisper subtitles work out of the box on Windows – the bundled FFmpeg now includes what they need.
- Linux: works with the FFmpeg 7 of current distributions; a note appears at start if the installed FFmpeg is too old.

**Fixed**
- Sound could be silent after importing a large CapCut project.
- Slideshows with more than 150 photos and transitions can be exported.
- "Zoom pulse" played at half speed in 60 fps projects.
- Linux: transition previews stayed empty, a clip's last frame did not hold, and shader transitions fell back to a simple one in the export.
- Proxies of files whose video starts later than their audio were off by a few frames.
- An export range starting in the middle of an effect clip no longer jumps.
- Voice recordings no longer lose audio when the disk is briefly busy.
- "Create new token" no longer switches MCP off.
- Tab and arrow keys work again on buttons and sliders; the space bar still starts playback.
- Several rare cases in which changes or files could be lost (full disk, unplugged drive, two actions at once).

### Deutsch

**Neu**
- **Neuer Player.** Die Wiedergabe startet sofort, Bild und Ton bleiben synchron, und Schneiden während des Abspielens hält das Bild nicht mehr an. Wird der Rechner knapp, sinkt die Vorschauqualität automatisch (im Player als ½ oder ¼ zu sehen) und kommt im Stand zurück. Abschaltbar unter Einstellungen → Leistung.
- **Proxys in Stücken.** Ein Proxy ist Sekunden nach dem Import nutzbar – die Stelle am Playhead wird zuerst gebaut, eine dünne Linie unten am Clip zeigt, was fertig ist. Nach einem Neustart geht der Bau weiter, statt von vorn zu beginnen.
- **Dateien direkt in die Zeitleiste ziehen.** Videos, Bilder und Ton aus dem Explorer auf die Zeitleiste ziehen; eine Vorschau zeigt, wo sie landen. Mehrere Dateien reihen sich hintereinander, als ein Rückgängig-Schritt.
- **Geführter Import großer Projekte.** Der Import eines CapCut- oder Final-Cut-Projekts zeigt jeden Schritt mit Zähler und lässt sich abbrechen. Am Ende steht, was übernommen wurde und was nicht. Clips, deren Datei fehlt, bleiben erhalten und lassen sich neu verknüpfen. Zusammengesetzte Clips werden aufgelöst.
- **Tonspur je Clip.** Bei Dateien mit mehreren Tonspuren wählst du im Inspektor, welche spielt. CapCut-Projekte mit getrenntem Originalton und stummen Spuren kommen jetzt mit dem richtigen Ton an.
- **Export sehr großer Projekte.** Projekte mit tausenden Clips werden in Abschnitten exportiert und am Ende zusammengefügt – vorher konnte der Export gar nicht starten.
- **Spuren.** Ein Clip auf einer Spur der falschen Art legt genau dort eine passende Spur an. Spuren lassen sich am Spurkopf umsortieren. Das „+" über den Spuren fügt eine hinzu.
- **Weiter herauszoomen.** Das ganze Projekt passt mit freiem Platz an beiden Enden; das Lineal reicht bis Stunden.
- **Protokoll exportieren.** Einstellungen → Über exportiert das Protokoll der letzten 24 Stunden zur Fehlersuche – ohne Projektnamen, Dateipfade oder Schlüssel.
- **Was ist neu.** Das Update-Fenster zeigt die Änderungen jeder Version in einfachen Worten.
- **Hardware-Kodierung auf mehr Rechnern.** Neben den Encodern von NVIDIA, Intel und AMD nutzt Cutline jetzt auch den in Windows eingebauten, sodass ältere Grafikkarten wieder in Hardware kodieren. Einstellungen → Leistung → „Hardware-Kodierung" legt den Weg fest: automatisch, einer der gefundenen Encoder oder Software.

**Verbessert**
- Öffnen, Speichern und automatisches Speichern großer Projekte laufen im Hintergrund – kein eingefrorenes Fenster mehr. Beim Verlassen eines Projekts wird gewartet, bis es wirklich gespeichert ist.
- Die Zeitleiste zeichnet bei hunderten Clips um ein Vielfaches schneller; Clips gleiten an ihren neuen Platz, Zoom und Rollen laufen weich.
- Die Namen von Text- und Effektspuren stehen in derselben Spalte wie die übrigen.
- Medien-Kacheln zeigen zuerst ihr Bild, der Streifen zum Durchblättern folgt.
- Epidemic Sound: Geladene Titel bleiben vorrätig und werden in anderen Projekten wiederverwendet; klarere Meldungen bei abgelaufener Anmeldung und erreichtem Limit.
- Whisper-Untertitel funktionieren auch, wenn der Benutzerordner einen Apostroph enthält.
- Einmaliger Hinweis, wenn die Hardware-Kodierung nur wegen eines veralteten Grafiktreibers nicht verfügbar ist.
- Untertitel: Die Spracherkennung ist wählbar (automatisch, Parakeet oder Whisper).
- Zahlen folgen der Sprache: Dezimalkomma im Deutschen, auch beim Eintippen von Werten.
- Whisper-Untertitel funktionieren unter Windows ohne Zutun – das mitgelieferte FFmpeg bringt jetzt alles Nötige mit.
- Linux: läuft mit dem FFmpeg 7 aktueller Distributionen; ist das installierte FFmpeg zu alt, kommt beim Start ein Hinweis.

**Behoben**
- Nach dem Import eines großen CapCut-Projekts konnte der Ton stumm sein.
- Diashows mit mehr als 150 Fotos und Übergängen lassen sich exportieren.
- „Zoom-Puls" lief in Projekten mit 60 Bildern/s halb so schnell.
- Linux: Vorschaubilder der Übergänge blieben leer, das letzte Bild eines Clips blieb nicht stehen, und Shader-Übergänge fielen im Export auf einen einfachen zurück.
- Proxys von Dateien, deren Bild später beginnt als der Ton, lagen um einige Bilder daneben.
- Ein Export-Bereich, der mitten in einem Effekt-Clip beginnt, springt nicht mehr.
- Sprachaufnahmen verlieren keinen Ton mehr, wenn die Platte kurz beschäftigt ist.
- „Neues Token erzeugen" schaltet MCP nicht mehr ab.
- Tab und Pfeiltasten funktionieren wieder auf Knöpfen und Reglern; die Leertaste startet weiterhin die Wiedergabe.
- Mehrere seltene Fälle, in denen Änderungen oder Dateien verloren gehen konnten (volle Platte, abgezogenes Laufwerk, zwei Aktionen gleichzeitig).

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
