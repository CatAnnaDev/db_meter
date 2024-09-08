use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use serde_json;

// Struct for managing the moving average
struct MovingAverage {
    window: VecDeque<f32>,
    size: usize,
}

impl MovingAverage {
    fn new(size: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(size),
            size,
        }
    }

    fn add(&mut self, value: f32) -> f32 {
        if self.window.len() == self.size {
            self.window.pop_front();
        }
        self.window.push_back(value);

        let sum: f32 = self.window.iter().sum();
        sum / self.window.len() as f32
    }
}

// Trait for sound processing
trait SoundProcessor {
    fn calculate_rms(&self, samples: &[f32]) -> f32;
    fn calculate_db(&self, rms: f32) -> f32;
    fn normalize_db_to_0_100(&self, db: f32) -> f32;
}

struct AudioProcessor;

impl SoundProcessor for AudioProcessor {
    fn calculate_rms(&self, samples: &[f32]) -> f32 {
        let sum_of_squares: f32 = samples.iter().map(|&sample| sample * sample).sum();
        (sum_of_squares / samples.len() as f32).sqrt()
    }

    fn calculate_db(&self, rms: f32) -> f32 {
        20.0 * rms.max(1e-10).log10()
    }

    fn normalize_db_to_0_100(&self, db: f32) -> f32 {
        let min_db = -100.0;
        let max_db = 0.0;
        ((db - min_db) / (max_db - min_db)) * 100.0
    }
}

// Configuration struct for JSON deserialization
#[derive(Debug, Deserialize, Serialize)]
struct Config {
    meter_width: usize,
    moving_avg_size: usize,
    alert_threshold: f32,
    use_moving_average: bool,
}

impl Config {
    fn default() -> Self {
        Self {
            meter_width: 100,
            moving_avg_size: 10,
            alert_threshold: 80.0,
            use_moving_average: true,
        }
    }
}

// Struct for managing the audio stream
struct AudioStream {
    processor: AudioProcessor,
    meter_width: usize,
    moving_average: MovingAverage,
    use_moving_average: bool,
    min_level: f32,
    max_level: f32,
    current_level: f32,
    alert_threshold: f32,
    start_time: Instant,
    prev_moving_avg: Option<f32>,
}

impl Default for AudioStream {
    fn default() -> Self {
        Self {
            processor: AudioProcessor,
            meter_width: 100,
            moving_average: MovingAverage::new(10),
            use_moving_average: true,
            min_level: f32::MAX,
            max_level: f32::MIN,
            current_level: 0.0,
            alert_threshold: 80.0,
            start_time: Instant::now(),
            prev_moving_avg: None,
        }
    }
}

impl AudioStream {
    fn update_levels(&mut self, level: f32) {
        self.current_level = level;
        if level < self.min_level {
            self.min_level = level;
        }
        if level > self.max_level {
            self.max_level = level;
        }
    }

    fn calculate_trend(&self) -> &str {
        match self.prev_moving_avg {
            Some(prev) if self.current_level > prev => "↑", // Trend up
            Some(prev) if self.current_level < prev => "↓", // Trend down
            _ => "→", // No trend or initial state
        }
    }

    fn display_vu_meter(&mut self, level: f32, db: f32) {
        let meter_width = self.meter_width;
        let filled_length = (level / 100.0 * meter_width as f32).round() as usize;
        let empty_length = meter_width - filled_length;

        let color_code = if level < 33.0 {
            "32"  // Green for low levels
        } else if level < 66.0 {
            "33"  // Yellow for medium levels
        } else {
            "31"  // Red for high levels
        };

        let bar = format!(
            "\x1b[{}m[{}{}]\x1b[0m",
            color_code,
            "#".repeat(filled_length),
            " ".repeat(empty_length)
        );

        let alert = if level > self.alert_threshold {
            " !! ALERT !! "
        } else {
            ""
        };

        let trend = self.calculate_trend();
        let elapsed = Instant::now().duration_since(self.start_time);
        let elapsed_seconds = elapsed.as_secs();
        let elapsed_millis = elapsed.subsec_millis();

        print!(
            "\r{} {:.2} dB | Min: {:.2}/100 | Max: {:.2}/100 | Current: {:.2}/100 | Trend: {} | Elapsed: {}.{:03}s{}",
            bar, db, self.min_level, self.max_level, self.current_level, trend, elapsed_seconds, elapsed_millis, alert
        );
        std::io::stdout().flush().unwrap();  // Force the terminal to update

        // Update the previous moving average value
        self.prev_moving_avg = Some(level);
    }

    fn run(mut self) {
        let host = cpal::default_host();
        let device = host.default_input_device().expect("Failed to find an input device");

        let config = device.default_input_config().expect("Error in input device configuration");

        println!("Selected input device: {:?}", device.name());

        let config: cpal::StreamConfig = config.into();

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let rms = self.processor.calculate_rms(data);
                let db = self.processor.calculate_db(rms);
                let normalized_level = self.processor.normalize_db_to_0_100(db);

                let final_level = if self.use_moving_average {
                    self.moving_average.add(normalized_level)
                } else {
                    normalized_level
                };

                self.update_levels(final_level);

                // Display the vu-meter with the (smoothed or raw) level
                self.display_vu_meter(final_level, db);

            },
            move |err| {
                eprintln!("Error during capture: {}", err);
            },
            None,
        )
            .expect("Failed to create input stream");

        stream.play().expect("Failed to start the input stream");

        std::io::stdin().read_line(&mut String::new()).unwrap();
    }
}

// Load configuration from a JSON file or create it if it doesn't exist
fn load_or_create_config(file_path: &str) -> Config {
    if Path::new(file_path).exists() {
        let file = File::open(file_path).expect("Unable to open config file");
        serde_json::from_reader(file).expect("Unable to parse config file")
    } else {
        let default_config = Config::default();
        let file = File::create(file_path).expect("Unable to create config file");
        serde_json::to_writer_pretty(file, &default_config).expect("Unable to write config file");
        default_config
    }
}

fn main() {
    let config = load_or_create_config("config.json");

    let audio_stream = AudioStream {
        meter_width: config.meter_width,
        moving_average: MovingAverage::new(config.moving_avg_size),
        use_moving_average: config.use_moving_average,
        alert_threshold: config.alert_threshold,
        ..Default::default()
    };

    audio_stream.run();
}
