#include <WiFi.h>
#include <MQTT.h>
#include <ArduinoJson.h>
#include <time.h>
#include "credentials.h"

// Fuel station: dumb executor for shotexpress/fuel/command (see
// ../fuel-station/README.md and protocol/src/lib.rs, "fuel station").
// One relay drives the pump, one ultrasonic sensor checks for a glass.

constexpr int RELAY_PIN = 26;
constexpr int TRIG_PIN = 25;
constexpr int ECHO_PIN = 33;

constexpr unsigned long WIFI_RETRY_INTERVAL = 5000;
constexpr unsigned long STATUS_PUBLISH_INTERVAL = 1000;
constexpr unsigned long GLASS_CHECK_INTERVAL = 100;

// TODO calibrate: run the pump, measure the poured volume, adjust.
constexpr float MS_PER_ML = 120.0;
// Hard valve-open cap, independent of everything else (stuck-valve net).
constexpr unsigned long MAX_POUR_MS = 10000;
// Sensor-to-glass-rim distance below this means a glass is present.
constexpr long GLASS_PRESENT_MM = 60;
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

WiFiClient net;
MQTTClient mqttClient(1024);

WiFiServer telnetServer(23);
WiFiClient telnetClient;

unsigned long lastWifiRetry = 0;
unsigned long lastStatusTime = 0;
unsigned long lastGlassCheck = 0;

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

// ---- dummy hardware (replace with the real libraries) --------------------
// The rest of the sketch only talks to these functions.

void pumpSetup() {
  pinMode(RELAY_PIN, OUTPUT);
  digitalWrite(RELAY_PIN, LOW); // TODO relay module may be active-low
}

void pumpOn() {
  digitalWrite(RELAY_PIN, HIGH); // TODO real relay drive
}

void pumpOff() {
  digitalWrite(RELAY_PIN, LOW); // TODO real relay drive
}

void glassSensorSetup() {
  pinMode(TRIG_PIN, OUTPUT); // TODO real ultrasonic lib setup
  pinMode(ECHO_PIN, INPUT);
}

long readDistanceMm() {
  // TODO real HC-SR04 read (10 us trigger pulse, pulseIn on echo).
  return 42; // dummy: pretend a glass always sits under the spout
}

bool glassPresent() {
  return readDistanceMm() <= GLASS_PRESENT_MM;
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

void handleConnection() {
  unsigned long currentMillis = millis();

  if (WiFi.status() != WL_CONNECTED) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      Serial.print("Connecting to WiFi...");
      WiFi.begin(ssid, pass);
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
  WiFi.begin(ssid, pass);
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
  }

  // A running pour continues offline; only the reports wait for reconnect.
  if (activeCmdId != "") {
    if (millis() - pourStartedAt > MAX_POUR_MS) {
      finishPour("failed", "POUR_TIMEOUT", "valve open past the hard cap");
      debugPrintln("Error: pour watchdog tripped");
    } else if (millis() - pourStartedAt >= pourDurationMs) {
      finishPour("completed");
      debugPrintln("Status: pour completed");
    } else if (millis() - lastGlassCheck >= GLASS_CHECK_INTERVAL) {
      lastGlassCheck = millis();
      if (!glassPresent()) {
        finishPour("failed", "NO_GLASS", "glass removed mid-pour");
        debugPrintln("Error: glass removed mid-pour");
      }
    }
  }
}
