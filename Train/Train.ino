#include <SPI.h>
#include <WiFi.h>
#include <MQTT.h>
#include <FastAccelStepper.h>
#include <MFRC522.h>
#include <ESPmDNS.h>
#include <ArduinoJson.h>
#include "credentials.h"
// #include <ArduinoOTA.h>

constexpr int STEP_PIN = 18;
constexpr int DIR_PIN  = 19;
constexpr int EN_PIN   = 5;
constexpr int RST_PIN  = 22;
constexpr int SS_PIN   = 15;
constexpr int SCK_PIN  = 14;
constexpr int MISO_PIN = 13;
constexpr int MOSI_PIN = 12;

constexpr unsigned long WIFI_RETRY_INTERVAL = 5000;
// WiFi lost this long: give up on this network and try the other one
constexpr unsigned long WIFI_SWITCH_INTERVAL = 10000;
constexpr unsigned long MQTT_PUBLISH_INTERVAL = 1000;

// Speed comes from the web UI (retained on shotexpress/speed, steps/s).
// DEFAULT_SPEED_HZ only applies until the retained value arrives.
constexpr unsigned long DEFAULT_SPEED_HZ = 3000;
constexpr unsigned long MAX_SPEED_HZ = 20000;
constexpr unsigned long ACCELERATION = 500; // start ramp only; stop is instant
constexpr unsigned long SHOT_STEP = 1000;

// Rides (JSON `drive` on shotexpress/command) use normalized speeds 0..1.
// FULL_SPEED_HZ is what 1.0 means on the motor; normalized accel (speed
// units per second) scales the same way. Calibrate on the real track.
constexpr unsigned long FULL_SPEED_HZ = MAX_SPEED_HZ;
constexpr int MAX_TAGS = 64;
constexpr int MAX_TAG_LEN = 24;
constexpr int MAX_STATION_LEN = 32;
constexpr int MQTT_BUFFER_SIZE = 4096; // the tag table is ~50 bytes per tag

// Known networks (credentials.h). The train moves between access points:
// at boot it joins the stronger one, and when WiFi stays lost it switches
// to the other.
struct WifiNetwork {
  const char* ssid;
  const char* pass;
};
const WifiNetwork networks[] = {
  {SECRET_SSID, SECRET_PASS},
#ifdef SECRET_SSID2
  {SECRET_SSID2, SECRET_PASS2},
#endif
};
constexpr int NETWORK_COUNT = sizeof(networks) / sizeof(networks[0]);
int currentNetwork = 0;
constexpr const char* mqttClientID = "StupidStepper"; // TODO rename?
constexpr const char* mqttClientUsername = "admin";
constexpr const char* mqttClientPassword = "123";
// The broker is found via mDNS (the server advertises _mqtt._tcp and
// shotexpress.local); this IP is only the last resort if nothing answers.
constexpr const char* mqttHost = "192.168.16.127";
constexpr int mqttPort = 1883;
constexpr const char* mqttMdnsHost = "shotexpress"; // shotexpress.local
constexpr const char* trainMdnsName = "shotexpress-train";
constexpr const char* mqttCommandTopic = "shotexpress/command";
constexpr const char* mqttSpeedTopic = "shotexpress/speed";
constexpr const char* mqttStepsTopic = "shotexpress/steps"; // signed relative move
constexpr const char* mqttAccelTopic = "shotexpress/accel"; // retained, steps/s^2
constexpr const char* mqttTrackTopic = "shotexpress/track"; // retained tag table (JSON)

// Manual mode only (no ride active): the train halts the moment it reads
// this tag. It then ignores it until any other tag was read, so driving off
// the tag doesn't stop it again. During a ride the route decides.
constexpr const char* STOP_TAG = "93E0CD1C";

MFRC522 mfrc522(SS_PIN, RST_PIN);
bool stopArmed = true;

WiFiClient net;
MQTTClient mqttClient(MQTT_BUFFER_SIZE);

WiFiServer telnetServer(23);
WiFiClient telnetClient;

FastAccelStepperEngine engine;
FastAccelStepper *stepper = NULL;

unsigned long lastWifiRetry = 0;
bool wifiUp = false; // the 10 s before switching count from losing WiFi
bool mdnsStarted = false;
unsigned long lastPublishTime = 0;
unsigned long speedHz = DEFAULT_SPEED_HZ;
unsigned long accel = ACCELERATION; // start ramp; set from the web UI

// ---- tag table (balises) + on-board ride ("drive") ----
// The server publishes what each tag means (retained, shotexpress/track):
// a speed tag sets the speed from there on, a station tag is a full stop
// when it is the ride's target. Tags are read from one side only, so no
// order or direction is needed: every read is looked up on its own, and
// speed changes and the stop happen without any network round trip.
struct TagRule {
  char uid[MAX_TAG_LEN];
  bool isStation;
  float speed;                   // speed tags: normalized 0..1
  char station[MAX_STATION_LEN]; // station tags: name
};

TagRule tagTable[MAX_TAGS];
int tagCount = 0;
uint32_t trackVersion = 0;
bool haveTrack = false;

float zoneSpeed = -1;      // last speed tag read (also outside rides); -1 = unknown
bool driveActive = false;
String driveCmdId = "";
char driveTarget[MAX_STATION_LEN] = "";
float driveMaxSpeed = 1.0;
unsigned long driveAccel = ACCELERATION;
unsigned long driveWatchdogMs = 0;
unsigned long lastTagMs = 0;
unsigned long eventSeq = 0;

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
// ----------------------------------------------

void applySpeed(long hz) {
  if (hz <= 0) {
    debugPrintln("Error: invalid speed " + String(hz));
    return;
  }
  speedHz = min((unsigned long)hz, MAX_SPEED_HZ);
  stepper->setSpeedInHz(speedHz);
  if (stepper->isRunning()) {
    stepper->applySpeedAcceleration(); // change speed while driving
  }
  debugPrintln("Status: Speed " + String(speedHz) + " Hz");
}

void applyAccel(long value) {
  if (value <= 0) {
    debugPrintln("Error: invalid acceleration " + String(value));
    return;
  }
  accel = value;
  stepper->setAcceleration(accel);
  if (stepper->isRunning()) {
    stepper->applySpeedAcceleration();
  }
  debugPrintln("Status: Acceleration " + String(accel) + " steps/s^2");
}

// Raw commands on shotexpress/command, every one is executed:
// 0 = stop (instant), 1 = move SHOT_STEP, 2 = run backward, 3 = run forward
void runCommand(int command) {
  switch (command) {
    case 3:
      stepper->runForward();
      debugPrintln("Status: Run Forward");
      break;
    case 2:
      stepper->runBackward();
      debugPrintln("Status: Run Backward");
      break;
    case 1:
      stepper->move(SHOT_STEP);
      debugPrintln("Status: Move " + String(SHOT_STEP) + " Steps");
      break;
    case 0:
      stepper->forceStop();
      debugPrintln("Status: Stop");
      break;
    default:
      debugPrintln("Error: Invalid command");
      break;
  }
}

void messageReceived(String &topic, String &payload) {
  debugPrintln("Incoming: " + topic + " - " + payload);

  // retained speed/accel are the manual settings: during a ride they are
  // only remembered, the ride's own speeds stay in charge
  if (topic == mqttTrackTopic) {
    loadTrack(payload);
  } else if (topic == mqttSpeedTopic) {
    if (driveActive) {
      speedHz = min((unsigned long)max(payload.toInt(), 1L), MAX_SPEED_HZ);
    } else {
      applySpeed(payload.toInt());
    }
  } else if (topic == mqttAccelTopic) {
    if (driveActive) {
      accel = max(payload.toInt(), 1L);
    } else {
      applyAccel(payload.toInt());
    }
  } else if (topic == mqttStepsTopic) {
    long steps = payload.toInt(); // negative = backward
    if (steps != 0) {
      cancelDrive("manual move");
      stepper->move(steps);
      debugPrintln("Status: Move " + String(steps) + " Steps");
    }
  } else if (payload.startsWith("{")) {
    handleJsonCommand(payload);
  } else {
    cancelDrive("manual command");
    runCommand(payload.toInt());
  }
}

// ---- exec events (shotexpress/event/exec), same envelope as the server ----

String randomUuid() {
  uint8_t b[16];
  for (int i = 0; i < 16; i += 4) {
    uint32_t r = esp_random();
    memcpy(b + i, &r, 4);
  }
  b[6] = (b[6] & 0x0F) | 0x40; // version 4
  b[8] = (b[8] & 0x3F) | 0x80; // RFC 4122 variant
  char out[37];
  snprintf(out, sizeof(out),
           "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
           b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
           b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
  return String(out);
}

// `fill` adds the event-specific fields (exec_type etc.)
template <typename F>
void publishExec(const String& cmdId, F fill) {
  JsonDocument doc;
  doc["msg_id"] = randomUuid();
  doc["cmd_id"] = cmdId;
  doc["seq"] = ++eventSeq;
  doc["ts_ms"] = millis(); // no wall clock on board
  fill(doc);
  String out;
  serializeJson(doc, out);
  debugPrintln("Exec: " + out);
  if (mqttClient.connected()) {
    mqttClient.publish("shotexpress/event/exec", out);
  }
}

void publishSimple(const String& cmdId, const char* execType) {
  publishExec(cmdId, [&](JsonDocument& doc) { doc["exec_type"] = execType; });
}

void publishFailed(const String& cmdId, const char* code, const String& reason) {
  publishExec(cmdId, [&](JsonDocument& doc) {
    doc["exec_type"] = "failed";
    doc["error"]["code"] = code;
    doc["error"]["reason"] = reason;
  });
}

unsigned long toHz(float speed) {
  speed = constrain(speed, 0.0f, 1.0f);
  return max(min((unsigned long)(speed * FULL_SPEED_HZ), MAX_SPEED_HZ), 1UL);
}

void driveSpeed(float speed) {
  stepper->setSpeedInHz(toHz(min(speed, driveMaxSpeed)));
  if (stepper->isRunning()) {
    stepper->applySpeedAcceleration();
  }
}

const TagRule* findRule(const String& uid) {
  for (int i = 0; i < tagCount; i++) {
    if (uid == tagTable[i].uid) return &tagTable[i];
  }
  return nullptr;
}

const TagRule* findStation(const char* name) {
  for (int i = 0; i < tagCount; i++) {
    if (tagTable[i].isStation && strcmp(tagTable[i].station, name) == 0) return &tagTable[i];
  }
  return nullptr;
}

// Retained tag table from the server. A new table during a ride is taken
// as-is; the ride keeps going with the new speeds.
void loadTrack(const String& payload) {
  JsonDocument doc;
  DeserializationError err = deserializeJson(doc, payload);
  if (err) {
    debugPrintln(String("Error: bad tag table: ") + err.c_str());
    return;
  }
  JsonArray tags = doc["tags"].as<JsonArray>();
  if ((int)tags.size() > MAX_TAGS) {
    debugPrintln("Error: tag table has " + String(tags.size()) + " tags, the train holds " + String(MAX_TAGS));
    haveTrack = false;
    return;
  }
  tagCount = 0;
  for (JsonObject t : tags) {
    TagRule& rule = tagTable[tagCount++];
    strlcpy(rule.uid, t["uid"] | "", MAX_TAG_LEN);
    rule.isStation = String(t["kind"] | "") == "station";
    rule.speed = t["speed"] | 0.0f;
    strlcpy(rule.station, t["station"] | "", MAX_STATION_LEN);
  }
  trackVersion = doc["version"] | 0UL;
  haveTrack = true;
  debugPrintln("Status: Tag table v" + String(trackVersion) + ", " + String(tagCount) + " tags");
}

// End the ride. Arrival decelerates gently (shots stay in the glasses);
// faults stop at once.
void endDrive(bool gentle) {
  if (gentle) {
    stepper->stopMove();
  } else {
    stepper->forceStop();
  }
  driveActive = false;
  // back to the manual settings for the raw buttons
  stepper->setSpeedInHz(speedHz);
  stepper->setAcceleration(accel);
}

// Any manual command takes over: an active ride is cancelled first.
void cancelDrive(const char* why) {
  if (!driveActive) return;
  endDrive(false);
  debugPrintln(String("Status: Ride cancelled (") + why + ")");
  publishSimple(driveCmdId, "cancelled");
}

void startDrive(JsonDocument& doc) {
  String cmdId = doc["cmd_id"] | "";
  const char* target = doc["target"] | "";
  uint32_t version = doc["track_version"] | 0UL;

  if (!haveTrack || version != trackVersion) {
    publishFailed(cmdId, "TRACK_MISMATCH",
                  "ride planned with tag table v" + String(version) + ", train has " +
                  (haveTrack ? "v" + String(trackVersion) : String("none")));
    return;
  }
  if (!findStation(target)) {
    publishFailed(cmdId, "UNKNOWN_STATION", String("no station ") + target + " in the tag table");
    return;
  }

  cancelDrive("new ride");
  driveCmdId = cmdId;
  strlcpy(driveTarget, target, MAX_STATION_LEN);
  driveMaxSpeed = doc["max_speed"] | 1.0f;
  float startSpeed = doc["start_speed"] | 0.3f;
  driveAccel = toHz(doc["accel"] | 0.5f);
  driveWatchdogMs = doc["max_run_ms_without_tag"] | 30000UL;
  lastTagMs = millis();
  driveActive = true;

  stepper->setAcceleration(driveAccel);
  driveSpeed(zoneSpeed > 0 ? zoneSpeed : startSpeed);
  stepper->runForward();
  debugPrintln(String("Status: Ride to ") + driveTarget);
  publishSimple(driveCmdId, "accepted");
}

// Every read, ride or not: remember the speed zone. During a ride a speed
// tag changes the speed at once and the target's station tag stops.
void driveOnTag(const String& uid) {
  const TagRule* rule = findRule(uid);
  if (rule && !rule->isStation) {
    zoneSpeed = rule->speed;
  }
  if (!driveActive) return;

  lastTagMs = millis();
  if (rule && rule->isStation && strcmp(rule->station, driveTarget) == 0) {
    endDrive(true);
    debugPrintln(String("Status: Arrived at ") + driveTarget);
    publishSimple(driveCmdId, "completed");
    return;
  }
  if (rule && !rule->isStation) {
    driveSpeed(rule->speed);
  }
  // other stations and unlisted tags: keep going
  publishExec(driveCmdId, [&](JsonDocument& doc) {
    doc["exec_type"] = "progress";
    doc["last_tag"] = uid;
    doc["distance_m"] = 0.0; // no odometry on board yet
  });
}

void checkDriveWatchdog() {
  if (driveActive && millis() - lastTagMs > driveWatchdogMs) {
    String cmdId = driveCmdId;
    endDrive(false);
    debugPrintln("Status: RFID watchdog, safe stop");
    publishFailed(cmdId, "RFID_TIMEOUT",
                  "no tag for " + String(driveWatchdogMs) + " ms");
  }
}

// JSON on shotexpress/command: the server's protocol (drive / stop / steps)
void handleJsonCommand(const String& payload) {
  JsonDocument doc;
  DeserializationError err = deserializeJson(doc, payload);
  if (err) {
    debugPrintln(String("Error: bad JSON command: ") + err.c_str());
    return;
  }
  String type = doc["cmd_type"] | "";
  String cmdId = doc["cmd_id"] | "";

  if (type == "drive") {
    startDrive(doc);
  } else if (type == "stop") {
    cancelDrive("stop command");
    stepper->forceStop();
    publishSimple(cmdId, "accepted");
    publishSimple(cmdId, "completed");
  } else if (type == "steps") {
    cancelDrive("steps command");
    long steps = doc["steps"] | 0L;
    if (String(doc["direction"] | "forward") == "reverse") steps = -steps;
    stepper->setSpeedInHz(toHz(doc["speed"] | 0.2f));
    stepper->move(steps);
    publishSimple(cmdId, "accepted");
    // completion when the move ends is not tracked yet
  } else {
    debugPrintln("Error: unknown cmd_type " + type);
  }
}

void handleTag(const String& uid) {
  debugPrintln("Card ID : " + uid);

  bool riding = driveActive;
  driveOnTag(uid);
  if (riding) {
    // the ride decides (also when this read just ended it)
  } else if (uid == STOP_TAG) {
    if (stopArmed) {
      stepper->forceStop();
      stopArmed = false;
      debugPrintln("Status: Stop tag, stopped");
    }
  } else {
    stopArmed = true;
  }

  if (mqttClient.connected()) {
    // raw uid, no JSON — the server shows it as "last tag" (setup aid)
    mqttClient.publish("shotexpress/event/tag", uid);
  }
}

// void setupOTA() {
//   ArduinoOTA.setHostname(mqttClientID); // Hostname for port scanning

//   ArduinoOTA.onStart([]() {
//     String type;
//     if (ArduinoOTA.getCommand() == U_FLASH)
//       type = "sketch";
//     else // U_SPIFFS
//       type = "filesystem";

//     if (stepper) {
//       stepper->forceStop();
//     }
//     debugPrintln("Start updating " + type);
//   });

//   ArduinoOTA.onEnd([]() {
//     debugPrintln("\nEnd");
//   });

//   ArduinoOTA.onProgress([](unsigned int progress, unsigned int total) {
//     // Avoid spamming telnet during progress, usually too fast
//     // Serial.printf("Progress: %u%%\r", (progress / (total / 100)));
//   });

//   ArduinoOTA.onError([](ota_error_t error) {
//     debugPrint("Error: ");
//     if (error == OTA_AUTH_ERROR) debugPrintln("Auth Failed");
//     else if (error == OTA_BEGIN_ERROR) debugPrintln("Begin Failed");
//     else if (error == OTA_CONNECT_ERROR) debugPrintln("Connect Failed");
//     else if (error == OTA_RECEIVE_ERROR) debugPrintln("Receive Failed");
//     else if (error == OTA_END_ERROR) debugPrintln("End Failed");
//   });

//   ArduinoOTA.begin();
// }

void handleTelnet() {
  // Check for new clients
  if (telnetServer.hasClient()) {
    if (!telnetClient || !telnetClient.connected()) {
      if (telnetClient) telnetClient.stop(); // Clean up old
      telnetClient = telnetServer.available(); // Accept new
      telnetClient.println("Connected to StupidStepper Debug Console");
      debugPrintln("New Telnet Client Connected");
    } else {
      // Reject if already connected
      WiFiClient serverClient = telnetServer.available();
      serverClient.stop();
      debugPrintln("Rejected extra Telnet connection");
    }
  }
}

int knownNetwork(const String& ssid) {
  for (int n = 0; n < NETWORK_COUNT; n++) {
    if (ssid == networks[n].ssid) return n;
  }
  return -1;
}

// Boot-time scan: shows which of our networks the train sees on 2.4 GHz
// and how strong (above -70 dBm is good, below -80 dBm will not connect).
// Returns the strongest known network (the first one if none is visible).
int scanForBestNetwork() {
  int count = WiFi.scanNetworks();
  Serial.println("WiFi scan: " + String(count) + " networks");
  int best = -1;
  int bestRssi = -1000;
  for (int i = 0; i < count; i++) {
    int n = knownNetwork(WiFi.SSID(i));
    if (n >= 0 && WiFi.RSSI(i) > bestRssi) {
      best = n;
      bestRssi = WiFi.RSSI(i);
    }
    Serial.println(String(n >= 0 ? "  * " : "    ") + WiFi.SSID(i) + "  " + String(WiFi.RSSI(i)) +
                   " dBm  ch " + String(WiFi.channel(i)));
  }
  WiFi.scanDelete();
  if (best < 0) {
    Serial.println("  none of our networks visible on 2.4 GHz from here");
    return 0;
  }
  return best;
}

void connectWifi(int n) {
  currentNetwork = n;
  Serial.println("Connecting to WiFi " + String(networks[n].ssid) + "...");
  WiFi.begin(networks[n].ssid, networks[n].pass);
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

// Point the MQTT client at the broker: _mqtt._tcp service first (gives the
// port too), then shotexpress.local, else keep the previous/fallback address.
// Blocks a few seconds, but only runs while MQTT is disconnected.
void resolveBroker() {
  if (!mdnsStarted) {
    mdnsStarted = MDNS.begin(trainMdnsName);
    if (!mdnsStarted) {
      debugPrintln("mDNS start failed, using fallback broker " + String(mqttHost));
      return;
    }
  }
  int found = MDNS.queryService("mqtt", "tcp");
  for (int i = 0; i < found; i++) {
    IPAddress ip = MDNS.address(i);
    if (ip != IPAddress(0, 0, 0, 0)) {
      debugPrintln("Broker via mDNS service: " + ip.toString() + ":" + String(MDNS.port(i)));
      mqttClient.setHost(ip, MDNS.port(i));
      return;
    }
  }
  IPAddress ip = MDNS.queryHost(mqttMdnsHost);
  if (ip != IPAddress(0, 0, 0, 0)) {
    debugPrintln("Broker via " + String(mqttMdnsHost) + ".local: " + ip.toString());
    mqttClient.setHost(ip, mqttPort);
    return;
  }
  debugPrintln("Broker not found via mDNS, trying last known address");
}

void handleConnection() {
  unsigned long currentMillis = millis();

  if (WiFi.status() != WL_CONNECTED) {
    if (wifiUp) {
      wifiUp = false;
      lastWifiRetry = currentMillis;
    }
    // the ESP retries the same network on its own (setAutoReconnect); after
    // a quiet spell switch to the other one. Non-blocking: tags keep being
    // read while the train looks for WiFi.
    if (currentMillis - lastWifiRetry > WIFI_SWITCH_INTERVAL) {
      int next = (currentNetwork + 1) % NETWORK_COUNT;
      Serial.println("WiFi " + String(networks[currentNetwork].ssid) + " not connected, status " +
                     String(wifiStatusName(WiFi.status())) + " — trying " + String(networks[next].ssid));
      WiFi.disconnect();
      connectWifi(next);
      lastWifiRetry = currentMillis;
    }
    return;
  }
  wifiUp = true;

  // ArduinoOTA.handle();

  handleTelnet();

  if (!mqttClient.connected()) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      resolveBroker();
      debugPrintln("Connecting MQTT...");

      if (mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
        debugPrintln("MQTT Connected!");
        // subscriptions do not survive a reconnect
        mqttClient.subscribe(mqttCommandTopic);
        mqttClient.subscribe(mqttSpeedTopic); // retained: last speed arrives at once
        mqttClient.subscribe(mqttStepsTopic);
        mqttClient.subscribe(mqttAccelTopic); // retained, like the speed
        mqttClient.subscribe(mqttTrackTopic); // retained tag table
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

  SPI.begin(SCK_PIN, MISO_PIN, MOSI_PIN, SS_PIN);
  mfrc522.PCD_Init();

  engine.init();
  stepper = engine.stepperConnectToPin(STEP_PIN);

  if (stepper) {
    stepper->setDirectionPin(DIR_PIN);
    stepper->setEnablePin(EN_PIN);
    stepper->setAutoEnable(true);
    stepper->setSpeedInHz(speedHz);
    stepper->setAcceleration(accel);
  }

  WiFi.mode(WIFI_STA);
  WiFi.setAutoReconnect(true);
  WiFi.onEvent([](WiFiEvent_t event, WiFiEventInfo_t info) {
    if (event == ARDUINO_EVENT_WIFI_STA_GOT_IP) {
      Serial.println("WiFi connected to " + WiFi.SSID() + ", IP " + WiFi.localIP().toString() +
                     ", signal " + String(WiFi.RSSI()) + " dBm");
    } else if (event == ARDUINO_EVENT_WIFI_STA_DISCONNECTED) {
      // reason 201 = network not found, 15/202/204 = wrong password, 2 = auth timeout (signal/TX power)
      Serial.println("WiFi disconnected, reason " + String(info.wifi_sta_disconnected.reason));
    }
  });
  // multi-AP networks: scan every channel and join the strongest AP
  // instead of the first one found (a far one fails with reason 2)
  WiFi.setScanMethod(WIFI_ALL_CHANNEL_SCAN);
  WiFi.setSortMethod(WIFI_CONNECT_AP_BY_SIGNAL);
  WiFi.setSleep(false);
  connectWifi(scanForBestNetwork());
  lastWifiRetry = millis();

  telnetServer.begin();
  telnetServer.setNoDelay(true); // Faster transmission

  // setupOTA();

  // begin with an IP, not a hostname: the library prefers a hostname over
  // any IP set later, which would ignore what resolveBroker() finds
  IPAddress fallbackBroker;
  fallbackBroker.fromString(mqttHost);
  mqttClient.begin(fallbackBroker, mqttPort, net);
  mqttClient.onMessage(messageReceived);
}

void loop() {
  handleConnection();

  if (mqttClient.connected()) {
    mqttClient.loop();

    if (millis() - lastPublishTime > MQTT_PUBLISH_INTERVAL) {
      // motor telemetry in the heartbeat: the web UI shows this string
      // verbatim, so a climbing hz/pos proves the ESP32 is commanding motion
      String status = "Alive";
      if (stepper) {
        status += " run=" + String(stepper->isRunning() ? 1 : 0);
        status += " hz=" + String(stepper->getCurrentSpeedInMilliHz() / 1000);
        status += " pos=" + String(stepper->getCurrentPosition());
        status += " spd=" + String(speedHz);
        status += " acc=" + String(accel);
        status += " stop_armed=" + String(stopArmed ? 1 : 0);
        status += " track=" + (haveTrack ? String(trackVersion) : String("none"));
        if (driveActive) {
          status += " ride=" + String(driveTarget);
        }
      }
      mqttClient.publish("shotexpress/status", status);
      lastPublishTime = millis();
    }
  }

  checkDriveWatchdog();

  // handled right away: the stop tag must halt the train in the moment it is read
  if (mfrc522.PICC_IsNewCardPresent() && mfrc522.PICC_ReadCardSerial()) {
    String uid = "";
    for (byte i = 0; i < mfrc522.uid.size; i++) {
      if (mfrc522.uid.uidByte[i] < 0x10) uid += "0";
      uid += String(mfrc522.uid.uidByte[i], HEX);
    }
    uid.toUpperCase();
    mfrc522.PICC_HaltA();
    handleTag(uid);
  }
}
