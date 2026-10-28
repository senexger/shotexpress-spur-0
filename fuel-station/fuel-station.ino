#include <WiFi.h>
#include <MQTT.h>
#include <ArduinoJson.h>
#include <time.h>
#include "credentials.h"

// Fuel station: dumb executor for shotexpress/fuel/command (see
// ../fuel-station/README.md and protocol/src/lib.rs, "fuel station").
// Hardware as in ../fuel-station-old/UltraschallPimpe.ino: the pump hangs on
// an IRLZ34N low-side MOSFET, an HC-SR04 ultrasonic sensor checks for a glass.

constexpr int PUMP_PIN = 19; // MOSFET gate, HIGH = pump on
constexpr int TRIG_PIN = 4;
constexpr int ECHO_PIN = 5;

constexpr unsigned long WIFI_RETRY_INTERVAL = 5000;
constexpr unsigned long WIFI_RESTART_INTERVAL = 20000;
// ESP32-C3 boards with a small onboard antenna often fail to join at full
// power (disconnect reason 2, AUTH_EXPIRE). Lower TX power fixes it; must be
// set after every WiFi.begin().
#define WIFI_TX_POWER_FIX() WiFi.setTxPower(WIFI_POWER_8_5dBm)
constexpr unsigned long STATUS_PUBLISH_INTERVAL = 1000;
constexpr unsigned long SENSOR_READ_INTERVAL = 100;
// raw distance for the web UI (watch a glass fill, calibrate thresholds)
constexpr unsigned long SENSOR_PUBLISH_INTERVAL = 250;

// TODO calibrate: run the pump, measure the poured volume, adjust.
constexpr float MS_PER_ML = 120.0;
// Hard valve-open cap, independent of everything else (stuck-valve net).
constexpr unsigned long MAX_POUR_MS = 10000;
// TODO calibrate: sensor-to-glass distance below this means a glass is
// present (20 cm is the threshold the old sketch switched the pump at).
constexpr long GLASS_PRESENT_MM = 200;
// Ultrasonic sensors miss an echo now and then; only abort a running pour
// after this many no-glass readings in a row (~300 ms at 100 ms checks).
constexpr int NO_GLASS_ABORT_READS = 3;
// Reservoir estimate: one bottle; refill and reboot to reset the counter.
constexpr unsigned long TANK_CAPACITY_ML = 700;

constexpr const char* ssid = SECRET_SSID;
constexpr const char* pass = SECRET_PASS;
constexpr const char* mqttClientID = "FuelStation";
constexpr const char* mqttClientUsername = "admin";
constexpr const char* mqttClientPassword = "123";
constexpr const char* mqttHost = "192.168.16.127"; // same broker as the train
constexpr const char* mqttSubTopic = "shotexpress/fuel/command";
constexpr const char* mqttEventTopic = "shotexpress/fuel/event";
constexpr const char* mqttStatusTopic = "shotexpress/fuel/status";
constexpr const char* mqttSensorTopic = "shotexpress/fuel/sensor"; // plain mm, -1 = no echo

WiFiClient net;
MQTTClient mqttClient(1024);

WiFiServer telnetServer(23);
WiFiClient telnetClient;

unsigned long lastWifiRetry = 0;
unsigned long lastStatusTime = 0;
unsigned long lastSensorRead = 0;
unsigned long lastSensorPublish = 0;
long distanceMm = -1; // latest ultrasonic reading, refreshed every SENSOR_READ_INTERVAL

// ---- envelope bookkeeping ----------------------------------------------

unsigned long seq = 0;
String seenMsgIds[8]; // QoS 1 may deliver duplicates; remember the last few
int seenMsgIdNext = 0;

// Terminal events must survive a wifi gap (a pour finishes offline);
// they are queued here and flushed after the next successful connect.
String queuedEvents[4];
int queuedEventCount = 0;

// ---- dispense state machine ----------------------------------------------
// IDLE: activeCmdId == "". DISPENSING: pump on until pourDurationMs elapsed.

String activeCmdId = "";
unsigned long pourStartedAt = 0;
unsigned long pourDurationMs = 0;
unsigned long dispensedMl = 0;
int noGlassReads = 0;

// ---- hardware --------------------------------------------------------------
// The rest of the sketch only talks to these functions.

void pumpSetup() {
  pinMode(PUMP_PIN, OUTPUT);
  digitalWrite(PUMP_PIN, LOW);
}

void pumpOn() {
  digitalWrite(PUMP_PIN, HIGH);
}

void pumpOff() {
  digitalWrite(PUMP_PIN, LOW);
}

void glassSensorSetup() {
  pinMode(TRIG_PIN, OUTPUT);
  pinMode(ECHO_PIN, INPUT);
  digitalWrite(TRIG_PIN, LOW);
}

// HC-SR04 distance in mm, -1 when no echo came back (blocks up to 30 ms).
long readDistanceMm() {
  digitalWrite(TRIG_PIN, LOW);
  delayMicroseconds(2);
  digitalWrite(TRIG_PIN, HIGH);
  delayMicroseconds(10);
  digitalWrite(TRIG_PIN, LOW);

  unsigned long durationUs = pulseIn(ECHO_PIN, HIGH, 30000);
  if (durationUs == 0) return -1;
  return (long)(durationUs * 0.343 / 2.0); // speed of sound: 0.343 mm/us
}

// Uses the latest reading (at most SENSOR_READ_INTERVAL old).
// No echo counts as "no glass" — never pour on a failed measurement.
bool glassPresent() {
  return distanceMm >= 0 && distanceMm <= GLASS_PRESENT_MM;
}

// ---------------------------------------------------------------------------

void debugPrint(String msg) {
  Serial.print(msg);
  if (telnetClient && telnetClient.connected()) {
    telnetClient.print(msg);
  }
}

void debugPrintln(String msg) {
  Serial.println(msg);
  if (telnetClient && telnetClient.connected()) {
    telnetClient.println(msg);
  }
}

// ---- protocol helpers ------------------------------------------------------

// Unix millis via NTP; 0 until the clock has synced (then ttl checks are
// skipped and events carry ts_ms 0 — the server tolerates both).
uint64_t nowMs() {
  time_t now = time(nullptr);
  if (now < 1000000000) return 0;
  return (uint64_t)now * 1000ULL;
}

String newMsgId() {
  char buf[37];
  snprintf(buf, sizeof(buf), "%08lx-%04x-4%03x-8%03x-%04x%08lx",
           (unsigned long)esp_random(), (unsigned)(esp_random() & 0xffff),
           (unsigned)(esp_random() & 0xfff), (unsigned)(esp_random() & 0xfff),
           (unsigned)(esp_random() & 0xffff), (unsigned long)esp_random());
  return String(buf);
}

bool isDuplicate(const String& msgId) {
  for (const String& seen : seenMsgIds) {
    if (msgId.length() > 0 && seen == msgId) return true;
  }
  seenMsgIds[seenMsgIdNext] = msgId;
  seenMsgIdNext = (seenMsgIdNext + 1) % 8;
  return false;
}

int tankPct() {
  if (dispensedMl >= TANK_CAPACITY_ML) return 0;
  return (int)(100 - (dispensedMl * 100) / TANK_CAPACITY_ML);
}

void flushQueuedEvents() {
  for (int i = 0; i < queuedEventCount; i++) {
    mqttClient.publish(mqttEventTopic, queuedEvents[i], false, 1);
    debugPrintln("Status: flushed queued event " + queuedEvents[i]);
  }
  queuedEventCount = 0;
}

void sendEvent(const String& cmdId, const char* execType,
               const char* errCode = nullptr, const char* errReason = nullptr) {
  JsonDocument doc;
  doc["msg_id"] = newMsgId();
  doc["cmd_id"] = cmdId;
  doc["seq"] = ++seq;
  doc["ts_ms"] = nowMs();
  doc["exec_type"] = execType;
  if (errCode) {
    doc["error"]["code"] = errCode;
    doc["error"]["reason"] = errReason;
  }
  String payload;
  serializeJson(doc, payload);

  if (mqttClient.connected() && mqttClient.publish(mqttEventTopic, payload, false, 1)) {
    return;
  }
  // offline: keep the report for after the reconnect (drop oldest if full)
  if (queuedEventCount == 4) {
    for (int i = 1; i < 4; i++) queuedEvents[i - 1] = queuedEvents[i];
    queuedEventCount = 3;
  }
  queuedEvents[queuedEventCount++] = payload;
  debugPrintln("Status: offline, queued event " + String(execType));
}

void publishStatus() {
  JsonDocument doc;
  doc["msg_id"] = newMsgId();
  doc["cmd_id"] = activeCmdId;
  doc["seq"] = ++seq;
  doc["ts_ms"] = nowMs();
  doc["state"] = activeCmdId != "" ? "DISPENSING" : (tankPct() == 0 ? "ERROR" : "IDLE");
  doc["tank_pct"] = tankPct();
  String payload;
  serializeJson(doc, payload);
  mqttClient.publish(mqttStatusTopic, payload);
}

// ---- command handling --------------------------------------------------------

void finishPour(const char* execType, const char* errCode = nullptr,
                const char* errReason = nullptr) {
  pumpOff();
  sendEvent(activeCmdId, execType, errCode, errReason);
  activeCmdId = "";
}

void messageReceived(String &topic, String &payload) {
  debugPrintln("Incoming: " + topic + " - " + payload);

  JsonDocument doc;
  if (deserializeJson(doc, payload)) {
    debugPrintln("Error: payload is not JSON, ignoring");
    return;
  }
  String msgId = doc["msg_id"] | "";
  String cmdId = doc["cmd_id"] | "";
  String cmdType = doc["cmd_type"] | "";
  if (isDuplicate(msgId)) {
    debugPrintln("Status: duplicate msg_id, ignoring");
    return;
  }

  if (cmdType == "stop") {
    if (activeCmdId != "") {
      finishPour("cancelled");
      debugPrintln("Status: pour cancelled");
    }
    return;
  }

  if (cmdType != "dispense") {
    debugPrintln("Error: unknown cmd_type " + cmdType);
    return;
  }

  uint64_t tsMs = doc["ts_ms"] | (uint64_t)0;
  uint64_t ttlMs = doc["ttl_ms"] | (uint64_t)0;
  uint32_t volumeMl = doc["volume_ml"] | (uint32_t)0;

  if (nowMs() > 0 && ttlMs > 0 && nowMs() > tsMs + ttlMs) {
    sendEvent(cmdId, "expired");
    return;
  }
  if (activeCmdId != "") {
    // cancel-and-replace, like the train
    finishPour("cancelled");
  }

  unsigned long pourMs = (unsigned long)(volumeMl * MS_PER_ML);
  if (volumeMl == 0 || pourMs > MAX_POUR_MS) {
    sendEvent(cmdId, "failed", "POUR_TIMEOUT",
              "volume_ml outside the pourable range");
    return;
  }
  if (tankPct() == 0) {
    sendEvent(cmdId, "failed", "TANK_EMPTY",
              "reservoir estimate at zero, refill and reboot");
    return;
  }
  if (!glassPresent()) {
    sendEvent(cmdId, "failed", "NO_GLASS", "no glass under the spout");
    return;
  }

  sendEvent(cmdId, "accepted");
  activeCmdId = cmdId;
  pourStartedAt = millis();
  pourDurationMs = pourMs;
  noGlassReads = 0;
  dispensedMl += volumeMl;
  pumpOn();
  debugPrintln("Status: pouring " + String(volumeMl) + " ml (" + String(pourMs) + " ms)");
}

// ---- connectivity (same pattern as Train.ino) ---------------------------------

void handleTelnet() {
  if (telnetServer.hasClient()) {
    if (!telnetClient || !telnetClient.connected()) {
      if (telnetClient) telnetClient.stop();
      telnetClient = telnetServer.available();
      telnetClient.println("Connected to FuelStation Debug Console");
      debugPrintln("New Telnet Client Connected");
    } else {
      WiFiClient serverClient = telnetServer.available();
      serverClient.stop();
      debugPrintln("Rejected extra Telnet connection");
    }
  }
}

const char* wifiStatusName(wl_status_t status) {
  switch (status) {
    case WL_NO_SSID_AVAIL: return "NO_SSID_AVAIL (network not found)";
    case WL_CONNECT_FAILED: return "CONNECT_FAILED (wrong password?)";
    case WL_CONNECTION_LOST: return "CONNECTION_LOST";
    case WL_DISCONNECTED: return "DISCONNECTED";
    case WL_IDLE_STATUS: return "IDLE";
    default: return "OTHER";
  }
}

void handleConnection() {
  unsigned long currentMillis = millis();

  if (WiFi.status() != WL_CONNECTED) {
    // the ESP keeps trying on its own (setAutoReconnect); only restart the
    // attempt after a long quiet spell, never while it is still connecting
    if (currentMillis - lastWifiRetry > WIFI_RESTART_INTERVAL) {
      Serial.println("WiFi not connected, status " + String(wifiStatusName(WiFi.status())) +
                     " — restarting connect to " + String(ssid));
      WiFi.disconnect();
      WiFi.begin(ssid, pass);
      WIFI_TX_POWER_FIX();
      lastWifiRetry = currentMillis;
    }
    return;
  }

  handleTelnet();

  if (!mqttClient.connected()) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      debugPrintln("Connecting MQTT...");

      if (mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
        debugPrintln("MQTT Connected!");
        // subscriptions do not survive a reconnect
        mqttClient.subscribe(mqttSubTopic, 1);
        flushQueuedEvents();
      } else {
        debugPrint(".");
      }
      lastWifiRetry = currentMillis;
    }
  }
}

void setup() {
  Serial.begin(115200);
  Serial.println("Booting...");

  pumpSetup();
  glassSensorSetup();

  WiFi.mode(WIFI_STA);
  WiFi.setAutoReconnect(true);
  WiFi.onEvent([](WiFiEvent_t event, WiFiEventInfo_t info) {
    if (event == ARDUINO_EVENT_WIFI_STA_GOT_IP) {
      Serial.println("WiFi connected, IP " + WiFi.localIP().toString() +
                     ", signal " + String(WiFi.RSSI()) + " dBm");
    } else if (event == ARDUINO_EVENT_WIFI_STA_DISCONNECTED) {
      // reason 201 = network not found, 15/202/204 = wrong password, 2 = auth timeout (signal/TX power)
      Serial.println("WiFi disconnected, reason " + String(info.wifi_sta_disconnected.reason));
    }
  });
  Serial.println("Connecting to WiFi " + String(ssid) + "...");
  WiFi.begin(ssid, pass);
  WIFI_TX_POWER_FIX();
  lastWifiRetry = millis();
  configTime(0, 0, "pool.ntp.org"); // ts_ms / ttl need the real clock

  telnetServer.begin();
  telnetServer.setNoDelay(true);

  mqttClient.begin(mqttHost, net);
  mqttClient.onMessage(messageReceived);
}

void loop() {
  handleConnection();

  if (mqttClient.connected()) {
    mqttClient.loop();

    if (millis() - lastStatusTime > STATUS_PUBLISH_INTERVAL) {
      publishStatus();
      lastStatusTime = millis();
    }
    if (millis() - lastSensorPublish >= SENSOR_PUBLISH_INTERVAL) {
      mqttClient.publish(mqttSensorTopic, String(distanceMm));
      lastSensorPublish = millis();
    }
  }

  bool freshReading = false;
  if (millis() - lastSensorRead >= SENSOR_READ_INTERVAL) {
    lastSensorRead = millis();
    distanceMm = readDistanceMm();
    freshReading = true;
  }

  // A running pour continues offline; only the reports wait for reconnect.
  if (activeCmdId != "") {
    if (millis() - pourStartedAt > MAX_POUR_MS) {
      finishPour("failed", "POUR_TIMEOUT", "valve open past the hard cap");
      debugPrintln("Error: pour watchdog tripped");
    } else if (millis() - pourStartedAt >= pourDurationMs) {
      finishPour("completed");
      debugPrintln("Status: pour completed");
    } else if (freshReading) {
      noGlassReads = glassPresent() ? 0 : noGlassReads + 1;
      if (noGlassReads >= NO_GLASS_ABORT_READS) {
        finishPour("failed", "NO_GLASS", "glass removed mid-pour");
        debugPrintln("Error: glass removed mid-pour");
      }
    }
  }
}
