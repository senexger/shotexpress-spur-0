// Pumpe über IRLZ34N (Low-side) an GPIO19
// HC-SR04: TRIG=4, ECHO=5
// Pumpe geht an, wenn Wasserstand unter Schwellwert (Tank fast leer)

const int PUMP_PIN  = 19;
const int TRIG_PIN  = 4;
const int ECHO_PIN  = 5;

// Schwellwert in cm – Pumpe startet, wenn Abstand GRÖSSER als dieser Wert
// (= Wasserstand niedrig = viel Luft über dem Wasser)
const float THRESHOLD_CM = 20.0;

// Messung alle X ms
const unsigned long MEASURE_INTERVAL = 200;
unsigned long lastMeasure = 0;

float measureDistance() {
  digitalWrite(TRIG_PIN, LOW);
  delayMicroseconds(2);
  digitalWrite(TRIG_PIN, HIGH);
  delayMicroseconds(10);
  digitalWrite(TRIG_PIN, LOW);

  long duration = pulseIn(ECHO_PIN, HIGH, 30000); // Timeout 30ms (~5m)
  if (duration == 0) {
    Serial.println("  [WARNUNG] Kein Echo empfangen (Timeout)");
    return -1.0;
  }
  return (duration / 2.0) * 0.0343; // in cm
}

void setup() {
  Serial.begin(115200);
  delay(1000);
  Serial.println();
  Serial.println("=== Pumpensteuerung mit HC-SR04 ===");
  Serial.print("Schwellwert: ");
  Serial.print(THRESHOLD_CM);
  Serial.println(" cm");

  pinMode(TRIG_PIN, OUTPUT);
  pinMode(ECHO_PIN, INPUT);
  pinMode(PUMP_PIN, OUTPUT);
  digitalWrite(PUMP_PIN, LOW);
  Serial.println("Startzustand: Pumpe AUS");
}

void loop() {
  unsigned long now = millis();
  if (now - lastMeasure >= MEASURE_INTERVAL) {
    lastMeasure = now;

    float dist = measureDistance();

    if (dist < 0) return; // Fehlmessung ignorieren

    Serial.print("Abstand: ");
    Serial.print(dist, 1);
    Serial.print(" cm  →  Pumpe ");

    if (dist < THRESHOLD_CM) {
      digitalWrite(PUMP_PIN, HIGH);
      Serial.println("AN");
    } else {
      digitalWrite(PUMP_PIN, LOW);
      Serial.println("AUS");
    }
  }
}