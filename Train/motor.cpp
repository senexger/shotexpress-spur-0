#include "motor.h"

#include <Arduino.h>

const uint8_t IN_PIN_1 = 22;
const uint8_t IN_PIN_2 = 23;

const int MAX_SPEED = 255;
const int RAMP_STEP_DELAY_MS = 40;
const int REVERSE_BRAKE_MS = 1000;

const uint16_t PWM_FREQUENCY = 2000;
const uint8_t PWM_RESOLUTION = 8;

static int current_speed = 0;

static void apply_pwm();
static void ramp_to_speed(int target_speed);

void motor_setup() {
    ledcAttach(IN_PIN_1, PWM_FREQUENCY, PWM_RESOLUTION);
    ledcAttach(IN_PIN_2, PWM_FREQUENCY, PWM_RESOLUTION);
    set_stop();
}

void set_speed(int speed) {
    speed = constrain(speed, -MAX_SPEED, MAX_SPEED);
    ramp_to_speed(speed);
}

int get_speed() { return current_speed; }

int get_direction() {
    if (current_speed > 0) return 1;
    if (current_speed < 0) return -1;
    return 0;
}

void set_stop() {
    ledcWrite(IN_PIN_1, 0);
    ledcWrite(IN_PIN_2, 0);
    current_speed = 0;
    delay(REVERSE_BRAKE_MS);
}

static void ramp_to_speed(int target_speed) {
    target_speed = constrain(target_speed, -MAX_SPEED, MAX_SPEED);

    if (current_speed * target_speed < 0) {  // reverse speed
        while (current_speed != 0) {
            current_speed += (current_speed > 0) ? -1 : 1;
            apply_pwm();
            delay(RAMP_STEP_DELAY_MS);
        }
        set_stop();
    }

    while (current_speed != target_speed) {
        current_speed += (current_speed < target_speed) ? 1 : -1;
        apply_pwm();
        delay(RAMP_STEP_DELAY_MS);
    }
    apply_pwm();
}

static void apply_pwm() {
    if (current_speed > 0) {
        ledcWrite(IN_PIN_1, current_speed);
        ledcWrite(IN_PIN_2, 0);
    } else if (current_speed < 0) {
        ledcWrite(IN_PIN_1, 0);
        ledcWrite(IN_PIN_2, -current_speed);
    } else {  // TODO: needed?
        ledcWrite(IN_PIN_1, 0);
        ledcWrite(IN_PIN_2, 0);
    }
}
