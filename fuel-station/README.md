# ShotExpress Tankstelle — Firmware-Anleitung (ESP32/Arduino)

Die Tankstelle steht an der Bar und befüllt die Shotgläser auf dem Zug.
Der Server fährt den Zug mit Millimeter-Kommandos unter den Auslauf und
schickt dir dann pro Glas ein einziges Kommando: **„gieße X ml"**. Deine
Firmware führt es aus und meldet zurück, ob es geklappt hat. Mehr nicht —
keine Logik, keine Entscheidungen. Das Gerät ist (wie der Zug) ein „dummer
Ausführer".

## MQTT in 5 Sätzen

MQTT ist ein Nachrichtensystem mit einem zentralen Vermittler, dem
**Broker** (läuft bei uns auf dem Server-Laptop, Port 1883). Geräte
verbinden sich zum Broker und **abonnieren** (subscribe) oder
**veröffentlichen** (publish) Nachrichten auf **Topics** — das sind einfach
Namen wie `shotexpress/fuel/command`. Wer ein Topic abonniert hat, bekommt
jede Nachricht, die irgendwer darauf veröffentlicht. Es gibt keine direkte
Verbindung zwischen Geräten, alles läuft über den Broker. Für Arduino gibt
es fertige Bibliotheken; du brauchst nur `begin()`, `connect()`,
`subscribe()`, `publish()` und `loop()`.

**Bibliotheken** (gleiche wie im Zug, `Train/Train.ino` ist eine gute
Vorlage): `MQTT` von Joël Gähwiler (im Library Manager: „MQTT" von 256dpi)
und `ArduinoJson` zum Parsen/Erzeugen der Nachrichten.

**Zugangsdaten** (dev-Setup): Host = IP des Server-Laptops, Port `1883`,
Username `admin`, Passwort `123`. Client-ID z. B. `"fuel-station"`.

## Topics

| Topic | Richtung | Inhalt |
|---|---|---|
| `shotexpress/fuel/command` | du **abonnierst** | Kommandos vom Server (JSON) |
| `shotexpress/fuel/event` | du **veröffentlichst** | Lebenszyklus-Antworten (JSON) |
| `shotexpress/fuel/status` | du **veröffentlichst** | Heartbeat, 1× pro Sekunde (JSON) |
| `shotexpress/fuel/sensor` | du **veröffentlichst** | Ultraschall-Abstand in mm, reine Zahl (kein JSON), ~4× pro Sekunde, `-1` = kein Echo — nur zur Anzeige im Web-UI |

## Kommandos (kommen rein)

Gieße 40 ml:

```json
{
  "msg_id": "11111111-2222-4333-8444-555555555555",
  "cmd_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
  "seq": 3,
  "ts_ms": 1751500000000,
  "cmd_type": "dispense",
  "volume_ml": 40,
  "ttl_ms": 15000
}
```

Sofort-Stopp (Ventil zu, egal was läuft):

```json
{ "msg_id": "…", "cmd_id": "…", "seq": 4, "ts_ms": 1751500000000, "cmd_type": "stop" }
```

Die vier Umschlag-Felder (`msg_id`, `cmd_id`, `seq`, `ts_ms`) stecken in
jeder Nachricht des Systems:

- **`msg_id`** — eindeutige ID *dieser Nachricht*. Beim Empfangen: merke
  dir die letzten IDs und ignoriere Duplikate (MQTT darf doppelt zustellen).
  Beim Senden: selbst eine würfeln (siehe unten).
- **`cmd_id`** — ID des *Kommandos*. In deinen Antworten **unverändert
  zurückkopieren** — daran erkennt der Server, worauf du antwortest.
- **`seq`** — dein eigener Zähler, bei jedem Senden +1.
- **`ts_ms`** — Unixzeit in Millisekunden. Auf dem ESP32 bekommst du sie
  per NTP, einmalig im `setup()` nach dem WLAN:
  `configTime(0, 0, "pool.ntp.org");` und dann `time(nullptr) * 1000ULL`.

**TTL-Prüfung:** Ist beim Empfang `jetzt > ts_ms + ttl_ms`, ist das
Kommando verfallen (lag z. B. wegen WLAN-Ausfall beim Broker) — **nicht
gießen**, nur `expired` melden.

## Antworten (gehen raus, auf `shotexpress/fuel/event`)

Immer mit frischer `msg_id`, kopierter `cmd_id`, eigenem `seq` und `ts_ms`.
Das Feld `exec_type` sagt, was passiert ist:

1. `{"…umschlag…", "exec_type": "accepted"}` — sofort nach Empfang, „habe verstanden, gieße jetzt".
2. `{"…umschlag…", "exec_type": "completed"}` — Sollmenge erreicht, Ventil zu.
3. `{"…umschlag…", "exec_type": "failed", "error": {"code": "TANK_EMPTY", "reason": "reservoir below cutoff"}}` — ging nicht. Bekannte Codes: `TANK_EMPTY`, `POUR_TIMEOUT` (harte Obergrenze der Gießzeit gerissen — Sicherheitsnetz gegen klemmendes Ventil), `NO_GLASS` (kein Glas unter dem Auslauf, laut Ultraschallsensor).
4. `{"…umschlag…", "exec_type": "cancelled"}` — ein `stop` hat einen laufenden Guss abgebrochen.
5. `{"…umschlag…", "exec_type": "expired"}` — Kommando war beim Empfang schon verfallen.

Auf jedes `dispense` folgt also `accepted` und danach **genau eine**
End-Antwort (`completed`/`failed`/`cancelled`) — oder direkt `expired`.

## Heartbeat (1× pro Sekunde, auf `shotexpress/fuel/status`)

```json
{ "msg_id": "…", "cmd_id": "", "seq": 12, "ts_ms": 1751500000000,
  "state": "IDLE", "tank_pct": 63 }
```

`cmd_id` ist hier der leere String (kein Kommando aktiv? dann immer `""`;
während eines Gusses die aktive `cmd_id`). `state` ist `IDLE`,
`DISPENSING` oder `ERROR`; `tank_pct` grob geschätzt reicht.

## Regeln

- **Niemals ohne Kommando gießen.** Kein Knopf, kein Timer — nur `dispense`.
- **Ein Guss läuft offline zu Ende.** Bricht WLAN/MQTT während des Gießens
  ab: fertig gießen, Ventil zu, und die End-Antwort nach dem Reconnect
  senden. Nicht mittendrin stehen bleiben (halbes Glas) und nicht neu anfangen.
- **Harte Obergrenze im Code:** maximale Ventil-offen-Zeit (z. B. 10 s),
  unabhängig von allem anderen. Reißt sie, Ventil zu + `failed`/`POUR_TIMEOUT`.
- **Kein `delay()` beim Gießen** — die MQTT-Bibliothek braucht regelmäßige
  `loop()`-Aufrufe. Gießzeit mit `millis()` überwachen (Muster unten).
- ml → Zeit musst du einmal kalibrieren (Pumpe/Ventil laufen lassen, Menge
  messen, Faktor `ms_pro_ml` im Code).

## Code

Die Referenz-Implementierung liegt hier im Ordner: **`fuel-station.ino`**
(kompiliert für den ESP32-C3: `esp32:esp32:esp32c3:CDCOnBoot=cdc`, gleiche Struktur wie
`Train/Train.ino` inkl. Telnet-Debug-Konsole auf Port 23). Bibliotheken:
„MQTT" (256dpi) und „ArduinoJson" aus dem Library Manager.

Die Hardware ist im Abschnitt **„hardware"** schon verdrahtet (übernommen
aus `fuel-station-old/UltraschallPimpe.ino`): Pumpe über IRLZ34N-MOSFET
(Low-side) an GPIO19, HC-SR04 mit TRIG=4 und ECHO=5. Liefert der Sensor kein
Echo, gilt das als „kein Glas" — gegossen wird nur mit gültiger Messung.
Während des Gießens bricht erst der 3. Fehl-Read in Folge den Guss ab
(`NO_GLASS_ABORT_READS`), weil Ultraschall gelegentlich ein Echo verliert.

Noch zu kalibrieren: `MS_PER_ML` (gießen und messen), `GLASS_PRESENT_MM`
(Abstand Sensor → Glas, aktuell 200 mm wie im alten Sketch) und
`TANK_CAPACITY_ML`.

**Achtung:** Der ECHO-Pin des HC-SR04 liefert 5 V, der ESP32 verträgt nur
3,3 V — ein Spannungsteiler am ECHO-Pin wird empfohlen.

Daneben gehört eine `credentials.h` mit `SECRET_SSID`/`SECRET_PASS`
(einfach die aus `Train/` kopieren, ist absichtlich nicht im Git).

## Testen ohne Server

Auf dem Server-Laptop (Broker läuft dort):

```sh
# was die Tankstelle sendet, live ansehen:
mosquitto_sub -h localhost -u admin -P 123 -t 'shotexpress/fuel/#' -v

# ein Kommando von Hand schicken:
mosquitto_pub -h localhost -u admin -P 123 -t shotexpress/fuel/command -m '{
  "msg_id":"11111111-2222-4333-8444-555555555555",
  "cmd_id":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
  "seq":1,"ts_ms":'"$(date +%s%3N)"',
  "cmd_type":"dispense","volume_ml":40,"ttl_ms":15000}'
```

Erwartet: sofort `accepted`, nach ~`volume_ml × ms_pro_ml` ein `completed`.

Die verbindlichen Nachrichtenformate (mit Tests) liegen in
`protocol/src/lib.rs`, Abschnitt „fuel station".
