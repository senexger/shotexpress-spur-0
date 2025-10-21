#include <Arduino.h>

#include "motor.h"
#include "web_server.h"

void setup() {
    Serial.begin(115200);
    while (!Serial) {
        ;
    }

    motor_setup();

    web_server_setup();
}

void loop() { web_server_loop(); }
