"""Baut die Kenney-Inhaltspakete für Cutline.

    python bau.py <entpackt> <ausgabe>

<entpackt>: Ordner mit je einem entpackten Kenney-ZIP (`kenney_<slug>/`).
<ausgabe>:  Wurzel des Zweigs `packs`; die Paketordner `kenney-*` darin werden neu geschrieben.
Braucht FFmpeg/FFprobe im PATH. Wiederholbar: gleicher Eingang → gleiche Paketordner.
"""
import concurrent.futures as cf
import hashlib
import os
import pathlib
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import namen  # noqa: E402

VERSION = "1.0.0"
LIZENZ = "CC0-1.0"
QUELLE = "https://kenney.nl/assets/"

# Pakete: Ordner → Metadaten und Quellen (Slug, Art). Reihenfolge = Vorrang bei Doppeln.
PAKETE = {
    "kenney-oberflaeche": dict(
        name=("Oberfläche (Kenney)", "Interface (Kenney)"),
        desc=("Klicks, Schalter, Fenster- und Hinweistöne, digitale Effekte",
              "Clicks, switches, window and notification sounds, digital effects"),
        sounds=["interface-sounds", "ui-audio", "digital-audio"]),
    "kenney-treffer": dict(
        name=("Treffer und Geräusche (Kenney)", "Impacts and foley (Kenney)"),
        desc=("Aufprall, Schritte, Alltagsgeräusche, Karten und Würfel",
              "Impacts, footsteps, everyday foley, cards and dice"),
        sounds=["impact-sounds", "rpg-audio", "casino-audio"]),
    "kenney-scifi": dict(
        name=("Sci-Fi (Kenney)", "Sci-fi (Kenney)"),
        desc=("Laser, Raumschiffe, Explosionen und Technik", "Lasers, spaceships, explosions and machines"),
        sounds=["sci-fi-sounds"]),
    "kenney-jingles": dict(
        name=("Jingles (Kenney)", "Jingles (Kenney)"),
        desc=("Kurze Musikstücke: 8-Bit, Hits, Pizzicato, Saxofon, Steeldrum",
              "Short music stings: 8-bit, hits, pizzicato, sax, steel drum"),
        sounds=["music-jingles"]),
    "kenney-stimmen": dict(
        name=("Ansagen (Kenney)", "Voice-overs (Kenney)"),
        desc=("Englische Ansagen: Zahlen, Runden, Sieg und Niederlage, Kampfrufe",
              "English announcer lines: numbers, rounds, win and lose, battle calls"),
        sounds=["voiceover-pack", "voiceover-pack-fighter"]),
    "kenney-emotes": dict(
        name=("Sprechblasen (Kenney)", "Emotes (Kenney)"),
        desc=("Sprechblasen mit Gefühlen und Zeichen (Vektor)", "Speech-bubble emotes (vector)"),
        stickers=["emotes-pack"]),
    "kenney-partikel": dict(
        name=("Partikel (Kenney)", "Particles (Kenney)"),
        desc=("Funken, Flammen, Lichter, Rauch und Spuren mit Transparenz",
              "Sparks, flames, lights, smoke and traces with transparency"),
        stickers=["particle-pack", "smoke-particles"]),
    "kenney-symbole": dict(
        name=("Symbole (Kenney)", "Game icons (Kenney)"),
        desc=("Weiße Symbole: Pfeile, Wiedergabe, Bedienung, Spiel (Vektor)",
              "White icons: arrows, playback, controls, games (vector)"),
        stickers=["game-icons"]),
}

KLEIN = 512           # größere PNG werden auf diese Kante verkleinert
SPITZE_MIN, SPITZE_MAX, SPITZE_ZIEL = -12.0, -1.0, -3.0
RAUCH_AUSWAHL = ("00", "04", "08")  # je Rauchfarbe drei Formen (die übrigen sind fast gleich)


# ---------------------------------------------------------------- Hilfen
def run(*args):
    return subprocess.run(args, capture_output=True, text=True, encoding="utf-8", errors="replace")


def slug(s):
    s = s.lower()
    for a, b in (("ä", "ae"), ("ö", "oe"), ("ü", "ue"), ("ß", "ss")):
        s = s.replace(a, b)
    return re.sub(r"[^a-z0-9]+", "-", s).strip("-")


def worte_en(base):
    """`impactMetal_heavy` → `impact metal heavy`"""
    s = re.sub(r"([a-z])([A-Z])", r"\1 \2", base).replace("_", " ").replace("-", " ")
    return re.sub(r"\s+", " ", s).strip().lower()


def teile(stem):
    """Dateiname ohne Endung → (Grundname, Nummer oder -1)."""
    m = re.match(r"^(.*?)[_-]?(\d+)$", stem)
    if m and m.group(1):
        return m.group(1), int(m.group(2))
    return stem, -1


def natur(p):
    """natürliche Sortierung: switch2 vor switch10"""
    return [int(x) if x.isdigit() else x for x in re.split(r"(\d+)", str(p))]


def toml_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


# ---------------------------------------------------------------- Sounds
def probe(f):
    """→ (codec, rate, kanäle, md5 der Decodierung, Spitze dB, L≠R?)"""
    r = run("ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries",
            "stream=codec_name,sample_rate,channels", "-of", "csv=p=0", str(f))
    codec, rate, ch = r.stdout.strip().split(",")[:3]
    r = run("ffmpeg", "-hide_banner", "-nostats", "-i", str(f), "-map", "0:a:0",
            "-af", "volumedetect", "-f", "md5", "-")
    md5 = re.search(r"MD5=([0-9a-f]+)", r.stdout).group(1)
    peak = float(re.search(r"max_volume: (-?[\d.]+|-inf) dB", r.stderr).group(1).replace("-inf", "-200"))
    stereo = False
    if int(ch) == 2:
        r = run("ffmpeg", "-hide_banner", "-nostats", "-i", str(f), "-map", "0:a:0",
                "-af", "pan=mono|c0=c0-c1,volumedetect", "-f", "null", "-")
        m = re.search(r"max_volume: (-?[\d.]+|-inf) dB", r.stderr)
        stereo = bool(m) and m.group(1) != "-inf" and float(m.group(1)) > -40  # darunter: Doppelmono mit Kodierrauschen
    return codec, int(rate), int(ch), md5, peak, stereo


def sound_quellen(ein, slug_):
    """→ Liste (Datei, Grundname, Nummer, deutsch, Kategorie, englisch, Zusatz-Tags)"""
    root = ein / f"kenney_{slug_}"
    out = []
    files = sorted((p for p in root.rglob("*.ogg") if not p.name.startswith("Preview")), key=natur)
    for f in files:
        if slug_.startswith("voiceover"):
            stem = f.stem
            stimme = {"Male": "Männlich", "Female": "Weiblich"}.get(f.parent.name, "Kampfansagen")
            if stem not in namen.ANSAGEN:
                raise SystemExit(f"Ansage fehlt in namen.py: {stem}")
            en = namen.ANSAGE_EN.get(stem) or worte_en(re.sub(r"^war_", "", stem)).capitalize()
            de = f"{namen.ANSAGEN[stem]} („{en}“)"
            out.append((f, stem, -1, de, stimme, en,
                        ["voice", "ansage", stimme.lower(), en.lower()]))
            continue
        base, nr = teile(f.stem)
        tab = namen.SOUNDS[slug_]
        if base not in tab:
            raise SystemExit(f"Grundname fehlt in namen.py ({slug_}): {base}")
        de, kat = tab[base]
        out.append((f, base, nr, de, kat, worte_en(base), [worte_en(base)]))
    return out


def spitze(f):
    """echte Spitze in dB (Fließkomma, ohne Abschneiden bei 0 dB)"""
    r = run("ffmpeg", "-hide_banner", "-nostats", "-i", str(f), "-af", "astats=measure_perchannel=none",
            "-f", "null", "-")
    return float(re.search(r"Peak level dB: (-?[\d.]+|-inf)", r.stderr).group(1).replace("-inf", "-200"))


def baue_sound(job):
    f, ziel, info, sprache = job
    codec, rate, ch, _md5, peak, stereo = info
    stereo = stereo and not sprache  # Ansagen immer mono
    gain = 0.0
    if peak > SPITZE_MAX or peak < SPITZE_MIN:
        gain = SPITZE_ZIEL - peak
    mono = ch == 1 or not stereo
    if codec == "vorbis" and rate == 48000 and (ch == 1 or stereo) and gain == 0.0:
        shutil.copyfile(f, ziel)
        return "kopiert", gain

    def kodieren(gain):
        # Zeitstempel neu aus der Sample-Zahl: einige Sci-Fi-Quellen ergeben sonst rückläufige DTS
        filt = ["asetpts=N/SR/TB"]
        if mono and ch == 2:
            # selbst mitteln: FFmpegs -ac 1 addiert in Fließkomma beide Kanäle mit −3 dB → +3 dB bei Doppelmono
            filt.append("pan=mono|c0=0.5*c0+0.5*c1")
        if gain:
            filt.append(f"volume={gain:.2f}dB")
        r = run("ffmpeg", "-hide_banner", "-v", "error", "-y", "-i", str(f), "-map", "0:a:0",
                "-map_metadata", "-1", "-af", ",".join(filt), "-ar", "48000", "-c:a", "libvorbis",
                "-q:a", "3", str(ziel))
        if r.returncode:
            raise SystemExit(f"FFmpeg-Fehler bei {f}: {r.stderr}")

    kodieren(gain)
    # Umrechnen und Vorbis können die Spitze um bis zu ~1 dB anheben: dann doch auf −3 dB
    nach = spitze(ziel)
    if nach > -0.5:
        gain += SPITZE_ZIEL - nach
        kodieren(gain)
    return "umgewandelt", gain


# ---------------------------------------------------------------- SVG-Bögen zerlegen
def svg_zellen(svg_text, x0, y0, raster, spalten, zeilen):
    """Zerlegt einen Flash-SVG-Bogen (absolute Koordinaten) in Zellen eines Rasters.
    → {(zeile, spalte): svg-Text}. Teilpfade werden der Zelle ihres Mittelpunkts zugeordnet."""
    defs = {gid: block for block, gid in re.findall(
        r'(<linearGradient[^>]*id="([^"]+)"[^>]*>.*?</linearGradient>)', svg_text, re.S)}
    zellen = {}
    for attrs, d in re.findall(r'<path ([^>]*?)d="([^"]*)"', svg_text):
        for sp in re.split(r"(?=M)", d):
            nums = [float(x) for x in re.findall(r"-?\d+(?:\.\d+)?", sp)]
            if len(nums) < 2:
                continue
            xs, ys = nums[0::2], nums[1::2]
            bx = (min(xs), min(ys), max(xs), max(ys))
            c = int(((bx[0] + bx[2]) / 2 - x0) // raster)
            r = int(((bx[1] + bx[3]) / 2 - y0) // raster)
            if 0 <= c < spalten and 0 <= r < zeilen:
                z = zellen.setdefault((r, c), [])
                if z and z[-1][0] == attrs:
                    z[-1][1].append(sp)  # gleicher Pfad: Teilpfade zusammenlassen (Löcher)
                else:
                    z.append((attrs, [sp]))
                zellen.setdefault(("box", r, c), []).append(bx)
    out = {}
    for key, teile_ in zellen.items():
        if key[0] == "box":
            continue
        boxen = zellen[("box",) + key]
        x1, y1 = min(b[0] for b in boxen), min(b[1] for b in boxen)
        x2, y2 = max(b[2] for b in boxen), max(b[3] for b in boxen)
        # feste Zellgröße statt Umriss: „Minus“ bleibt so klein wie im Original neben „Haus“
        seite = max(raster, max(x2 - x1, y2 - y1) * 1.08)
        vx, vy = (x1 + x2 - seite) / 2, (y1 + y2 - seite) / 2
        body = "".join(f'<path {a}d="{" ".join(s.strip() for s in sps)}"/>' for a, sps in teile_)
        used = "".join(defs[g] for g in sorted(set(re.findall(r"url\(#([^)]+)\)", body))))
        out[key] = (f'<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" '
                    f'viewBox="{vx:.2f} {vy:.2f} {seite:.2f} {seite:.2f}">'
                    + (f"<defs>{used}</defs>" if used else "") + body + "</svg>\n")
    return out


# ---------------------------------------------------------------- PNG
def baue_png(job):
    src, ziel = job
    r = run("ffprobe", "-v", "error", "-show_entries", "stream=width,height", "-of", "csv=p=0", str(src))
    w, h = map(int, r.stdout.strip().split(","))
    if max(w, h) <= KLEIN:
        shutil.copyfile(src, ziel)
        return False
    # vormultipliziert skalieren, sonst entstehen dunkle Ränder an halbtransparenten Kanten
    vf = (f"format=rgba,premultiply=inplace=1,scale={KLEIN}:{KLEIN}:force_original_aspect_ratio=decrease"
          ":flags=lanczos,unpremultiply=inplace=1,format=rgba")
    r = run("ffmpeg", "-hide_banner", "-v", "error", "-y", "-i", str(src), "-vf", vf, "-frames:v", "1",
            "-c:v", "png", str(ziel))
    if r.returncode:
        raise SystemExit(f"FFmpeg-Fehler bei {src}: {r.stderr}")
    return True


def sticker_quellen(ein, slug_):
    """→ Liste (Quelle: Pfad oder SVG-Text, Endung, Grundname, Nummer, deutsch, Kategorie, englisch)"""
    root = ein / f"kenney_{slug_}"
    out = []
    if slug_ == "emotes-pack":
        svg = (root / "Vector" / "emotes_vector.svg").read_text(encoding="utf-8")
        zellen = svg_zellen(svg, 16, 16, 40, 10, 3)
        for i, (orig, de) in enumerate(namen.EMOTES):
            en = "speech bubble" if orig == "__" else worte_en(orig)
            out.append((zellen[(i // 10, i % 10)], "svg", orig.strip("_") or "empty", -1, de,
                        "Sprechblasen", en))
    elif slug_ == "game-icons":
        svg = (root / "Vector" / "vector_whiteIcons.svg").read_text(encoding="utf-8")
        zellen = svg_zellen(svg, 0, 0, 50, 15, 7)
        for i, (orig, de, kat) in enumerate(namen.SYMBOLE):
            out.append((zellen[(i // 15, i % 15)], "svg", orig, -1, de, kat, worte_en(orig)))
    elif slug_ == "particle-pack":
        for f in sorted((root / "PNG (Transparent)").glob("*.png"), key=natur):  # ohne Unterordner „Rotated“
            base, nr = teile(f.stem)
            de, kat = namen.PARTIKEL[base]
            out.append((f, "png", base, nr, de, kat, worte_en(base)))
    elif slug_ == "smoke-particles":
        for f in sorted((root / "PNG").rglob("*.png"), key=natur):
            base, nr = teile(f.stem)
            if f.stem[-2:] not in RAUCH_AUSWAHL:
                continue
            de, kat = namen.PARTIKEL[base]
            out.append((f, "png", base, nr, de, kat, worte_en(base)))
    return out


# ---------------------------------------------------------------- Pakete
def nummerieren(eintraege, ordner="kenney-x"):
    """Gleicher deutscher Name mehrfach → Nummer 1…n in Quellreihenfolge anhängen.
    IDs beginnen mit dem Paketkürzel, damit sie über alle Pakete eindeutig sind („treffer-glas-schwer-1“)."""
    kurz = ordner.removeprefix("kenney-")
    gruppen = {}
    for e in eintraege:
        # `zusatz` trennt gleichlautende Einträge ohne Nummer (Ansage männlich/weiblich)
        gruppen.setdefault((e["de"], e.get("zusatz", "")), []).append(e)
    for (de, zusatz), g in gruppen.items():
        if len(g) > 1:
            for i, e in enumerate(g, 1):
                e["de"], e["en"] = f"{de} {i}", f"{e['en'].capitalize()} {i}"
                e["id"] = f"{kurz}-{slug(de)}{zusatz}-{i}"
        else:
            g[0]["en"] = g[0]["en"][:1].upper() + g[0]["en"][1:]
            g[0]["id"] = f"{kurz}-{slug(de)}{zusatz}"


def manifest(ordner, meta, eintraege):
    kind = "sound" if "sounds" in meta else "sticker"
    z = [f"# Erzeugt von tools/bau.py aus Kenney-Paketen (CC0) – nicht von Hand ändern.",
         "[plugin]", f'id = "kenney.{ordner.removeprefix("kenney-")}"', f'version = "{VERSION}"', "api = 1",
         'kind = "asset-pack"',
         f"name = {{ de = {toml_str(meta['name'][0])}, en = {toml_str(meta['name'][1])} }}",
         f"description = {{ de = {toml_str(meta['desc'][0])}, en = {toml_str(meta['desc'][1])} }}",
         'authors = ["Kenney"]', f'license = "{LIZENZ}"', 'source = "https://kenney.nl/assets"', ""]
    for e in eintraege:
        tags = []
        for t in e["tags"] + [e["de_basis"].lower(), "kenney"]:
            if t and t not in tags:
                tags.append(t)
        z += ["[[asset]]", f'id = "{e["id"]}"', f'kind = "{kind}"',
              f"name = {{ de = {toml_str(e['de'])}, en = {toml_str(e['en'])} }}",
              f"category = [{toml_str(e['kat'])}]", f'file = "{e["file"]}"', f'license = "{LIZENZ}"',
              'author = "Kenney"', f'source = "{QUELLE}{e["slug"]}"',
              "tags = [" + ", ".join(toml_str(t) for t in tags) + "]", ""]
    return "\n".join(z)


def main():
    ein, aus = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    nur = sys.argv[3:]  # optional: nur diese Paketordner
    pool = cf.ThreadPoolExecutor(max_workers=os.cpu_count() or 4)
    gesehen = {}  # md5 der Decodierung → (Paket, Datei) für Doppel über alle Pakete
    bericht = []
    for ordner, meta in PAKETE.items():
        if nur and ordner not in nur:
            continue
        ziel = aus / ordner
        if ziel.exists():
            shutil.rmtree(ziel)
        ziel.mkdir(parents=True)
        eintraege, doppel, kopiert, pegel, verkleinert = [], [], 0, 0, 0
        slugs = meta.get("sounds") or meta.get("stickers")
        for s in slugs:
            lic = next(p for p in (ein / f"kenney_{s}").iterdir() if p.name.lower() == "license.txt")
            shutil.copyfile(lic, ziel / f"LICENSE-{s}.txt")
            cred = ein / f"kenney_{s}" / "Credits.txt"
            if cred.exists():
                shutil.copyfile(cred, ziel / f"CREDITS-{s}.txt")
        if "sounds" in meta:
            (ziel / "audio").mkdir()
            quellen = [(s, q) for s in slugs for q in sound_quellen(ein, s)]
            infos = list(pool.map(probe, [q[0] for _, q in quellen]))
            jobs = []
            for (s, (f, base, nr, de, kat, en, tags)), info in zip(quellen, infos):
                if info[3] in gesehen:
                    doppel.append(f"{s}/{f.name} = {gesehen[info[3]]}")
                    continue
                gesehen[info[3]] = f"{s}/{f.name}"
                zusatz = {"Männlich": "-m", "Weiblich": "-w"}.get(kat, "")
                e = dict(slug=s, de=de, zusatz=zusatz, de_basis=re.sub(r" \(.*$|„.*$", "", de).strip(), kat=kat, en=en,
                         tags=tags, src=f, info=info)
                eintraege.append(e)
            nummerieren(eintraege, ordner)
            for e in eintraege:
                e["file"] = f"audio/{e['id']}.ogg"
                jobs.append((e["src"], ziel / e["file"], e["info"], e["slug"].startswith("voiceover")))
            for art, gain in pool.map(baue_sound, jobs):
                kopiert += art == "kopiert"
                pegel += gain != 0.0
        else:
            (ziel / "bilder").mkdir()
            jobs = []
            for s in slugs:
                for src, ext, base, nr, de, kat, en in sticker_quellen(ein, s):
                    eintraege.append(dict(slug=s, de=de, de_basis=de, kat=kat, en=en, tags=[en],
                                          src=src, ext=ext))
            nummerieren(eintraege, ordner)
            for e in eintraege:
                e["file"] = f"bilder/{e['id']}.{e['ext']}"
                if e["ext"] == "svg":
                    (ziel / e["file"]).write_text(e["src"], encoding="utf-8")
                else:
                    jobs.append((e["src"], ziel / e["file"]))
            verkleinert = sum(pool.map(baue_png, jobs))
        ids = [e["id"] for e in eintraege]
        assert len(ids) == len(set(ids)), f"{ordner}: doppelte IDs"
        (ziel / "plugin.toml").write_text(manifest(ordner, meta, eintraege), encoding="utf-8", newline="\n")
        bericht.append((ordner, len(eintraege), kopiert, pegel, verkleinert, doppel))
    for ordner, n, kop, peg, verk, doppel in bericht:
        print(f"{ordner}: {n} Einträge, {kop} unverändert, {peg} Pegel angepasst, {verk} verkleinert, "
              f"{len(doppel)} Doppel weggelassen")
        for d in doppel:
            print("   Doppel:", d)


def _selbsttest():
    assert teile("impactMetal_heavy_000") == ("impactMetal_heavy", 0)
    assert teile("click1") == ("click", 1) and teile("card-shuffle") == ("card-shuffle", -1)
    assert teile("doorClose_4") == ("doorClose", 4) and teile("10") == ("10", -1)
    assert sorted(["switch10", "switch2"], key=natur) == ["switch2", "switch10"]
    assert worte_en("impactMetal_heavy") == "impact metal heavy"
    assert slug("Glas-Ton schwer ü") == "glas-ton-schwer-ue"
    e = [dict(de="Klick", en="click"), dict(de="Klick", en="click"), dict(de="Gong", en="bong")]
    nummerieren(e)
    assert [x["de"] for x in e] == ["Klick 1", "Klick 2", "Gong"] and e[1]["id"] == "x-klick-2"
    sv = ('<svg><path fill="#FFF" d="M0 0 L10 0 10 10 0 10 0 0 M2 2 L8 2 8 8 2 8"/>'
          '<path fill="url(#g)" d="M60 0 L70 0 70 10 60 0"/></svg>')
    z = svg_zellen(sv.replace("<svg>", '<svg><linearGradient id="g"></linearGradient>'), 0, 0, 50, 2, 1)
    assert set(z) == {(0, 0), (0, 1)} and "M2 2" in z[(0, 0)] and "<defs>" in z[(0, 1)]


if __name__ == "__main__":
    _selbsttest()
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main()
