#define STEP_PIN 19
#define DIR_PIN  18

#define WIFI_RETRY_INTERVAL 5000
#define MQTT_PUBLISH_INTERVAL 1000

// #include <Arduino.h>
#include <WiFi.h>
#include <MQTT.h>
#include <FastAccelStepper.h>

#include <ESPmDNS.h>
#include <WiFiUdp.h>
#include <ArduinoOTA.h>

const char ssid[] = "Incubator";
const char pass[] = "Fl4mongo";

const char mqttClientID[] = "StupidStepper";
const char mqttClientUsername[] = "admin";
const char mqttClientPassword[] = "123";
const char mqttHost[] = "192.168.16.127";
const char mqttSubTopic[] = "shotexpress/command";

WiFiClient net;
MQTTClient mqttClient;
 WiFiServer telnetServer(23);
WiFiClient telnetClient;

FastAccelStepperEngine engine;
FastAccelStepper *stepper = NULL;

unsigned long lastWifiRetry = 0;
unsigned long lastPublishTime = 0;

int currentState = 0; // 0 - stop, 1 - backward, 2 - forward

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

void messageReceived(String &topic, String &payload) {
  debugPrintln("Incoming: " + topic + " - " + payload);

  int newCommand = payload.toInt();

  if (newCommand != currentState) {
    currentState = newCommand;

    switch (currentState) {
      case 2:
        stepper->runForward();
        debugPrintln("Status: Run Forward");
        break;
      case 1:
        stepper->runBackward();
        debugPrintln("Status: Run Backward");
        break;
      case 0:
        stepper->stopMove();
        debugPrintln("Status: Stop");
        break;
      default:
        debugPrintln("Error: Invalid command");
        break;
    }
  }
}

// --- Connection Managers ---

void setupOTA() {
  ArduinoOTA.setHostname(mqttClientID); // Hostname for port scanning

  ArduinoOTA.onStart([]() {
    String type;
    if (ArduinoOTA.getCommand() == U_FLASH)
      type = "sketch";
    else // U_SPIFFS
      type = "filesystem";

    if (stepper) { // SAFETY
      stepper->forceStop();
    }
    debugPrintln("Start updating " + type);
  });

  ArduinoOTA.onEnd([]() {
    debugPrintln("\nEnd");
  });

  ArduinoOTA.onProgress([](unsigned int progress, unsigned int total) {
    // Avoid spamming telnet during progress, usually too fast
    // Serial.printf("Progress: %u%%\r", (progress / (total / 100)));
  });

  ArduinoOTA.onError([](ota_error_t error) {
    debugPrint("Error: ");
    if (error == OTA_AUTH_ERROR) debugPrintln("Auth Failed");
    else if (error == OTA_BEGIN_ERROR) debugPrintln("Begin Failed");
    else if (error == OTA_CONNECT_ERROR) debugPrintln("Connect Failed");
    else if (error == OTA_RECEIVE_ERROR) debugPrintln("Receive Failed");
    else if (error == OTA_END_ERROR) debugPrintln("End Failed");
  });

  ArduinoOTA.begin();
}

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

void handleConnection() {
  unsigned long currentMillis = millis();

  if (WiFi.status() != WL_CONNECTED) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      Serial.print("Connecting to WiFi...");
      WiFi.mode(WIFI_STA);
      WiFi.begin(ssid, pass);
      lastWifiRetry = currentMillis;
    }
    return;
  }

  ArduinoOTA.handle();

  handleTelnet();

  if (!mqttClient.connected()) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      debugPrintln("Connecting MQTT...");

      if (mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
        debugPrintln("MQTT Connected!");
        mqttClient.subscribe(mqttSubTopic);
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

  engine.init();
  stepper = engine.stepperConnectToPin(STEP_PIN);

  if (stepper) {
    stepper->setDirectionPin(DIR_PIN);
    stepper->setEnablePin(EN_PIN);
    stepper->setAutoEnable(true);
    stepper->setSpeedInHz(8000);
    stepper->setAcceleration(1000);
  }

  WiFi.mode(WIFI_STA);
  WiFi.begin(ssid, pass);

  telnetServer.begin();
  telnetServer.setNoDelay(true); // Faster transmission

  setupOTA();

  mqttClient.begin(mqttHost, net);
  mqttClient.onMessage(messageReceived);
}

void loop() {
  handleConnection();

  if (mqttClient.connected()) {
    mqttClient.loop();

    if (millis() - lastPublishTime > MQTT_PUBLISH_INTERVAL) {
      mqttClient.publish("shotexpress/status", "Alive");
      lastPublishTime = millis();
    }
  }
}
