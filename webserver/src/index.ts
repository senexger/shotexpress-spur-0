import express, { Request, Response } from 'express';

const app = express();
const PORT = process.env.PORT || 3000;

// Middleware
app.use(express.json());
app.use(express.urlencoded({ extended: true }));

// Train state (dummy data for now)
interface TrainStatus {
  speed: number;
  direction: 'forward' | 'backward' | 'stopped';
  command: 'move' | 'stop' | 'none';
  timestamp: number;
}

let currentTrainStatus: TrainStatus = {
  speed: 0,
  direction: 'stopped',
  command: 'none',
  timestamp: Date.now()
};

// Endpoint that the train will poll for commands
app.get('/train-status', (req: Request, res: Response) => {
  res.json({
    success: true,
    data: currentTrainStatus
  });
});

// Endpoint to control the train (for web interface or API)
app.post('/control-train', (req: Request, res: Response) => {
  const { speed, command } = req.body;
  
  if (command === 'stop') {
    currentTrainStatus = {
      speed: 0,
      direction: 'stopped',
      command: 'stop',
      timestamp: Date.now()
    };
  } else if (command === 'move' && typeof speed === 'number') {
    const normalizedSpeed = Math.max(-255, Math.min(255, speed));
    
    currentTrainStatus = {
      speed: Math.abs(normalizedSpeed),
      direction: normalizedSpeed > 0 ? 'forward' : normalizedSpeed < 0 ? 'backward' : 'stopped',
      command: 'move',
      timestamp: Date.now()
    };
  } else {
    return res.status(400).json({
      success: false,
      error: 'Invalid command or speed parameter'
    });
  }
  
  res.json({
    success: true,
    data: currentTrainStatus
  });
});

// Health check endpoint
app.get('/health', (req: Request, res: Response) => {
  res.json({
    success: true,
    message: 'ShotExpress Webserver is running',
    timestamp: new Date().toISOString()
  });
});

// Start server
app.listen(PORT, () => {
  console.log(`🚂 ShotExpress Webserver running on port ${PORT}`);
  console.log(`📡 Train polling endpoint: http://localhost:${PORT}/train-status`);
  console.log(`🎮 Control endpoint: http://localhost:${PORT}/control-train`);
  console.log(`💚 Health check: http://localhost:${PORT}/health`);
});