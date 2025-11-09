# MQTT Broker Setup

This directory contains the configuration for a Mosquitto MQTT broker running in Docker.

## Setup

1. Copy the example environment file:
   ```bash
   cp .env.example .env
   ```

2. Edit `.env` and set a secure password:
   ```bash
   MQTT_USERNAME=admin
   MQTT_PASSWORD=your_very_secure_password_here
   ```

3. Start the broker:
   ```bash
   docker-compose up
   ```

## Configuration

- **MQTT Port**: 1883
- **WebSocket Port**: 9001
- **Authentication**: Required (no anonymous access)
- **Default User**: Set via `MQTT_USERNAME` environment variable
- **Password**: Set via `MQTT_PASSWORD` environment variable

## Testing

```bash
# Subscribe to messages
mosquitto_sub -h localhost -p 1883 -u admin -P your_password -t "test/topic"

# Publish a test message
mosquitto_pub -h localhost -p 1883 -u admin -P your_password -t "test/topic" -m "Hello MQTT"
```

## Security Notes

- The `.env` file is ignored by git and should never be committed
- Change the default password before production use
- Anonymous connections to Mosquitto are disabled