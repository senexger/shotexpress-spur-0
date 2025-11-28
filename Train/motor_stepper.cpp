#include "motor_stepper.h"
#include <TMCStepper.h>
#include <AccelStepper.h>

// --- PIN DEFINITIONS ---
const uint8_t STEP_PIN = 18;
const uint8_t DIR_PIN  = 19;
const uint8_t UART_RX  = 26;
const uint8_t UART_TX  = 27;

// --- LOGIC CONSTANTS ---
const int MAX_INPUT_VAL = 255;      // The range used in your test (-255 to 255)
const int MAX_STEPPER_SPEED = 2000; // Actual Steps per second

// --- DRIVER SETTINGS ---
#define R_SENSE 0.11f
#define DRIVER_ADDRESS 0b00
#define SERIAL_PORT Serial2

// Initialize Objects
TMC2209Stepper driver(&SERIAL_PORT, R_SENSE, DRIVER_ADDRESS);
AccelStepper stepper(AccelStepper::DRIVER, STEP_PIN, DIR_PIN);

void motor_setup() {
    // 1. Start Serial for Driver Communication
    SERIAL_PORT.begin(115200, SERIAL_8N1, UART_RX, UART_TX);

    // 2. Configure TMC2209 Driver
    driver.begin();
    driver.toff(5);                 // Enable driver (0 = off)
    driver.rms_current(1000);        // 600mA RMS
    driver.microsteps(16);          // 1/16 Microsteps
    driver.pwm_autoscale(true);     // StealthChop auto-tuning

    // 3. Configure AccelStepper
    // We set a high max limit, but specific speeds are set in set_speed()
    stepper.setMaxSpeed(MAX_STEPPER_SPEED + 500);

    // Explicit pin modes
    pinMode(STEP_PIN, OUTPUT);
    pinMode(DIR_PIN, OUTPUT);
}

void motor_loop() {
    // This must be called as fast as possible in the main loop
    stepper.runSpeed();
}

// --- MISSING FUNCTIONS ADDED BELOW ---

void set_speed(int input_val) {
    // 1. Constrain input to safety limits
    int safe_input = constrain(input_val, -MAX_INPUT_VAL, MAX_INPUT_VAL);

    // 2. Map the input (0-255) to real Motor Steps (0-2000)
    // Note: map() handles negative numbers correctly automatically
    float target_speed = map(safe_input, -MAX_INPUT_VAL, MAX_INPUT_VAL, -MAX_STEPPER_SPEED, MAX_STEPPER_SPEED);

    // 3. Apply speed to stepper
    stepper.setSpeed(target_speed);
}

void set_stop() {
    stepper.setSpeed(0);
}
