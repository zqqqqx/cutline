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

## 0.16.1 – 2026-11-22

### English

**Fixed**
- **Transparent videos stay transparent.** Files with an alpha channel – ProRes 4444, Animation and PNG in MOV, WebM with transparency – no longer lose it in the preview or the export. They are shown from the original instead of a preview copy.
- **A clip trimmed to the size of a gap fits into it.** Trimming an edge until it snaps keeps it exactly on the neighbour, and a clip that is a hair too long for a gap is shortened by that hair when you drop it in. Before, it landed on a new track.

### Deutsch

**Behoben**
- **Transparente Videos bleiben transparent.** Dateien mit Alphakanal – ProRes 4444, Animation und PNG in MOV, WebM mit Transparenz – verlieren ihn nicht mehr, weder in der Vorschau noch im Export. Sie werden aus dem Original gezeigt statt aus einer Vorschaukopie.
- **Ein auf Lückengröße geschnittener Clip passt in die Lücke.** Eine eingerastete Kante bleibt beim Trimmen genau am Nachbarn, und ein Clip, der um Haaresbreite zu lang für eine Lücke ist, wird beim Ablegen um dieses Stück gekürzt. Vorher landete er auf einer neuen Spur.

## 0.16.0 – 2026-11-22

### English

**New**
- **Reverb for sound.** The Audio tab has a "Reverb" slider and, once it is on, "Room size" – from a small room to a large hall.
- **Strength for the audio tools and filters.** Rumble, hum, clicks, voice enhancement and the audio and voice filters each get a "Strength" slider while they are on. Projects you already have sound as before.
- **Green screen with background effects.** A keyed clip can now have a blurred background, a background color or an outline, like a cut-out person – also on images.
- **Edge controls for cutouts.** "Soft edge", "Shift edge" and "Invert" work for cut-out people and for green screen.

**Improved**
- **One place for cutouts.** The Smart tab has one "Cutout" area with Off / Person / Color instead of two separate sections. The outline can now be shown without the background there too.
- **Moving a large selection is smooth.** Dragging thousands of selected clips no longer makes the timeline stutter.

**Fixed**
- **The selection rectangle follows the timeline while it scrolls.** Its start stays where you began, so clips that scroll out of view stay selected.
- **Cancelling a person cutout** no longer leaves an unfinished mask file behind.

### Deutsch

**Neu**
- **Hall für den Ton.** Im Reiter Audio gibt es den Regler „Hall“ und, sobald er an ist, „Raumgröße“ – vom kleinen Raum bis zur großen Halle.
- **Stärke für Audio-Werkzeuge und Filter.** Rumpeln, Brummen, Klicks, Stimme hervorheben sowie die Klang- und Stimmfilter haben je einen Regler „Stärke“, solange sie an sind. Vorhandene Projekte klingen wie bisher.
- **Greenscreen mit Hintergrund-Effekten.** Ein Clip mit Farbschlüssel kann jetzt einen weichen Hintergrund, eine Hintergrundfarbe oder einen Umriss bekommen, wie eine freigestellte Person – auch bei Bildern.
- **Kanten-Regler fürs Freistellen.** „Weiche Kante“, „Kantenlage“ und „Umkehren“ gelten für freigestellte Personen und für Greenscreen.

**Verbessert**
- **Ein Bereich fürs Freistellen.** Der Reiter Smart hat einen Bereich „Freistellen“ mit Aus / Person / Farbe statt zwei getrennter Abschnitte. Der Umriss lässt sich dort jetzt auch ohne Hintergrund zeigen.
- **Große Auswahl verschieben läuft flüssig.** Beim Ziehen tausender ausgewählter Clips ruckelt die Zeitleiste nicht mehr.

**Behoben**
- **Der Auswahlrahmen folgt der Zeitleiste beim Rollen.** Sein Anfang bleibt dort, wo man begonnen hat; Clips, die aus dem Bild rollen, bleiben ausgewählt.
- **Abbrechen beim Freistellen einer Person** lässt keine halbfertige Maskendatei mehr liegen.

## 0.15.4 – 2026-11-21

### English

**Fixed**
- **Cuts in a range or part export sit on the right frame.** When only an In/Out range or parts were exported, a cut could come one frame later than in the preview.
- **Effects and filters start and end on the right frame in the export.** An effect could start one frame late or stay one frame too long, into the next clip.
- **Stacked clips pushed onto new tracks keep their order.** When a moved selection ran into other clips, clips lying on top of each other could swap places.
- **Q and W with a locked track in the selection** cut the other selected clips instead of doing nothing.
- **Q and W no longer leave tiny leftover clips** when a clip edge lies right next to the playhead.
- **Stepping frame by frame lands exactly on the frame.** After many steps with the arrow keys the paused preview could still show the clip before a cut although the timecode already stood on it.

### Deutsch

**Behoben**
- **Schnitte im Bereichs- und Teile-Export liegen auf dem richtigen Bild.** Wurde nur ein In/Out-Bereich oder in Teilen exportiert, konnte ein Schnitt ein Bild später kommen als in der Vorschau.
- **Effekte und Filter beginnen und enden im Export auf dem richtigen Bild.** Ein Effekt konnte ein Bild zu spät beginnen oder ein Bild zu lang stehen, bis in den nächsten Clip.
- **Übereinander liegende Clips behalten auf neuen Spuren ihre Reihenfolge.** Stieß eine verschobene Auswahl an andere Clips, konnten übereinander liegende Clips die Plätze tauschen.
- **Q und W mit einer gesperrten Spur in der Auswahl** schneiden die übrigen gewählten Clips, statt nichts zu tun.
- **Q und W lassen keine winzigen Restclips mehr stehen**, wenn eine Clipkante direkt neben dem Abspielkopf liegt.
- **Bildweises Weiterschalten landet genau auf dem Bild.** Nach vielen Schritten mit den Pfeiltasten konnte die Vorschau im Standbild an einem Schnitt noch den alten Clip zeigen, obwohl der Timecode schon darauf stand.

## 0.15.3 – 2026-11-21

### English

**New**
- **Scrollbar in the timeline.** A slim bar at the bottom of the timeline moves the view left and right. It can be switched off in Settings → General.
- **Closing can lead to your projects.** With "Go to projects when closing" switched on in Settings → General, the window’s X saves the project and opens the project overview instead of quitting. A second X quits.
- **2K export.** The export list names 2560×1440 "2K (1440p)", and a target file size now steps a 4K project down to 1440p before 1080p.

**Improved**
- **Zooming the timeline keeps the playhead in place** instead of the spot under the mouse – with the wheel, the slider and the buttons.
- **Trimming by dragging shows the result live.** Thumbnails and waveform stay where they are and are cut away while you drag, not only when you let go.

**Fixed**
- **No more black frames at cuts in the exported file.** Between clips sitting edge to edge, the export could show a single black frame that the preview never showed. Export and preview now use the same rule for which clip owns a frame. Just export the project again.
- **Extracting audio from a video with several audio tracks.** Extracting one track no longer mutes the others, and a second extracted track is no longer silent. Projects edited before this fix keep their muted clips – raise the volume once.
- **Moving several selected clips** keeps them on their tracks as one block. Before, the clips could be scattered onto new tracks.
- **Moving a mixed selection to other tracks** works: video clips move within the video tracks, audio clips within the audio tracks, all together or not at all.
- **Dropping a clip next to another on a different track** lets it snap exactly and stay there, also into a gap it fits exactly. Before, it could jump back to its old track.
- **Q and W with several clips selected** cut every selected clip under the playhead, not only one.
- **Splitting several selected clips** keeps all of them selected afterwards.

### Deutsch

**Neu**
- **Scrollleiste in der Zeitleiste.** Eine schmale Leiste am unteren Rand der Zeitleiste verschiebt die Ansicht nach links und rechts. Ausschalten geht unter Einstellungen → Allgemein.
- **Schließen kann zur Projektübersicht führen.** Ist „Beim Schließen zur Projektübersicht“ unter Einstellungen → Allgemein eingeschaltet, speichert das X des Fensters das Projekt und öffnet die Projektübersicht, statt Cutline zu beenden. Ein zweites X beendet.
- **2K-Export.** Die Exportliste nennt 2560×1440 „2K (1440p)“, und eine Zielgröße stuft ein 4K-Projekt jetzt erst auf 1440p herunter, dann auf 1080p.

**Verbessert**
- **Zoomen in der Zeitleiste hält den Abspielkopf an seiner Stelle** statt die Stelle unter der Maus – mit Mausrad, Regler und Knöpfen.
- **Trimmen durch Ziehen zeigt das Ergebnis sofort.** Vorschaubilder und Wellenform bleiben stehen und werden beim Ziehen abgeschnitten, nicht erst beim Loslassen.

**Behoben**
- **Keine schwarzen Bilder mehr an Schnitten in der exportierten Datei.** Zwischen aneinanderliegenden Clips konnte der Export ein einzelnes schwarzes Bild zeigen, das die Vorschau nie zeigte. Export und Vorschau entscheiden jetzt nach derselben Regel, welcher Clip ein Bild zeigt. Einfach das Projekt neu exportieren.
- **Ton extrahieren bei Videos mit mehreren Tonspuren.** Das Extrahieren einer Spur schaltet die anderen nicht mehr stumm, und eine zweite extrahierte Spur ist nicht mehr lautlos. Projekte, die vor diesem Fix bearbeitet wurden, behalten ihre stummen Clips – dort einmal die Lautstärke hochziehen.
- **Mehrere ausgewählte Clips verschieben** hält sie als Block auf ihren Spuren. Vorher konnten die Clips auf neue Spuren verstreut werden.
- **Eine gemischte Auswahl in andere Spuren verschieben** geht: Videoclips wandern innerhalb der Videospuren, Audioclips innerhalb der Audiospuren, alle zusammen oder gar nicht.
- **Einen Clip neben einen anderen auf einer anderen Spur legen** lässt ihn genau einrasten und dort bleiben, auch in einer Lücke, in die er genau passt. Vorher konnte er auf seine alte Spur zurückspringen.
- **Q und W bei mehreren ausgewählten Clips** schneiden jeden ausgewählten Clip unter dem Abspielkopf, nicht nur einen.
- **Mehrere ausgewählte Clips teilen** lässt danach alle ausgewählt.

## 0.15.2 – 2026-11-20

### English

**Improved**
- **Large projects open and export more smoothly.** Thumbnails and waveforms appear sooner, long recordings need far less memory, the export window no longer slows down with many clips, and Cutline checks your files less often in the background.
- **Archives can be moved.** An archived project finds its media and fonts again after you move or copy the folder.
- **Leftovers of a crashed export are removed at the next start**, so they no longer block the next export.

**Fixed**
- **Two windows, one project.** A second window on the same project no longer saves over the first one. It keeps its own changes in a backup instead.
- **Projects from a newer Cutline** are no longer opened by an older version, which could silently drop newer edits. Cutline asks you to update.
- **Automation mode (MCP)** no longer writes an old state over a project you edited in the meantime.
- **Relink by folder** only links a file when its name is unambiguous. Before, it could pick the wrong clip with the same name.
- **Renaming or moving something in the library** no longer breaks the open project.
- **Importing a second recordings ZIP** with the same name no longer overwrites the first.
- **CapCut import** keeps speeds above 10× and fades the way the editor does.
- **The exported log** no longer shows parts of file names that contain spaces.
- **The first export after starting** waits a moment for your graphics card instead of quietly using the slower software encoder.
- **Saving** reports a failure every time you press Save, not just once a minute.
- **Pausing a reversed clip** now shows exactly the frame under the playhead. On a slow disk it could stay on a neighbouring frame.

### Deutsch

**Verbessert**
- **Große Projekte öffnen und exportieren flüssiger.** Vorschaubilder und Wellenformen erscheinen früher, lange Aufnahmen brauchen viel weniger Speicher, das Export-Fenster wird bei vielen Clips nicht mehr zäh, und Cutline prüft deine Dateien im Hintergrund seltener.
- **Archive lassen sich verschieben.** Ein archiviertes Projekt findet Medien und Schriften wieder, nachdem du den Ordner verschoben oder kopiert hast.
- **Reste eines abgestürzten Exports verschwinden beim nächsten Start** und blockieren den nächsten Export nicht mehr.

**Behoben**
- **Zwei Fenster, ein Projekt.** Ein zweites Fenster am selben Projekt speichert nicht mehr über das erste. Es sichert seine Änderungen stattdessen in einer Sicherung.
- **Projekte aus einer neueren Cutline-Version** öffnet eine ältere Version nicht mehr – dabei konnten neuere Änderungen still verloren gehen. Cutline bittet um ein Update.
- **Automations-Modus (MCP)** schreibt keinen alten Stand mehr über ein Projekt, das du inzwischen bearbeitet hast.
- **Neu verknüpfen per Ordner** verknüpft nur noch, wenn der Dateiname eindeutig ist. Vorher konnte der falsche Clip mit gleichem Namen landen.
- **Umbenennen oder Verschieben in der Bibliothek** bricht das offene Projekt nicht mehr.
- **Import einer zweiten Aufnahmen-ZIP** mit gleichem Namen überschreibt die erste nicht mehr.
- **CapCut-Import** behält Tempo über 10× und Blenden so, wie der Editor sie kennt.
- **Das exportierte Protokoll** zeigt keine Teile von Dateinamen mit Leerzeichen mehr.
- **Der erste Export nach dem Start** wartet kurz auf die Grafikkarte, statt still den langsameren Software-Encoder zu nehmen.
- **Speichern** meldet einen Fehler bei jedem Druck auf Speichern, nicht nur einmal pro Minute.
- **Pause auf einem Rückwärts-Clip** zeigt jetzt genau das Bild unter dem Abspielkopf. Auf langsamen Platten konnte ein Nachbarbild stehen bleiben.

## 0.15.1 – 2026-11-20

### English

**Fixed**
- **No more stutter while proxies are still being made.** When playback ran out of the finished part of a proxy, the picture could fall more than a second behind the sound and the quality dropped. The switch to the original file now happens at the right moment.

### Deutsch

**Behoben**
- **Kein Haken mehr, während Proxys noch entstehen.** Lief die Wiedergabe aus dem fertigen Teil eines Proxys heraus, konnte das Bild über eine Sekunde hinter den Ton fallen und die Qualität sank. Der Wechsel zur Originaldatei kommt jetzt im richtigen Moment.

## 0.15.0 – 2026-11-20

### English

**New**
- **Auto reframe in the export.** When you export in another aspect ratio, "Image" has a third choice next to Fill and Fit: Auto reframe moves the crop along with the action in the picture. Your project stays as it is.

**Improved**
- **Reversed clips play smoothly.** Reversed video without a proxy now plays at full speed up to 1080p instead of a few pictures per second.
- **4K starts at once.** A 4K clip without a proxy shows its first picture right when you press Play, once the playhead has rested on it for a moment.
- **Linux: FFmpeg included.** The Linux package now brings its own FFmpeg, so nothing has to be installed first.

**Fixed**
- **Picture and sound start together.** When the sound needed longer to load than the picture, playback started without it and the sound came in late or out of step. Playback now waits a moment for the sound.
- **Split at markers** stops at 50 parts, like the other ways to split.

### Deutsch

**Neu**
- **Auto-Reframe im Export.** Beim Export in einem anderen Seitenverhältnis hat „Bild“ eine dritte Wahl neben Füllen und Einpassen: Auto-Reframe führt den Ausschnitt mit dem Geschehen im Bild mit. Dein Projekt bleibt, wie es ist.

**Verbessert**
- **Rückwärts-Clips laufen flüssig.** Rückwärts laufendes Video ohne Proxy spielt bis 1080p jetzt in voller Geschwindigkeit statt mit wenigen Bildern pro Sekunde.
- **4K startet sofort.** Ein 4K-Clip ohne Proxy zeigt sein erstes Bild direkt beim Druck auf Play, sobald der Abspielkopf kurz darauf geruht hat.
- **Linux: FFmpeg ist dabei.** Das Linux-Paket bringt sein eigenes FFmpeg mit, vorher muss nichts mehr installiert werden.

**Behoben**
- **Bild und Ton starten zusammen.** Brauchte der Ton länger zum Laden als das Bild, lief die Wiedergabe ohne ihn los und der Ton kam verspätet oder versetzt. Die Wiedergabe wartet jetzt kurz auf den Ton.
- **Aufteilen an Markern** hört bei 50 Teilen auf, wie die anderen Arten.

## 0.14.0 – 2026-11-20

### English

**New**
- **Split into parts.** The export window can cut a video into several files in one go – by count, by length or at your markers. The parts land in their own folder, numbered in order, ready to upload one by one.
- **File size.** Choose "At most 10 MB" (or 25, 50, 100, or your own number) in the export window and the file stays under it. If the size is too small for the resolution, Cutline exports a smaller picture and says so.
- **Playback volume.** The speaker next to the time in the player sets how loud Cutline plays – click to mute, hover for the slider, or use the mouse wheel. Your project and the export are not changed.
- **Export in another aspect ratio.** The export window can turn a 16:9 project into a real 9:16, 1:1 or 4:5 file. "Fill" crops the sides, "Fit" keeps the whole picture and puts a blurred copy behind it. Titles and overlays stay in the picture. Picking a platform sets its ratio for you.

### Deutsch

**Neu**
- **In Teile aufteilen.** Das Export-Fenster zerlegt ein Video in einem Zug in mehrere Dateien – nach Anzahl, nach Länge oder an deinen Markern. Die Teile landen in einem eigenen Ordner, der Reihe nach nummeriert, fertig zum einzelnen Hochladen.
- **Dateigröße.** Wähle im Export-Fenster „Höchstens 10 MB“ (oder 25, 50, 100 oder eine eigene Zahl), und die Datei bleibt darunter. Reicht die Größe nicht für die Auflösung, exportiert Cutline ein kleineres Bild und sagt das an.
- **Lautstärke der Wiedergabe.** Der Lautsprecher neben der Zeit im Player stellt ein, wie laut Cutline abspielt – Klick für stumm, drüberfahren für den Regler, oder das Mausrad. Projekt und Export bleiben unverändert.
- **Export in anderem Seitenverhältnis.** Das Export-Fenster macht aus einem 16:9-Projekt eine echte 9:16-, 1:1- oder 4:5-Datei. „Füllen“ schneidet die Seiten ab, „Einpassen“ behält das ganze Bild und legt eine weichgezeichnete Kopie dahinter. Titel und Overlays bleiben im Bild. Die Wahl einer Plattform stellt ihr Verhältnis gleich mit ein.

## 0.13.0 – 2026-11-19

### English

**New**
- **Crop.** Cut away the edges of any video or image: four sliders under Basic → Crop, or drag the bars at the edges right in the player. The rest of the picture stays where it is.
- **Command search.** Ctrl+K opens a search for every command of the editor, including everything from the menu and the Quick Actions. Type a few letters, press Enter. It shows the shortcut next to each command.
- **Layouts.** Split screen for two, three or four clips and picture-in-picture in any corner, in one click: Quick Actions → Layout. Each clip is scaled and cropped to fill its cell.
- **Export queue.** "Add to queue" in the export window lines up several exports – other formats, other names, other projects. They run one after another while you keep editing.
- **Thumbnail tool.** Quick Actions → Thumbnail: pick the frame, cut out the person with an outline, blur or darken the background, add a title from a template, and save a 1280×720 PNG.
- **Karaoke captions.** Two new caption styles highlight the word that is being spoken. Any text clip can switch it on in the inspector; the highlight colour is yours to choose.
- **Censor.** Smart → Censor: drag a frame around a face, a licence plate or a name, and the pixelated or blurred area follows it through the clip. Several areas per clip are possible.
- **Brand Kit.** Keep your colours, fonts and logo in Settings → Brand Kit. The colours appear next to every colour picker, the fonts at the top of the font list, and "Insert logo" puts the logo in the corner for the whole video.

**Improved**
- The menu shows your own shortcuts, and every menu command can be given one under Settings → Keyboard shortcuts.
- Text lines in the exported video are spaced exactly as in the preview.

**Fixed**
- Text with several lines showed a small box at the end of each line in the exported video.
- A shortcut of your own could end up on two commands when a new version added a default with the same key.
- A finished export was listed under the project that happened to be open instead of the one it came from.

### Deutsch

**Neu**
- **Zuschnitt.** Schneide die Ränder von Videos und Bildern weg: vier Regler unter Basis → Zuschnitt, oder ziehe die Balken an den Kanten direkt im Player. Der Rest des Bilds bleibt an seiner Stelle.
- **Befehlssuche.** Strg+K öffnet eine Suche über alle Befehle des Editors, auch alles aus dem Menü und den Schnellaktionen. Ein paar Buchstaben tippen, Enter drücken. Neben jedem Befehl steht seine Taste.
- **Layouts.** Splitscreen für zwei, drei oder vier Clips und Bild-in-Bild in jeder Ecke, mit einem Klick: Schnellaktionen → Layout. Jeder Clip wird so skaliert und zugeschnitten, dass er sein Feld füllt.
- **Export-Warteschlange.** „In Warteschlange“ im Export-Fenster reiht mehrere Exporte ein – andere Formate, andere Namen, andere Projekte. Sie laufen nacheinander, während du weiterschneidest.
- **Thumbnail-Werkzeug.** Schnellaktionen → Thumbnail: Bild wählen, Person mit Kontur freistellen, Hintergrund weichzeichnen oder abdunkeln, Titel aus einer Vorlage dazu, als PNG in 1280×720 speichern.
- **Karaoke-Untertitel.** Zwei neue Untertitel-Stile heben das gerade gesprochene Wort hervor. Jeder Text-Clip kann das im Inspektor einschalten; die Farbe der Hervorhebung wählst du selbst.
- **Zensieren.** Smart → Zensieren: Rahmen um ein Gesicht, ein Kennzeichen oder einen Namen ziehen, und die verpixelte oder weichgezeichnete Fläche folgt dem Motiv durch den Clip. Mehrere Flächen je Clip sind möglich.
- **Brand Kit.** Deine Farben, Schriften und dein Logo an einer Stelle: Einstellungen → Brand Kit. Die Farben stehen neben jedem Farbwähler, die Schriften oben in der Schriftliste, und „Logo einfügen“ setzt das Logo für das ganze Video in die Ecke.

**Verbessert**
- Das Menü zeigt deine eigenen Tastenkürzel, und jeder Menübefehl kann unter Einstellungen → Tastenkürzel eine Taste bekommen.
- Textzeilen stehen im exportierten Video genau so weit auseinander wie in der Vorschau.

**Behoben**
- Text mit mehreren Zeilen zeigte im exportierten Video ein kleines Kästchen am Ende jeder Zeile.
- Ein eigenes Tastenkürzel konnte auf zwei Befehlen landen, wenn eine neue Version dieselbe Taste als Standard vergab.
- Ein fertiger Export wurde dem gerade geöffneten Projekt zugeordnet statt dem, aus dem er stammt.

## 0.12.2 – 2026-11-19

### English

**Improved**
- The exported log now has one line per playback (time to the first picture and sound, dropped frames). That makes playback problems much easier to track down.

**Fixed**
- Collapsed tracks and track heights are saved with the project.
- Quick Actions remember the values you set, and the safe area stays on after a restart.
- Typing a start or duration for a clip can no longer push it over its neighbour.
- Fades shrink with the clip when a speed change or trimming makes it shorter.
- CapCut import: nonsense numbers in a draft no longer produce broken clips.
- Story tools: a dialogue box inserted right before another text ends where that one starts instead of failing.
- Rolling a cut to the left works between text, image and effect clips.

### Deutsch

**Verbessert**
- Das exportierte Protokoll enthält jetzt eine Zeile je Wiedergabe (Zeit bis zum ersten Bild und Ton, verworfene Bilder). Damit lassen sich Wiedergabe-Probleme viel leichter eingrenzen.

**Behoben**
- Eingeklappte Spuren und Spurhöhen werden mit dem Projekt gespeichert.
- Schnellaktionen merken sich die eingestellten Werte, und die Safe-Area bleibt nach einem Neustart an.
- Ein getippter Start oder eine getippte Dauer kann einen Clip nicht mehr über seinen Nachbarn schieben.
- Blenden schrumpfen mit, wenn ein Clip durch Tempo oder Trimmen kürzer wird.
- CapCut-Import: unsinnige Zahlen in einem Entwurf ergeben keine kaputten Clips mehr.
- Story-Werkzeuge: Eine Dialogbox direkt vor einem anderen Text endet an dessen Anfang, statt zu scheitern.
- Einen Schnitt nach links rollen klappt jetzt auch zwischen Text-, Bild- und Effekt-Clips.

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
