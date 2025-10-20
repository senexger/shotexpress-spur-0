#include <Arduino.h>

// uint8_t led_pin = 2;
uint8_t in_pin_1 = 22;
uint8_t in_pin_2 = 23;

uint8_t pwm_frequency = 2000;
uint8_t pwm_resolution = 8;

void setup() {
    Serial.begin(115200);
    while (!Serial) {
      ;
    }
    ledcAttach(in_pin_1, pwm_frequency, pwm_resolution);
    ledcAttach(in_pin_2, pwm_frequency, pwm_resolution);
}

void loop() {
    Serial.println("forward");
    forward();
    delay(5000);

    Serial.println("stop");
    stop();
    delay(10000);

    Serial.println("backward");
    backward();
    delay(5000);

    Serial.println("stop");
    stop();
    delay(10000);
}

// Motor control functions
void forward() {
    ledcWrite(in_pin_1, 256);
    ledcWrite(in_pin_2, 0);
}

void backward() {
    ledcWrite(in_pin_1, 0);
    ledcWrite(in_pin_2, 155);
}

void stop() {
    ledcWrite(in_pin_1, 0);
    ledcWrite(in_pin_2, 0);
}
