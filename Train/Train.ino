#define STEP_PIN 19
#define DIR_PIN  18
#define EN_PIN   4

#define WIFI_RETRY_INTERVAL 5000
#define MQTT_PUBLISH_INTERVAL 1000

// #include <Arduino.h>
#include <WiFi.h>
#include <MQTT.h>
#include <FastAccelStepper.h>

const char ssid[] = "Incubator";
const char pass[] = "Fl4mongo";

const char mqttClientID[] = "StupidStepper";
const char mqttClientUsername[] = "admin";
const char mqttClientPassword[] = "123";
const char mqttHost[] = "192.168.16.127";
const char mqttSubTopic[] = "shotexpress/command";

WiFiClient net;
MQTTClient mqttClient;

FastAccelStepperEngine engine;
FastAccelStepper *stepper = NULL;

unsigned long lastWifiRetry = 0;
unsigned long lastPublishTime = 0;

int currentState = 0; // 0 - stop, 1 - backward, 2 - forward

void messageReceived(String &topic, String &payload) {
  Serial.println("Incoming: " + topic + " - " + payload);

  int newCommand = payload.toInt();

  if (newCommand != currentState) {
    currentState = newCommand;

    switch (currentState) {
      case 2:
        stepper->runForward();
        break;
      case 1:
        stepper->runBackward();
        break;
      case 0:
        stepper->stopMove();
        break;
      default:
        Serial.println("Invalid command");
        break;
    }
  }
}

// Non-blocking connection manager
void handleConnection() {
  unsigned long currentMillis = millis();

  // 1. Check WiFi
  if (WiFi.status() != WL_CONNECTED) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      Serial.print("Connecting to WiFi...");
      // WiFi.disconnect(); // Optional: sometimes helps to reset
      WiFi.begin(ssid, pass);
      lastWifiRetry = currentMillis;
    }
    return;
  }

  if (!mqttClient.connected()) {
    if (currentMillis - lastWifiRetry > WIFI_RETRY_INTERVAL) {
      Serial.print("Connecting MQTT...");

      if (mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
        Serial.println("\nMQTT Connected!");
        mqttClient.subscribe(mqttSubTopic);
      } else {
        Serial.print(".");
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

    // Ramp up settings
    stepper->setSpeedInHz(8000);
    stepper->setAcceleration(1000);
  }

  WiFi.begin(ssid, pass);

  mqttClient.begin(mqttHost, net);
  mqttClient.onMessage(messageReceived);
}

// Accelstepper runs in the background.
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
