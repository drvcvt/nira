# worum es bei einem musikplayer wirklich geht

checke ich. ein musikplayer ist auf dem papier „datei rein, ton raus“. praktisch ist er ein deadline-sensitives audio-system, eine mediendatenbank, ein netzwerkclient und ein auffangbecken für tags, die seit 2004 von unterschiedlich kaputten programmen misshandelt wurden.

für einen lokalen desktop-player mit optionalem streaming würde ich ungefähr so bauen:

```text
ui / cli / media-keys / mpris
              │
       commands + events
              │
      player state machine
       ├── queue / history / shuffle
       ├── library / sqlite / search
       ├── metadata / artwork
       ├── network / cache
       └── decode worker
              │
       dsp / replaygain / resample
              │
       bounded spsc pcm ring
              │
        audio callback
              │
          audio device
```

die wichtigste trennung ist:

* **control plane:** ui, queue, datenbank, netzwerk, decoder, dateisystem
* **audio plane:** bereits dekodierte pcm-samples rechtzeitig ans gerät liefern

der audio-callback darf nicht plötzlich anfangen, dateien zu öffnen, json zu parsen oder philosophisch über einen mutex nachzudenken.

## die goldene regel des audio-callbacks

im callback:

```text
audio_callback(output):
    written = pcm_ring.pop_into(output)

    if written < output.length:
        fill_silence(output[written:])
        underrun_counter += 1
```

mehr sollte dort möglichst nicht passieren.

kein:

* `malloc`, `new`, `vec::push`
* datei- oder netzwerkzugriff
* logging
* blockierender mutex
* warten auf condition variables
* datenbankzugriff
* decoderaufruf
* ui-event pro sampleblock

jack und apple schreiben für echtzeit-audio explizit vor, blockierende operationen, speicherallokationen und synchrones i/o aus dem callback herauszuhalten. auch cpal führt den callback typischerweise auf einem eigenen hoch priorisierten thread aus. ([JACK Audio][1])

zwischen decoder und callback eignet sich ein begrenzter spsc-ringbuffer. `rtrb` ist in rust genau dafür ausgelegt: feste kapazität, nach dem erstellen keine allokationen, ein producer und ein consumer. ([Docs.rs][2])

# was der player können sollte

## brauchbares mvp

das ist der teil, ohne den du keinen musikplayer hast, sondern einen demo-button mit lautsprechergeräuschen:

* play, pause, stop
* seek
* nächster und vorheriger track
* queue
* shuffle und repeat
* lautstärke und mute
* dateien, ordner und playlists öffnen
* lokale library scannen
* suche nach titel, artist, album und dateiname
* metadata und cover anzeigen
* audioausgabegerät wählen
* zustand zwischen starts wiederherstellen
* kaputte oder nicht unterstützte dateien sauber überspringen
* hardware-media-keys und betriebssystemintegration
* klare zustände für loading, buffering, playing, paused und error

## dinge, die einen player tatsächlich gut machen

* gapless playback
* preload des nächsten tracks
* replaygain für track und album
* zuverlässiges seeking bei vbr-dateien
* device-hotplug und recovery
* mehrere artists pro track
* album artist, discnummer und compilation-support
* schnelle, indexierte library-suche
* playlists und queue-persistenz
* tag-editing ohne datenverlust
* cover-cache mit thumbnails
* konsistente shuffle-history
* abbruch alter decoder- und netzwerkjobs beim trackwechsel
* brauchbare fehlermeldungen statt „playback failed“, danke für nichts

foobar2000 und strawberry führen unter anderem gapless playback, replaygain, tagging, library-organisation, shortcuts, dsp, covers, lyrics und netzwerkfunktionen als zentrale player-features. das ist ein brauchbarer indikator dafür, was nutzer bei einem ausgereiften desktop-player erwarten. ([foobar2000.org][3])

## später, nicht zuerst

* crossfade
* equalizer
* visualizer
* lyrics-provider
* scrobbling
* smart playlists
* internetradio
* remote-control-api
* waveform-vorschau
* transcoding
* casting
* plugin-system
* cloud-sync

ein 31-band-eq bringt wenig, wenn seek manchmal drei sekunden alte samples ausspuckt. menschen lieben sichtbare features, der audio-thread liebt dagegen langweilige korrektheit.

# die wichtigsten performancefallen

## 1. zu viel arbeit im audio-thread

das klassische problem:

```text
callback
  -> lock global player state
  -> decode next packet
  -> update ui
  -> maybe log something
```

damit funktionieren zunächst zwölf mp3s auf deinem rechner. dann öffnet jemand ein großes png-cover, der lock hängt kurz, und der player entdeckt avantgardistische klickgeräusche.

besser:

* callback liest nur aus einem vorallokierten ringbuffer
* decoder arbeitet auf einem separaten worker
* ui kommuniziert über commands und coalesced events
* statistik über atomics oder lockfreie queues
* keine unbeschränkten channels

## 2. falsche buffergröße

kleine buffer reduzieren latenz, erhöhen aber die gefahr von underruns. große buffer sind stabiler, machen seek, pause, lautstärkeänderungen und formatwechsel aber träger. mpv dokumentiert genau diesen trade-off und verwendet standardmäßig einen vergleichsweise großen audiobuffer, weil audio-player keine gitarrenverstärker sind. ([mpv][4])

sinnvolle strategie:

* ringbufferkapazität konfigurierbar machen
* zielbereich in millisekunden definieren, nicht in beliebigen chunks
* low-watermark und high-watermark messen
* decoding stoppen, wenn der buffer voll ist
* decoding priorisieren, wenn er sich leert
* underruns sichtbar zählen

für normalen desktop-playback sind etwa 100 bis 500 ms interne reserve meist vernünftiger als zwanghafte 5 ms. wichtiger als eine magische zahl ist, dass du den tatsächlichen bufferfüllstand misst.

## 3. stale work nach seek oder trackwechsel

beispiel:

1. track a wird dekodiert
2. user springt zu track b
3. alter decoder-job für a liefert noch ein paket
4. das paket landet nach dem seek wieder im ringbuffer
5. für 30 ms lebt track a erneut. geistermusik, sehr premium.

verwende für jeden playback-abschnitt eine generation:

```text
generation += 1

decode_job {
    generation,
    source,
    target_position
}
```

jedes decoderergebnis trägt dieselbe generation. stimmt sie beim konsumieren nicht mehr, wird das ergebnis verworfen.

beim seek:

1. generation erhöhen
2. decoder abbrechen
3. pcm-ring leeren
4. demuxer seeken
5. decoderzustand flushen
6. bis zum gewünschten sample dekodieren
7. audio wieder freigeben

ffmpeg unterscheidet zwischen schnellem und genauem seek. container-timestamps, timebases und keyframes machen seek komplizierter als `position = x`. überraschend, dateiformate haben sich nicht kollektiv auf vernunft geeinigt. ([FFmpeg][5])

## 4. gapless playback falsch verstehen

„keine sichtbare pause“ ist nicht automatisch gapless.

richtiges gapless braucht:

* exakte anzahl gültiger samples
* encoder-delay und padding berücksichtigen
* nächsten track vorladen
* ausgabegerät offen halten
* keine zusätzlichen nullsamples zwischen tracks
* decodergrenzen samplegenau behandeln

opus verlangt beispielsweise, dass eine definierte anzahl an pre-skip-samples verworfen wird. ignorierst du das, stimmen grenzen und seek-positionen nicht exakt. ([RFC Editor][6])

mpv und gstreamer lösen gapless unter anderem durch vorladen, puffern und dekodieren des folgenden tracks. formatwechsel können trotzdem ein reopen oder resampling erzwingen. ([mpv][4])

crossfade ist ein separater modus. gapless erhält die ursprüngliche albumgrenze. crossfade überlappt sie absichtlich.

## 5. verstecktes resampling und formatkonvertierungen

jede unnötige konvertierung kostet cpu und kann zusätzliche buffer erzeugen:

```text
source int16
 -> decoder float32 planar
 -> dsp float32 interleaved
 -> resampler float32 planar
 -> output int16
```

das kann korrekt sein, sollte aber bewusst passieren.

definiere:

* internes pcm-format
* interleaved oder planar
* channel-layout, nicht nur channel-count
* ziel-samplerate
* resampling-policy
* dithering-policy
* bit-perfect-policy

wenn quelle und gerät unterschiedliche samplerates verwenden, muss irgendwo resampled werden. mpv fügt dafür einen resampler ein und dokumentiert den qualitäts- und performance-trade-off. rubato bietet für echtzeitbetrieb explizit eine api mit vorallokiertem ausgabebuffer an, damit die heap-allokation nicht spontan im falschen moment menschliche kreativität beweist. ([mpv][4])

für einen normalen player ist `f32` als internes dsp-format bequem. für einen echten bit-perfect-modus solltest du dsp, replaygain, eq, crossfade und resampling umgehen und ein kompatibles geräteformat verwenden. strawberry behandelt bit-perfect playback und das Vermeiden unnötigen resamplings ausdrücklich als eigenes qualitätsziel. ([wiki.strawberrymusicplayer.org][7])

## 6. library-scan mit einer db-transaktion pro datei

das hier ist langsam:

```text
for file in files:
    begin
    parse_tags(file)
    insert(file)
    commit
```

sqlite kann viele inserts schnell ausführen, aber einzelne durable transactions verursachen erheblichen overhead. scans sollten writes bündeln. ([SQLite][8])

besser:

* scanner enumeriert dateien
* begrenzter worker-pool liest tags
* ergebnisse gehen in eine bounded queue
* db-writer schreibt batches, etwa 100 bis 1000 einträge
* bestehende dateien über `mtime`, größe und file-id erkennen
* hashes nur berechnen, wenn wirklich nötig
* scan fortsetzbar machen
* fortschritt und fehler getrennt speichern

mit sqlite:

* wal-modus für bessere read/write-concurrency
* fts5 für suche
* passende zusammengesetzte indexes
* regelmäßig `pragma optimize`
* wal nicht als magischen netzwerkdateisystem-trick missbrauchen

wal erlaubt parallele leser und schreiber, ist aber nicht für eine zwischen mehreren hosts geteilte datenbank auf einem netzwerkdateisystem gedacht. fts5 stellt sqlite-intern volltextsuche bereit. ([SQLite][9])

## 7. cover-art

cover sind häufig ein größerer performancefresser als das audio selbst, weil irgendein genie ein 9000-mal-9000-pixel-png in eine mp3 eingebettet hat.

regeln:

* maximale komprimierte dateigröße
* maximale pixelanzahl
* maximale dimension
* bilddekodierung nie im ui-thread
* thumbnails einmalig erzeugen
* mehrere thumbnailgrößen cachen
* lru-cache mit speicherlimit
* listenansicht nie mit originalbildern rendern
* fehlgeschlagene bilder negativ cachen
* original-cover nicht blind als große blobs in sqlite duplizieren

für eine listenansicht reichen meist 64 bis 256 pixel. das 38-mb-cover darf gern existieren, es muss nur nicht sechzigmal gleichzeitig dekodiert werden.

## 8. große listen und ui-updates

bei 100.000 tracks:

* rows virtualisieren
* keine widgets für unsichtbare einträge erzeugen
* cover lazy laden
* suchanfragen debouncen
* sortierung und filterung möglichst in sqlite erledigen
* progress nicht bei jedem audioblock aktualisieren
* visualizer nicht mit callback-rate rendern

für die positionsanzeige reichen normalerweise 10 bis 30 updates pro sekunde. die position sollte aus der audio-clock oder dem geschätzten dac-zeitpunkt entstehen, nicht aus „seit dem klick auf play vergangene wandzeit“. cpal kann callback- und geschätzte dac-timestamps liefern. ([Docs.rs][10])

## 9. file-watcher als einzige wahrheit

filesystem-watcher sind hinweise, keine göttliche offenbarung.

unter windows kann der interne watcher-buffer überlaufen. dann gehen events verloren und eine vollständige neue enumeration ist nötig. ähnliche overflow- und rename-probleme existieren auch bei anderen watcher-systemen. ([Microsoft Learn][11])

verwende daher:

* watcher-events debouncen
* rename-paarung unterstützen
* events zu „pfad möglicherweise geändert“ zusammenfassen
* periodische reconciliation
* vollständigen rescan bei overflow
* rescan idempotent gestalten

## 10. streaming

für remote-dateien brauchst du zusätzlich:

* http-range-requests für seek
* begrenzten memory- und disk-cache
* cancellation
* reconnect mit backoff
* getrennten zustand für `buffering`
* timeout und stall-erkennung
* token-refresh außerhalb des audiopfads
* cache-validierung
* schutz vor endlosen oder falschen content-length-angaben

http-range-requests erlauben das Abrufen bestimmter bytebereiche und bilden damit die basis für seekbares dateistreaming. für laufende streams ist hls eine weitere typische quelle. ffmpeg unterstützt seekability und diverse reconnect-optionen für http. ([RFC Editor][12])

# state-management

verwende keinen haufen bools:

```text
is_playing
is_paused
is_loading
is_buffering
has_error
is_seeking
```

damit kannst du irgendwann gleichzeitig pausiert, ladend, spielend und tot sein.

besser:

```text
enum playback_state {
    stopped,
    opening,
    buffering,
    playing,
    paused,
    seeking,
    draining,
    error
}
```

dazu separat:

```text
playback_generation
active_track_id
requested_position
decoded_position
submitted_position
device_position
```

die positionen sind nicht dasselbe:

* **source position:** position im container
* **decoded position:** bis wohin bereits dekodiert wurde
* **submitted position:** bis wohin samples ans gerät übergeben wurden
* **device position:** was der nutzer gerade tatsächlich hört

# queue, shuffle und history

queue, playlist und history sollten getrennte konzepte sein.

für shuffle:

1. aus der aktuellen queue eine konkrete zufallsreihenfolge erzeugen
2. index in dieser reihenfolge speichern
3. `previous` geht tatsächlich zum vorherigen track
4. neu hinzugefügte tracks kontrolliert einsortieren
5. seed oder reihenfolge persistieren

nicht bei jedem `next` zufällig irgendeinen song ziehen. sonst ist `previous` semantisch ungefähr so stabil wie ein social-media-feed.

repeat sollte klar getrennt sein:

```text
off
queue
track
```

# metadata und datenmodell

speichere nicht einfach alles in einer `songs`-tabelle. eine brauchbare basis:

```text
files
    id
    path
    stable_file_id
    size
    mtime
    codec
    sample_rate
    channels
    duration
    scan_status

tracks
    id
    file_id
    title
    album_id
    disc_number
    track_number
    date
    replaygain_track
    replaygain_album

artists
    id
    name
    normalized_name

track_artists
    track_id
    artist_id
    role
    ordering

albums
    id
    title
    album_artist_id
    release_date

artwork
    id
    source
    cache_key
    width
    height

playlists
playlist_items
queue_state
play_history
play_stats
```

wichtig:

* mehrere artists unterstützen
* displaywerte und normalisierte suchwerte trennen
* tracknummer und discnummer numerisch speichern
* „1/12“ nicht als mystischen string behandeln
* dateipfad nicht als dauerhafte identität verwenden
* unbekannte tags beim schreiben möglichst erhalten
* formatabhängige rohwerte oder zusätzliche tag-map bewahren

vorbis comments erlauben denselben key mehrfach. das ist unter anderem für mehrere artists relevant. die generische tag-api von taglib deckt außerdem nur einen gemeinsamen teil der formatspezifischen metadata ab. eine zu aggressive normalisierung kann deshalb informationen verlieren. ([XiphWiki][13])

taglib unterstützt id3, ape, flac, xiph, mp4 und weitere übliche metadatenformate. ([TagLib][14])

# replaygain und lautstärke

mindestens unterstützen:

* track gain
* album gain
* peak-value
* clipping prevention
* preamp
* fallback bei fehlenden tags

track-modus normalisiert einzelne songs. album-modus erhält lautstärkeunterschiede innerhalb eines albums.

gain-änderungen sollten über einige millisekunden gerampt werden:

```text
current_gain -> target_gain
```

ein harter sprung produziert clicks.

replaygain spezifiziert track- und album-gain sowie clipping-vermeidung. mpv implementiert diese modi ebenfalls. für eigene loudness-analyse kann `libebur128` ebu-r128-messungen durchführen. ([wiki.hydrogenaudio.org][15])

# betriebssystemintegration

ein desktop-player sollte sich wie ein media-player verhalten, nicht wie ein eigenbrötlerisches terminalprogramm mit fenster.

* linux: mpris über d-bus
* windows: system media transport controls
* macos: now playing information center
* hardware play/pause/next/previous
* lockscreen- und system-metadata
* systemweite volume- und playback-events

mpris definiert eine standardisierte schnittstelle für playback, metadata, tracklisten und playlists. windows smtc und apples now-playing-api erfüllen ähnliche integrationsrollen. ([specifications.freedesktop.org][16])

alle externen controls sollten dieselben commands verwenden wie die ui:

```text
play
pause
seek
skip_next
skip_previous
set_volume
```

keine zweite parallele playback-logik nur für media-keys. das wäre doppelte fehlerproduktion mit weniger übersicht.

# ein prozess oder backend-daemon

## ein prozess reicht, wenn

* es genau eine ui gibt
* playback nur läuft, solange die app läuft
* du keine remote-clients brauchst
* crash-isolation keine priorität ist

module können trotzdem sauber getrennt sein.

## daemon plus ipc lohnt sich, wenn

* playback ohne ui weiterlaufen soll
* mehrere clients gleichzeitig steuern
* cli, gui und web-interface denselben core nutzen
* ui-crash die wiedergabe nicht stoppen soll
* du eine headless-servervariante planst

das ipc sollte commands und events übertragen, nicht beliebigen zugriff auf globalen zustand geben.

```text
client -> command { request_id, type, payload }
server -> event   { sequence, type, payload }
server -> result  { request_id, result }
```

# stack-empfehlungen

## c++ mit möglichst viel kontrolle

```text
ffmpeg
  libavformat
  libavcodec
  libswresample

miniaudio oder native audio-api
taglib
sqlite
qt / imgui / eigene ui
```

vorteile:

* breite formatunterstützung
* genaue kontrolle über demuxing, decoding und timestamps
* eigene audio-pipeline
* wenig versteckte magie

nachteile:

* clocking, seek, buffering, gapless und recovery gehören dann dir
* ffmpeg-api ist kein wellnessbereich

ffplay ist selbst im wesentlichen ein kleiner test-player auf basis von ffmpeg und sdl. er ist als referenz für grundlegende demux-, decode- und playbackabläufe brauchbar, aber nicht als fertige produktarchitektur. ([FFmpeg][17])

## c++ mit weniger eigener pipeline-arbeit

```text
gstreamer
qt
taglib
sqlite
```

gstreamer eignet sich gut, wenn du:

* viele container und codecs willst
* streaming planst
* fertige pipeline-elemente nutzen willst
* weniger audiogeräte- und decoderplumbing schreiben möchtest

die gstreamer-dokumentation beschreibt explizit player-anwendungen und modulare pipelines. ([gstreamer.freedesktop.org][18])

## rust

```text
symphonia
cpal
rtrb
rubato
sqlite-binding
mature tag-library oder taglib-binding
ui-framework deiner wahl
```

rollen:

* `symphonia`: demuxing und decoding
* `cpal`: audio-device und callback
* `rtrb`: lockfreier pcm- oder command-ringbuffer
* `rubato`: resampling
* `sqlite`: library und suche

symphonia ist eine pure-rust demux- und decoderbibliothek. gapless-support hängt dort vom jeweiligen format und codec ab, was du pro format testen solltest. ([Docs.rs][19])

`rodio` ist für einen simpleren player oder prototyp angenehm. bei eigenem clocking, genauer gapless-logik, custom-dsp und umfangreicher device-recovery ist die niedrigere schicht mit cpal meist kontrollierbarer. rodio besitzt aber auch decoder- und gapless-funktionen. ([Docs.rs][20])

# externe metadata

musicbrainz und cover art archive können fehlende metadata und covers ergänzen.

dabei:

* niemals playback blockieren
* ergebnisse cachen
* user-agent korrekt setzen
* request-rate begrenzen
* matches als vorschlag behandeln
* lokale tags nicht kommentarlos überschreiben
* release-id speichern, nicht nur text vergleichen

der musicbrainz-webservice begrenzt normale clients auf ungefähr eine anfrage pro sekunde und verlangt einen identifizierbaren user-agent. cover art archive bietet passende cover-metadaten. ([MusicBrainz][21])

# sicherheit und robuste dateiverarbeitung

medien, tags und bilder sind nicht vertrauenswürdig, nur weil sie `.flac` heißen.

begrenze:

* metadata-länge
* anzahl der tagfelder
* covergröße und pixelanzahl
* maximale playlistgröße
* verschachtelung bei playlistformaten
* cachegröße
* redirects
* downloadgröße
* decoderzeit pro datei

tag-writing:

1. in temporäre datei schreiben
2. originalberechtigungen erhalten
3. daten flushen
4. atomisch ersetzen
5. bei fehler original behalten
6. unbekannte metadata möglichst nicht zerstören

decoder- und parseradapter sollten mit fuzzing und sanitizern getestet werden. oss-fuzz verwendet genau diese kombination, um crashes und sicherheitsprobleme in c++- und rust-projekten zu finden. flac stellt zusätzlich offizielle conformance-testdateien bereit. ([GitHub][22])

bei ffmpeg musst du beim release die buildkonfiguration prüfen. standardmäßig ist ffmpeg überwiegend unter lgpl nutzbar, bestimmte optionale komponenten aktivieren aber gpl- oder nicht redistribuierbare konfigurationen. ([FFmpeg][23])

# tests, die du wirklich brauchst

## audio

* leere oder extrem kurze datei
* abgeschnittene datei
* beschädigte frames
* vbr-mp3
* gapless-album
* opus mit pre-skip
* flac mit seektable
* wechselnde samplerates
* wechselnde channel-layouts
* mono, stereo und multichannel
* seek auf anfang, mitte und ende
* seek-spam
* schneller trackwechsel
* pause während buffering
* gerät während playback entfernen
* ausgabegerät während playback wechseln
* decoder liefert langsamer als realtime
* decoder liefert schneller als ringbuffer aufnehmen kann

## metadata

* mehrere artists
* fehlender title
* ungültiges utf-8
* sehr lange tags
* eingebettete nullbytes
* mehrere cover
* riesiges cover
* falscher mime-type
* tracknummer `03/12`
* discnummer ohne album
* doppelte dateien
* symlink-schleifen
* case-sensitive und case-insensitive pfade

## library und datenbank

* scan abbrechen und fortsetzen
* datei während scan löschen
* ordner während scan umbenennen
* watcher-overflow
* removable drive verschwindet
* datenbankmigration schlägt fehl
* rollback
* 100.000 bis 1.000.000 testtracks
* parallele suche während scan
* playlist mit fehlenden dateien

## deterministische tests

baue einen null-audio-sink:

```text
decoder -> dsp -> null sink
```

damit kannst du:

* sampleanzahl prüfen
* gapless-grenzen vergleichen
* seek-toleranz messen
* pcm-hashes für lossless-dateien bilden
* playback ohne physisches audiogerät testen
* eine virtuelle clock verwenden

# metriken, die direkt in den player gehören

mindestens intern messen:

```text
audio_underruns
audio_ring_fill_min
audio_ring_fill_max
decode_time_per_audio_second
time_to_first_audio
seek_latency_ms
track_switch_latency_ms
device_recovery_time_ms
library_scan_files_per_second
database_query_p95_ms
artwork_decode_p95_ms
artwork_cache_hit_ratio
network_buffer_seconds
stale_decode_results_discarded
```

damit weißt du, ob etwas besser geworden ist. „fühlt sich irgendwie flotter an“ ist keine performanceanalyse, sondern wettervorhersage.

# was du möglichst vermeiden solltest

* locks im audio-callback
* unbounded queues
* decoder und ui auf demselben thread
* cover-decode im ui-thread
* eine db-transaktion pro track
* vollständiges hashing bei jedem start
* watcher als einzige datenquelle
* direkte mutable globals zwischen allen modulen
* seek nur in der ui anzeigen, ohne decoder zu flushen
* alten decode-output nach trackwechsel akzeptieren
* shuffle bei jedem `next` neu würfeln
* progress anhand der wall-clock berechnen
* alle covers dauerhaft im ram halten
* tags beim editieren komplett neu aufbauen
* stillschweigend resamplen und „bit-perfect“ draufschreiben
* plugin-system bauen, bevor der interne core stabil ist
* netzwerkzugriff im playback-critical-path
* visualizer-fft im audio-callback
* logging für jeden audioblock
* fehlerzustände als fünf widersprüchliche bools modellieren

# sinnvolle implementierungsreihenfolge

## phase 1: playback-core

* eine lokale datei öffnen
* dekodieren
* ringbuffer
* audio-callback
* pause und stop
* null-sink
* underrun-messung

## phase 2: korrekte steuerung

* explizite state machine
* seek
* generationen und cancellation
* queue
* shuffle-history
* devicewechsel
* hotplug-recovery

## phase 3: library

* scanner
* metadata
* sqlite
* batching
* fts-suche
* cover-thumbnails
* watcher plus reconciliation

## phase 4: audioqualität

* gapless
* preload
* replaygain
* gain-ramping
* resampler-policy
* bit-perfect-bypass
* trackwechsel zwischen unterschiedlichen formaten

## phase 5: integration

* mpris, smtc und now playing
* playlists
* tag-editor
* metadata-provider
* streaming-cache
* scrobbling

## phase 6: spielzeug

* eq
* crossfade
* lyrics
* visualizer
* waveform
* plugins
* remote-control

# schnelle selbstprüfung

deine aktuelle architektur ist vermutlich auf einem guten weg, wenn diese punkte stimmen:

* [ ] der audio-callback allokiert nicht und blockiert nie
* [ ] pcm-puffer sind begrenzt und ihr füllstand ist messbar
* [ ] seek und trackwechsel invalidieren alte decodergebnisse
* [ ] playback hat eine explizite state machine
* [ ] queue, shuffle-reihenfolge und history sind getrennt
* [ ] der nächste track wird vor gapless-übergängen vorgeladen
* [ ] resampling und formatkonvertierung passieren bewusst
* [ ] library-writes werden gebatcht
* [ ] große listen sind virtualisiert
* [ ] cover werden als begrenzte thumbnails gecacht
* [ ] watcher können durch einen rescan korrigiert werden
* [ ] device-disconnects führen nicht zum crash
* [ ] metadata unterstützt mehrere artists und unbekannte tags
* [ ] ui und system-controls verwenden denselben command-pfad
* [ ] underruns, seek-latenz und startup-latenz werden gemessen
* [ ] tag-writing kann das original bei einem crash nicht zerstören

der kern ist am ende ziemlich unspektakulär: decode außerhalb des echtzeitpfads, bounded buffers, explizite zustände, samplegenaue zeitrechnung, eine vernünftige datenbank und keinerlei vertrauen in dateien. genau diese langweiligen stellen entscheiden später, ob dein player solide wirkt oder bei jedem dritten seek akustisch implodiert.

[1]: https://jackaudio.org/api/group__ClientCallbacks.html?utm_source=chatgpt.com "Setting Client Callbacks"
[2]: https://docs.rs/rtrb/?utm_source=chatgpt.com "rtrb - Rust"
[3]: https://www.foobar2000.org/?utm_source=chatgpt.com "foobar2000"
[4]: https://mpv.io/manual/stable/ "mpv.io"
[5]: https://ffmpeg.org/ffmpeg.html?utm_source=chatgpt.com "ffmpeg Documentation"
[6]: https://www.rfc-editor.org/info/rfc7845/?utm_source=chatgpt.com "RFC 7845: Ogg Encapsulation for the Opus Audio Codec"
[7]: https://wiki.strawberrymusicplayer.org/wiki/Differences_from_Clementine?utm_source=chatgpt.com "Differences from Clementine - Strawberry Music Player Wiki"
[8]: https://sqlite.org/faq.html?utm_source=chatgpt.com "Frequently Asked Questions"
[9]: https://sqlite.org/wal.html?utm_source=chatgpt.com "Write-Ahead Logging"
[10]: https://docs.rs/cpal/latest/cpal/struct.OutputCallbackInfo.html?utm_source=chatgpt.com "OutputCallbackInfo in cpal - Rust"
[11]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw?utm_source=chatgpt.com "ReadDirectoryChangesW function (winbase.h) - Win32 apps"
[12]: https://www.rfc-editor.org/rfc/rfc9110.html?utm_source=chatgpt.com "RFC 9110: HTTP Semantics"
[13]: https://wiki.xiph.org/Metadata?utm_source=chatgpt.com "Metadata - XiphWiki - Xiph.org"
[14]: https://taglib.org/api/?utm_source=chatgpt.com "TagLib API Documentation"
[15]: https://wiki.hydrogenaudio.org/index.php?title=Revised_ReplayGain_specification&utm_source=chatgpt.com "Revised ReplayGain specification"
[16]: https://specifications.freedesktop.org/mpris/latest/?utm_source=chatgpt.com "MPRIS D-Bus Interface Specification — v2.2"
[17]: https://ffmpeg.org/ffplay.html?utm_source=chatgpt.com "ffplay Documentation"
[18]: https://gstreamer.freedesktop.org/documentation/application-development/?utm_source=chatgpt.com "Application Development Manual"
[19]: https://docs.rs/symphonia/?utm_source=chatgpt.com "symphonia - Rust"
[20]: https://docs.rs/rodio/latest/rodio/decoder/struct.Decoder.html?utm_source=chatgpt.com "Decoder in rodio::decoder - Rust"
[21]: https://musicbrainz.org/doc/MusicBrainz_API?utm_source=chatgpt.com "MusicBrainz API"
[22]: https://github.com/google/oss-fuzz?utm_source=chatgpt.com "OSS-Fuzz - continuous fuzzing for open source software."
[23]: https://www.ffmpeg.org/legal.html?utm_source=chatgpt.com "FFmpeg License and Legal Considerations"
