#ifndef MOTOR_STEPPER_H
#define MOTOR_STEPPER_H

#include <Arduino.h>

void motor_setup();
void motor_loop();
void set_speed(int input_val); // Input: -255 to 255
void set_stop();

#endif
