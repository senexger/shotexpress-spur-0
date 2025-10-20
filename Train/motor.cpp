#include "motor.h"

const uint8_t IN_PIN_1 = 22;
const uint8_t IN_PIN_2 = 23;

const int MAX_SPEED = 256;
const int RAMP_DELAY = 40;
const int STOP_DELAY = 2000;

const uint16_t PWM_FREQUENCY = 2000;
const uint8_t PWM_RESOLUTION = 8;

void motor_setup() {
    ledcAttach(IN_PIN_1, PWM_FREQUENCY, PWM_RESOLUTION);
    ledcAttach(IN_PIN_2, PWM_FREQUENCY, PWM_RESOLUTION);
}

void moveForward(int speed) {
    speed = constrain(speed, 0, 255);
    ledcWrite(IN_PIN_1, speed);
    ledcWrite(IN_PIN_2, 0);
}

void moveBackward(int speed) {
    speed = constrain(speed, 0, 255);
    ledcWrite(IN_PIN_1, 0);
    ledcWrite(IN_PIN_2, speed);
}

void moveStop() {
    ledcWrite(IN_PIN_1, 0);
    ledcWrite(IN_PIN_2, 0);
}
