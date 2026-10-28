#include <SPI.h>
#include <WiFi.h>
#include <MQTT.h>
#include <FastAccelStepper.h>
#include <MFRC522.h>
#include <ESPmDNS.h>
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
constexpr unsigned long WIFI_RESTART_INTERVAL = 20000;
constexpr unsigned long MQTT_PUBLISH_INTERVAL = 1000;

// Speed comes from the web UI (retained on shotexpress/speed, steps/s).
// DEFAULT_SPEED_HZ only applies until the retained value arrives.
constexpr unsigned long DEFAULT_SPEED_HZ = 3000;
constexpr unsigned long MAX_SPEED_HZ = 20000;
constexpr unsigned long ACCELERATION = 500; // start ramp only; stop is instant
constexpr unsigned long SHOT_STEP = 1000;

constexpr const char* ssid = SECRET_SSID;
constexpr const char* pass = SECRET_PASS;
constexpr const char* mqttClientID = "StupidStepper"; // TODO rename?
constexpr const char* mqttClientUsername = "admin";
constexpr const char* mqttClientPassword = "123";
constexpr const char* mqttHost = "192.168.16.127"; // My Laptop for now
constexpr const char* mqttCommandTopic = "shotexpress/command";
constexpr const char* mqttSpeedTopic = "shotexpress/speed";
constexpr const char* mqttStepsTopic = "shotexpress/steps"; // signed relative move
constexpr const char* mqttAccelTopic = "shotexpress/accel"; // retained, steps/s^2

// The train halts the moment it reads this tag. It then ignores it until
// any other tag was read, so driving off the tag doesn't stop it again.
constexpr const char* STOP_TAG = "93E0CD1C";

MFRC522 mfrc522(SS_PIN, RST_PIN);
bool stopArmed = true;

WiFiClient net;
MQTTClient mqttClient;

WiFiServer telnetServer(23);
WiFiClient telnetClient;

FastAccelStepperEngine engine;
FastAccelStepper *stepper = NULL;

unsigned long lastWifiRetry = 0;
unsigned long lastPublishTime = 0;
unsigned long speedHz = DEFAULT_SPEED_HZ;
unsigned long accel = ACCELERATION; // start ramp; set from the web UI

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

  if (topic == mqttSpeedTopic) {
    applySpeed(payload.toInt());
  } else if (topic == mqttAccelTopic) {
    applyAccel(payload.toInt());
  } else if (topic == mqttStepsTopic) {
    long steps = payload.toInt(); // negative = backward
    if (steps != 0) {
      stepper->move(steps);
      debugPrintln("Status: Move " + String(steps) + " Steps");
    }
  } else {
    runCommand(payload.toInt());
  }
}

void handleTag(const String& uid) {
  debugPrintln("Card ID : " + uid);

  if (uid == STOP_TAG) {
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

// Boot-time scan: shows whether the train sees our network on 2.4 GHz and
// how strong (above -70 dBm is good, below -80 dBm will not connect).
void printVisibleNetworks() {
  int count = WiFi.scanNetworks();
  Serial.println("WiFi scan: " + String(count) + " networks");
  bool found = false;
  for (int i = 0; i < count; i++) {
    bool ours = WiFi.SSID(i) == ssid;
    found = found || ours;
    Serial.println(String(ours ? "  * " : "    ") + WiFi.SSID(i) + "  " + String(WiFi.RSSI(i)) +
                   " dBm  ch " + String(WiFi.channel(i)));
  }
  if (!found) Serial.println("  " + String(ssid) + " NOT visible on 2.4 GHz from here");
  WiFi.scanDelete();
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
      lastWifiRetry = currentMillis;
    }
    return;
  }

  // ArduinoOTA.handle();

  handleTelnet();

  if (!mqttClient.connected()) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      debugPrintln("Connecting MQTT...");

      if (mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
        debugPrintln("MQTT Connected!");
        // subscriptions do not survive a reconnect
        mqttClient.subscribe(mqttCommandTopic);
        mqttClient.subscribe(mqttSpeedTopic); // retained: last speed arrives at once
        mqttClient.subscribe(mqttStepsTopic);
        mqttClient.subscribe(mqttAccelTopic); // retained, like the speed
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
      Serial.println("WiFi connected, IP " + WiFi.localIP().toString() +
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
  printVisibleNetworks();
  Serial.println("Connecting to WiFi " + String(ssid) + "...");
  WiFi.begin(ssid, pass);
  lastWifiRetry = millis();

  telnetServer.begin();
  telnetServer.setNoDelay(true); // Faster transmission

  // setupOTA();

  mqttClient.begin(mqttHost, net);
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
      }
      mqttClient.publish("shotexpress/status", status);
      lastPublishTime = millis();
    }
  }

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
