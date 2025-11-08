# ShotExpress Webserver

A TypeScript-based HTTP server that controls the ShotExpress train via REST API endpoints.

## Getting Started

### Prerequisites
- Node.js (v16 or higher)
- npm or yarn

### Installation

1. Navigate to the webserver directory:
   ```bash
   cd webserver
   ```

2. Install dependencies:
   ```bash
   npm install
   ```

3. Build the project:
   ```bash
   npm run build
   ```

4. Start the server:
   ```bash
   npm start
   ```

### Development

For development with auto-compilation:
```bash
npm run watch
```

## API Endpoints

### GET `/train-status`
Returns the current train status and commands. This endpoint is designed to be polled by the train's ESP microcontroller.

**Response:**
```json
{
  "success": true,
  "data": {
    "speed": 0,
    "direction": "stopped",
    "command": "none",
    "timestamp": 1699459200000
  }
}
```

### POST `/control-train`
Controls the train by setting speed and commands.

**Request Body:**
```json
{
  "command": "move",
  "speed": 128
}
```

**Response:**
```json
{
  "success": true,
  "data": {
    "speed": 128,
    "direction": "forward",
    "command": "move",
    "timestamp": 1699459200000
  }
}
```

### GET `/health`
Health check endpoint.

**Response:**
```json
{
  "success": true,
  "message": "ShotExpress Webserver is running",
  "timestamp": "2024-11-08T10:00:00.000Z"
}
```

## Train Integration

The train's ESP microcontroller should periodically poll the `/train-status` endpoint to receive commands. The expected polling interval is configurable but typically ranges from 100ms to 1000ms depending on responsiveness requirements.

## Project Structure

```
webserver/
├── src/
│   └── index.ts          # Main server entry point
├── dist/                 # Compiled JavaScript output
├── package.json          # Dependencies and scripts
├── tsconfig.json         # TypeScript configuration
└── README.md             # This file
```