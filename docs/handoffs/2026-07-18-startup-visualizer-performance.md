# Handoff: Startup- und Visualizer-Performance

Date: 2026-07-18
Project: `/home/mt/projects/nira`

## Goal / current status

- Ziel: Den kurzen Plain-HTML-Flash beim Start, gelegentliche UI-Ruckler und den hohen RAM-/CPU-Verbrauch untersuchen und die belegten Ursachen minimal beheben.
- Branch/Commit: `master` auf `7b6ab40`.
- Implementierung ist fertig, aber nicht committed. Der Worktree enthält fünf gezielte Performance-/Build-Dateien und diese Handoff-Notiz.
- Ein optimiertes Release-Bundle wurde erfolgreich nach `target/dx/nira/release/linux/app/nira` gebaut. Der Launcher wählt es beim nächsten vollständigen Neustart automatisch.
- Die laufende Nira-Instanz zeigt noch auf das vor dem letzten Build gestartete, inzwischen ersetzte Binary. Die jüngsten UI-Optimierungen wurden deshalb noch nicht mit einer Vorher-/Nachher-Messung verifiziert.

## Files changed

- `nira/src/main.rs`
  - Verschiebt App-CSS, FontAwesome-CSS und Font-Faces in den initialen Dokument-Head via `with_custom_head`.
  - Entfernt die drei nachträglich gerenderten `document::Style`-Komponenten.
  - Ergänzt einen kleinen Regressionstest für kritisches CSS im initialen Head.
- `components/src/visualizer.rs`
  - Zerstört den Butterchurn/WebGL-Renderer und schließt den `AudioContext`, sobald die Visualizer-Instanz retired ist.
  - Begrenzt Butterchurn auf 30 Render-FPS, behält volle interne Auflösung und verliert beim Schließen explizit den WebGL-Kontext.
  - Lädt nur noch das 100-Preset-Basispaket; die Injektion sinkt von ungefähr 2,8 MB auf unter 1,1 MB.
  - Ergänzt einen kleinen Regressionstest für beide Cleanup-Aufrufe.
- `scripts/build-desktop.sh`
  - Überschreibt die erzeugte `libxdo.so.3` mit `cp -f`, weil Anvils ELF-Fixup das vorherige Bundle absichtlich schreibgeschützt hinterlässt.
- `hooks/src/use_player.rs`
  - Senkt die globale Player-Snapshot-Rate während Wiedergabe von ungefähr 8,3 auf 5 Aktualisierungen pro Sekunde; Idle bleibt bei 2 pro Sekunde.
- `nira/assets/css/base.css`
  - Entfernt permanente `will-change`-Promotion von Sidebar, Content und Player. Die eigentliche 150-ms-Suchanimation bleibt unverändert.
- `docs/handoffs/2026-07-18-startup-visualizer-performance.md`
  - Hält Diagnose, Messkontext und die spätere Vergleichsmessung fest.

## Files inspected

- `nira/src/main.rs` — Desktop-Konfiguration, initialer Head und App-Root.
- `components/src/visualizer.rs` — Butterchurn-Injektion, Renderloop und Lifecycle.
- `components/assets/` — Butterchurn-Skripte; zusammen ungefähr 2,8 MB.
- `components/src/bottombar.rs` — kurz wegen eines parallelen, inzwischen behobenen Build-Zwischenstands.
- `~/.local/bin/nira` — bevorzugt Release und fällt nur ohne Release-Binary auf Debug zurück.
- `~/.cache/nira/nira.log` — Style/Script-Warnungen und ein Audio-Buffer-Underrun während hoher Last.
- Lokale Dioxus-0.7.9-Quellen — `document::Style` wird im Desktop-Webview per queued effect/JavaScript erst nach dem Body-Patch eingefügt; `with_custom_head` landet direkt im initialen HTML.

## Key decisions / assumptions

- Ursache des Plain-HTML-Flashs: ungefähr 230 KB CSS wurden erst nach dem ersten Body-Render in den Head eingefügt. Die Korrektur gehört in den initialen Head, nicht in einen Lade-Overlay oder künstlichen Delay.
- Der Launcher-Befund war real: Bei der ersten Messung fehlte das Release-Bundle, daher lief Nira als unoptimierter Debug-Build. Das Release-Bundle existiert jetzt; erst ein kompletter Neustart aktiviert es.
- Der WebKit-Prozess lag mit aktivem Visualizer zeitweise bei ungefähr 60 % CPU. Diese Messung gehört zur alten Debug-Instanz und darf nicht als Messwert des neuen Release-Builds verwendet werden.
- Die spätere Release-Messung bestätigte die aktive Ursache genauer: Auf den 200/240-Hz-Monitoren lief Butterchurn via `requestAnimationFrame` mit Display- statt Datenrate, obwohl PCM nur mit 30 Hz ankommt.
- Der Visualizer betreibt einen Vollbild-WebGL-Renderloop und einen eigenen `AudioContext`. Beim Schließen wurden diese Ressourcen bislang nicht explizit freigegeben; das Cleanup ist jetzt ergänzt.
- Butterchurn-Globals bleiben absichtlich geladen. Ein Unload würde bei jedem erneuten Öffnen Parse- und Start-Ruckler erzeugen.
- User-Präferenz: Visualizer-Auflösung muss bei `textureRatio: 1` bleiben. Hohe Last ist bei bewusst aktivem Visualizer akzeptabler als sichtbar schlechte Qualität; CPU wird weiterhin auf 30 Render-FPS begrenzt.
- WebKitGTK hat einen merklichen Grundverbrauch. RSS mehrerer Prozesse nicht einfach addieren; für belastbare RAM-Vergleiche bevorzugt PSS verwenden.
- Im normalen Musikbetrieb waren zwei vermeidbare UI-Kosten sichtbar: drei dauerhaft promovierte große Compositor-Flächen und ein kompletter Bottombar-Diff alle 120 ms. Beides wurde minimal reduziert, ohne Audio- oder Visualizer-Qualität anzufassen.
- Projektvorgabe: schwere Checks und Builds ausschließlich über die benannten Anvil-Tasks ausführen, nicht lokal via Cargo/Dioxus.

## Commands run and results

- `anvil tests` — 68 Unit-Tests und alle Doc-Tests bestanden, inklusive der zwei neuen Regressionstests.
- `anvil release` — erster Lauf scheiterte an einem parallel bearbeiteten Zwischenstand mit fehlendem `QueuePopover::on_close`; lokal war der Callback danach bereits vorhanden.
- `anvil release` — zweiter Lauf erfolgreich; Release-Bundle gebaut, zurücksynchronisiert und ELF-Fixup angewandt.
- Release-Baseline mit laufender Musik und geschlossener UI: ungefähr 478 MB Gesamt-PSS und 4,8 % Gesamt-CPU (`nira` 136 MB/2,2 %, Netzwerk 45 MB/0,3 %, WebKit 297 MB/2,2 %).
- Visualizer aktiv, vor Drosselung: ungefähr 832 MB Gesamt-PSS und 114 % Gesamt-CPU (`nira` 142 MB/19,7 %, Netzwerk 46 MB/0,4 %, WebKit 648 MB/94,1 %).
- 30 Sekunden nach Schließen, vor verbessertem Cleanup: CPU fiel auf ungefähr 3 %, Speicher blieb jedoch bei 728 MB Gesamt-PSS. Rund 250 MB gegenüber der ursprünglichen Baseline blieben im WebKit-Prozess erhalten.
- `anvil tests` nach FPS-, Textur- und WebGL-Cleanup-Fix — alle Unit- und Doc-Tests bestanden.
- `anvil release` nach Visualizer-Fix — App wurde gebaut; erster Bundle-Versuch scheiterte nur an der schreibgeschützten generierten `libxdo.so.3`.
- `anvil release` nach `cp -f` — erfolgreich gebaut, zurücksynchronisiert und ELF-Fixup angewandt.
- Sauberer Release-Idle ohne Wiedergabe und ohne jemals geöffneten Visualizer: ungefähr 352 MB Gesamt-PSS und 0,3 % Gesamt-CPU.
- Wiedergabe ohne jemals geöffneten Visualizer: ungefähr 349 MB Gesamt-PSS und 3,5 % Gesamt-CPU; Musik allein erhöht den Speicher nicht relevant.
- Visualizer mit 30 FPS und `textureRatio: 0.5`: ungefähr 559 MB Gesamt-PSS und 54 % Gesamt-CPU; nach Schließen ungefähr 494 MB und 3 % CPU.
- Eine spätere Messung „Musik ohne Visualizer“ mit 535 MB war kein sauberer Baseline-Zustand: Das Log bestätigte, dass Butterchurn sieben Sekunden nach App-Start bereits injiziert worden war.
- `anvil tests` und `anvil release` nach Reduktion auf das Basispaket und Rückkehr zu voller Auflösung — erfolgreich.
- `anvil tests` nach Entfernen der permanenten Layer-Promotion und Senken des aktiven Snapshot-Pollings auf 200 ms — alle Unit- und Doc-Tests bestanden.
- `anvil release` mit diesen Normalbetrieb-Optimierungen — erfolgreich; Artefakt vom 18.07.2026 16:57 unter `target/dx/nira/release/linux/app/nira`.
- `git diff --check` — bestanden.
- `ps -eo pid,comm,%cpu,rss,args --sort=-%cpu` — alte laufende Instanz zuletzt ungefähr 133 MB RSS für `nira` und 402 MB RSS für den WebKit-Prozess; die frühere Visualizer-Spitze lag bei ungefähr 60 % WebKit-CPU. RSS ist hier nur ein Hinweis, kein sauberer PSS-Vergleich.

## Open blockers / risks

- Das finale Release mit voller Auflösung, 30 FPS, reduziertem Preset-Bundle und den beiden Normalbetrieb-Optimierungen ist gebaut, aber noch nicht nach einem Neustart gemessen.
- `dx` 0.7.6 meldet eine Versionsabweichung zu Dioxus 0.7.9, der Release-Build war dennoch erfolgreich. Nur anfassen, falls daraus ein konkreter Buildfehler entsteht.
- Änderungen sind nicht committed.

## Exact next steps

1. Sicherstellen, dass kein anderer Prozess den Bereich bearbeitet; `git status --short --branch` prüfen.
2. Nira vollständig schließen und über `~/.local/bin/nira` neu starten. Im Launcher-Log bestätigen, dass das Release-Bundle läuft.
3. Nach 30 Sekunden Idle CPU sowie PSS/RSS von Nira, WebKitWebProcess und WebKitNetworkProcess erfassen.
4. Zuerst Musik ohne jemals geöffneten Visualizer starten und gegen die saubere Baseline von ungefähr 349 MB PSS / 3,5 % CPU vergleichen.
5. Den Visualizer mit demselben Preset für 30 Sekunden aktivieren und dieselben Werte erfassen.
6. Visualizer schließen und nach weiteren 30 Sekunden prüfen, ob CPU zurückfällt und Speicher stabilisiert/freigegeben wird.
7. Volle Auflösung beibehalten. Nur wenn aktive CPU trotz 30-FPS-Deckel praktisch stört, Render-FPS weiter senken; nicht erneut die Auflösung reduzieren.

## Useful resume commands

```sh
git status --short --branch
git diff --stat
git diff -- nira/src/main.rs components/src/visualizer.rs
ps -eo pid,comm,%cpu,rss,args --sort=-%cpu | rg 'nira|WebKit|webkit'
tail -100 ~/.cache/nira/launcher.log
tail -100 ~/.cache/nira/nira.log
anvil tests
anvil release
```
