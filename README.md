# ShotExpress

A train control system consisting of two main components:
- `/train`, an ESP microcontroller project in C that controls the train's motor, and
- `/webserver`, a Typescript project for controlling the train.

## Communication Architecture

The train continuously polls the webserver's `/train-status` endpoint to receive commands.

## Getting Started

### Train Setup
1. Configure your WiFi credentials in `train/credentials.h`
2. Upload the code to your ESP32/ESP8266
3. The train will poll the central webserver

### Webserver Setup
1. Navigate to the `webserver/` directory
2. Run `npm install` to install dependencies
3. Run `npm run build` to compile TypeScript
4. Run `npm start` to start the server
