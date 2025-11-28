#include <AccelStepper.h>
#include <MQTT.h>
#include <WiFi.h>
#include <HardwareSerial.h>

// Define Step/Dir pins
#define STEP_PIN 19
#define DIR_PIN 18
#define EN_PIN 4

// 0 - stop
// 1 - forward
// 2 - backward
int state = 0;

// const char ssid[] = "dachboden";
// const char pass[] = "epicattic";
const char ssid[] = "Incubator";
const char pass[] = "Fl4mongo";

WiFiClient net;
MQTTClient mqttClient;

const char mqttClientID[] = "StupidStepper";
const char mqttClientUsername[] = "admin";
const char mqttClientPassword[] = "123";

// (1 : Driver + Step + Dir)
AccelStepper stepper(1, STEP_PIN, DIR_PIN);

void connect() {
  Serial.println("checking wifi...");
  while (WiFi.status() != WL_CONNECTED) {
    Serial.print(".");
    delay(1000);
  }

  Serial.print("connect mqtt: ");
  while (!mqttClient.connect(mqttClientID, mqttClientUsername, mqttClientPassword)) {
      Serial.print(".");
      delay(1000);
  }

  Serial.println("Mqtt connected");
  mqttClient.subscribe("shotexpress/command");

}

void messageReceived(String &topic, String &payload) {
  Serial.println("Incoming: " + topic + " - " + payload);

  // Convert payload to integer (0, 1, or 2)
  int newCommand = payload.toInt();

  // Only react if the command actually changed
  if (newCommand != currentCommand) {
    currentCommand = newCommand;

    // -- STATE MACHINE --
    if (currentCommand == 1) {
      // FORWARD
      digitalWrite(EN_PIN, LOW); // Enable motor driver
      stepper.moveTo(farDistance); // Go "forever" forward
    }
    else if (currentCommand == 2) {
      // BACKWARD
      digitalWrite(EN_PIN, LOW); // Enable motor driver
      stepper.moveTo(-farDistance); // Go "forever" backward
    }
    else if (currentCommand == 0) {
      // STOP
      // stepper.stop() calculates a smooth deceleration to stop
      stepper.stop();
    }
  }
}

void setup() {
  Serial.begin(115200);
  pinMode(EN_PIN, OUTPUT);
  digitalWrite(EN_PIN, HIGH);

  Serial.println("Booot");
  Serial.println("Connecting to wifi:");
  WiFi.begin(ssid, pass);

  Serial.println("WiFi connected");

  stepper.setMaxSpeed(6000);      // Target Speed, steps per second (Try increasing this later!)
  stepper.setAcceleration(600);   // Acceleration (Lower = smoother, Higher = snappier)

  mqttClient.begin("192.168.16.127", net);
  mqttClient.onMessage(messageReceived);
  connect();
  delay(5000);
}


void forward() {
    stepper.moveTo(40000);
}

void backward() {
    stepper.moveTo(-40000);
}

void stop(int currentState) {
    if (currentState == 1) {
        stepper.moveTo(100);
    }
    if (currentState == 2) {
        stepper.moveTo(-100);
    }
    digitalWrite(EN_PIN, HIGH);
}

int readCmd() {
    return 1;
}

unsigned long lastPublishTime = 0;

void loop() {
  mqttClient.loop();
  if (!mqttClient.connected()) {
    connect();
  }

  if (millis() - lastPublishTime > 1000) {
      mqttClient.publish("topic/to/publish/to", "Alive");
      lastPublishTime = millis();
    }

  stepper.run();

    // Optional: Completely cut power if stopped and destination reached
    if (currentCommand == 0 && stepper.distanceToGo() == 0) {
       digitalWrite(EN_PIN, HIGH); // Disable driver (saves power/heat)
    }
}
