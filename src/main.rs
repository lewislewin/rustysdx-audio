use rodio::{Decoder, OutputStream, Sink, Source};
use serialport::SerialPort;
use std::io::{self, Cursor};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

// Configuration parameters
const AUDIO_TX_RATE: u32 = 11525;
const SERIAL_PORT: &str = "/dev/ttyUSB0";
const SERIAL_BAUD_RATE: u32 = 115200;

// Shared state
struct SharedState {
    buf: Vec<u8>,
    underrun_counter: usize,
    tx_status: bool,
}

fn receive_serial_audio(serport: Box<dyn SerialPort>, state: Arc<Mutex<SharedState>>) {
    let mut serport = serport;
    let mut buffer = [0u8; 500];
    loop {
        match serport.read(&mut buffer) {
            Ok(bytes_read) => {
                let mut state = state.lock().unwrap();
                state.buf.extend_from_slice(&buffer[..bytes_read]);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // No data yet, just wait
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                eprintln!("Serial read error: {:?}", e);
                break;
            }
        }
    }
}

fn play_receive_audio(state: Arc<Mutex<SharedState>>, sink: Sink) {
    loop {
        let mut state_guard = state.lock().unwrap(); // Correct usage of lock
        if state_guard.buf.len() < 2 {
            println!("UNDERRUN #{} - refilling", state_guard.underrun_counter);
            state_guard.underrun_counter += 1;
            while state_guard.buf.len() < 10 {
                drop(state_guard); // Release lock
                thread::sleep(Duration::from_millis(10));
                state_guard = state.lock().unwrap(); // Lock again
            }
        }
        if let Some(data) = state_guard.buf.drain(..500).collect::<Vec<u8>>().get(0..500) {
            let cursor = Cursor::new(data.to_vec());
            let source = Decoder::new(cursor).unwrap().convert_samples::<f32>();
            sink.append(source);
        }
    }
}

fn transmit_audio_via_serial(
    input_data: &[u8],
    mut serport: Box<dyn SerialPort>,
    state: Arc<Mutex<SharedState>>,
) {
    let mut buffer = Vec::new();
    loop {
        buffer.extend_from_slice(input_data);

        let min_sample = buffer.iter().cloned().min().unwrap_or(128);
        let max_sample = buffer.iter().cloned().max().unwrap_or(128);

        let mut state = state.lock().unwrap();
        if min_sample != 128 || max_sample != 128 {
            if !state.tx_status {
                state.tx_status = true;
                println!("TX ON");
                serport.write(b"UA1;TX0;").unwrap();
            }
            serport.write(&buffer).unwrap();
            println!("{}, {}, {}", buffer.len(), min_sample, max_sample);
        } else if state.tx_status {
            thread::sleep(Duration::from_millis(100));
            serport.write(b";RX;").unwrap();
            state.tx_status = false;
            println!("TX OFF");
        }
        buffer.clear();
    }
}

fn main() {
    let state = Arc::new(Mutex::new(SharedState {
        buf: Vec::new(),
        underrun_counter: 0,
        tx_status: false,
    }));

    // Open serial port
    let mut serport = serialport::new(SERIAL_PORT, SERIAL_BAUD_RATE)
        .timeout(Duration::from_millis(10))
        .open()
        .expect("Failed to open serial port");

    // Set up audio output
    let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&stream_handle).unwrap();

    // Set up audio input using cpal
    let host = cpal::default_host();
    let input_device = host.default_input_device().expect("No input device available");

    let input_config = cpal::StreamConfig {
        channels: 1,
        sample_rate: cpal::SampleRate(AUDIO_TX_RATE),
        buffer_size: cpal::BufferSize::Default,
    };

    // Buffer to store input data
    let input_data: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    let input_data_clone = input_data.clone();
    let stream = input_device.build_input_stream(
        &input_config,
        move |data: &[u8], _: &cpal::InputCallbackInfo| {
            let mut input_data = input_data_clone.lock().unwrap();
            input_data.extend_from_slice(data);
        },
        move |err| {
            eprintln!("An error occurred on input stream: {}", err);
        },
        None, // Optional latency argument, use None for default latency
    ).unwrap();

    // Start the input stream
    stream.play().unwrap();

    // Wait for device to start after opening serial port
    thread::sleep(Duration::from_secs(3));
    serport.write(b"UA1;").unwrap(); // Enable audio streaming

    // Cloning state and serial port to pass to threads
    let state_rx = Arc::clone(&state);
    let state_play = Arc::clone(&state);
    let state_tx = Arc::clone(&state);
    let serport_clone = serport.try_clone().expect("Failed to clone serial port");

    // Threads for handling different tasks
    thread::spawn(move || receive_serial_audio(serport_clone, state_rx));
    thread::spawn(move || play_receive_audio(state_play, sink));

    // Using the cpal input data for transmit
    let input_data_clone = input_data.clone();
    thread::spawn(move || {
        let input_data = input_data_clone.lock().unwrap();
        transmit_audio_via_serial(&input_data, serport, state_tx);
    });

    // Display some stats every 10 seconds
    let start_time = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(10));
        let state = state.lock().unwrap();
        println!("{:?} BUF: {}", start_time.elapsed().as_secs(), state.buf.len());
    }
}
