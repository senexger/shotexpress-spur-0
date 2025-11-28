#include <Arduino.h>
#include "motor_stepper.h"

unsigned long previousMillis = 0;
const long interval = 3000; // Change state every 3 seconds
int testState = 0;

void setup() {
    Serial.begin(115200);
    while (!Serial);

    Serial.println("Setup Motor");
    motor_setup();

    Serial.println("Motor initialized. Starting Loop...");
}

void loop() {
    // CRITICAL: This must run every cycle to generate pulses
    motor_loop();

    // Non-blocking timer for the test sequence
    unsigned long currentMillis = millis();

    if (currentMillis - previousMillis >= interval) {
        previousMillis = currentMillis;

        testState++;
        if (testState > 3) testState = 0;

        switch (testState) {
            case 0:
                Serial.println("Test: Forward (Low Speed)");
                set_speed(100); // ~780 steps/sec
                break;

            case 1:
                Serial.println("Test: Stop");
                set_stop();
                break;

            case 2:
                Serial.println("Test: Backward (Low Speed)");
                set_speed(-100); // ~-780 steps/sec
                break;

            case 3:
                Serial.println("Test: Stop");
                set_stop();
                break;
        }
    }
}
