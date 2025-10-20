#include <Arduino.h>

#include "motor.h"

void setup() {
    Serial.begin(115200);
    while (!Serial) {
        ;  // Wait for serial port to connect
    }

    // Initialize the motor pins
    //
    motor_setup();
}

void loop() {
    Serial.println("Accelerating forward...");
    for (int speed = 0; speed <= MAX_SPEED; speed++) {
        moveForward(speed);
        delay(RAMP_DELAY);
    }

    Serial.println("Hold Speed");
    delay(60000);

    Serial.println("Decelerating");
    for (int speed = MAX_SPEED; speed >= 0; speed--) {
        moveForward(speed);
        delay(RAMP_DELAY);
    }

    Serial.println("Stopped");
    moveStop();
    delay(STOP_DELAY);
}
