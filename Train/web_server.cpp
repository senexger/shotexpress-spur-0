#include "web_server.h"

#include <SPIFFS.h>
#include <WebServer.h>
#include <WiFi.h>

#include "credentials.h"
#include "motor.h"

WebServer server(80);

void handle_root() {
    File file = SPIFFS.open("/index.html", "r");
    if (!file) {
        server.send(404, "text/plain", "404: File Not Found");
        return;
    }
    String page = file.readString();
    file.close();

    int speed = get_speed() * get_direction();
    page.replace("%%SPEED%%", String(speed));

    String direction_str = "Stopped";
    if (get_direction() == 1) {
        direction_str = "Forward";
    } else if (get_direction() == -1) {
        direction_str = "Backward";
    }
    page.replace("%%DIRECTION%%", direction_str);

    server.send(200, "text/html", page);
}

void handle_set() {
    int speed = server.arg("speed").toInt();
    set_speed(speed);

    server.sendHeader("Location", "/");
    server.send(302, "text/plain", "OK");
}

void handle_stop() {
    set_stop();

    server.sendHeader("Location", "/");
    server.send(302, "text/plain", "OK");
}

void web_server_setup() {
    Serial.begin(115200);
    delay(10);

    if (!SPIFFS.begin(true)) {
        Serial.println("An Error has occurred while mounting SPIFFS");
        return;
    }

    Serial.println();
    Serial.print("Connecting to ");
    Serial.println(SSID);

    WiFi.begin(SSID, PASSWORD);

    while (WiFi.status() != WL_CONNECTED) {
        delay(500);
        Serial.print(".");
    }

    Serial.println("");
    Serial.println("WiFi connected.");
    Serial.println("IP address: ");
    Serial.println(WiFi.localIP());

    server.on("/", HTTP_GET, handle_root);
    server.on("/set", HTTP_POST, handle_set);
    server.on("/stop", HTTP_POST, handle_stop);

    server.begin();
    Serial.println("HTTP server started");
}

void web_server_loop() { server.handleClient(); }
