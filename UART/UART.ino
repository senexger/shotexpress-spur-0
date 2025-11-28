#include "esp32-hal.h"
#include <TMCStepper.h>
#include <system_error>

// --- PIN DEFINITIONS ---
const uint8_t UART_RX = 16; // ESP32 RX pin
const uint8_t UART_TX = 17; // ESP32 TX pin

// --- DRIVER SETTINGS ---
#define R_SENSE 0.11f
#define DRIVER_ADDRESS 0b00
#define SERIAL_PORT Serial2

TMC2209Stepper driver(&SERIAL_PORT, R_SENSE, DRIVER_ADDRESS);

void setup() {
  Serial.begin(115200);
  while(!Serial);
  Serial.println("\n--- TMC2209 CONNECTION TEST ---");

  // Start UART
  SERIAL_PORT.begin(115200, SERIAL_8N1, UART_RX, UART_TX);

  driver.begin();
  driver.toff(5);            // Turn on driver
  driver.rms_current(600);   // Set some current

  // --- THE TEST ---
  // We try to read the version register of the chip.
  // A correct TMC2209 always returns 0x21 (33 in decimal).
  uint8_t result = driver.version();

  Serial.print("Driver Version Register: 0x");
  Serial.println(result, HEX);

  if (result == 0x21) {
    Serial.println("✅ SUCCESS: Connection is GOOD.");
    Serial.print("Connection test: ");
    Serial.println(driver.test_connection()); // Should be 0
  } else if (result == 0x00 || result == 0xFF) {
    Serial.println("❌ FAILURE: No response (0x00) or Bad wiring (0xFF).");
    Serial.println("Check RX/TX wiring and the 1k resistor.");
  } else {
    Serial.println("⚠️ WARNING: Unknown chip version. Are you sure it's a 2209?");
  }
}

void loop() {
  // Nothing to do here
}
