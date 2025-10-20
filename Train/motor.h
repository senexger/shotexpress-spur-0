#ifndef MOTOR_H
#define MOTOR_H

#include <Arduino.h>

extern const uint8_t IN_PIN_1;
extern const uint8_t IN_PIN_2;

extern const int MAX_SPEED;
extern const int RAMP_DELAY;
extern const int STOP_DELAY;

extern const uint16_t PWM_FREQUENCY;
extern const uint8_t PWM_RESOLUTION;

void motor_setup();
void moveForward(int speed);
void moveBackward(int speed);
void moveStop();

#endif
