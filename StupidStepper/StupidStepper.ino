#include <AccelStepper.h>

// Define Step/Dir pins
#define STEP_PIN 19
#define DIR_PIN 18
#define EN_PIN 4

// (1 : Driver + Step + Dir)
AccelStepper stepper(1, STEP_PIN, DIR_PIN);

void setup() {
  pinMode(EN_PIN, OUTPUT);
  digitalWrite(EN_PIN, LOW);

  stepper.setMaxSpeed(6000);      // Target Speed, steps per second (Try increasing this later!)
  stepper.setAcceleration(600);   // Acceleration (Lower = smoother, Higher = snappier)
}

void loop() {
  stepper.moveTo(-200000);

  // This function must be called as fast as possible in the loop
  // It decides if a step needs to be taken right now.
  while (stepper.distanceToGo() != 0) {
    stepper.run();
  }

  digitalWrite(EN_PIN, HIGH);
  delay(5000);
  digitalWrite(EN_PIN, LOW);

  stepper.moveTo(0);
  while (stepper.distanceToGo() != 0) {
    stepper.run();
  }

  digitalWrite(EN_PIN, HIGH);
  delay(5000);
  digitalWrite(EN_PIN, LOW);
}
