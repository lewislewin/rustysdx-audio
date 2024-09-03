use rodio::{Decoder, OutputStream, Sink, Source};
use serialport::SerialPort;
use std::io::{self, Cursor};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam::channel::{bounded, Receiver};

// Configuration parameters
const AUDIO_TX_RATE: u32 = 11525;
const SERIAL_PORT: &str = "/dev/ttyUSB0";
const SERIAL_BAUD_RATE: u32 = 115200;
const BUFFER_SIZE: usize = 500;
const SERIAL_TIMEOUT_MS: u64 = 10;

// Shared state
struct SharedState {
    buf: Vec<u8>,
    underrun_counter: usize,
    tx_status: bool,
}

fn receive_serial_audio(serport: Box<dyn SerialPort>, tx: crossbeam::channel::Sender<Vec<u8>>) {
    let mut serport = serport;
    let mut buffer = [0u8; BUFFER_SIZE];
    loop {
        match serport.read(&mut buffer) {
            Ok(bytes_read) => {
                tx.send(buffer[..bytes_read].to_vec()).expect("Failed to send data");
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(SERIAL_TIMEOUT_MS));
            }
            Err(e) => {
                eprintln!("Serial read error: {:?}", e);
                break;
            }
        }
    }
}

fn play_receive_audio(rx: Receiver<Vec<u8>>, sink: Sink) {
    loop {
        match rx.recv() {
            Ok(data) => {
                if data.len() < 2 {
                    println!("UNDERRUN - refilling");
                }
                let cursor = Cursor::new(data);
                let source = Decoder::new(cursor).unwrap().convert_samples::<f32>();
                sink.append(source);
            }
            Err(_) => break,
        }
    }
}

fn main() {
    // Initialize shared state and channel
    let state = Arc::new(Mutex::new(SharedState {
        buf: Vec::new(),
        underrun_counter: 0,
        tx_status: false,
    }));

    let (tx, rx) = bounded(10);

    // Open serial port
    let mut serport = serialport::new(SERIAL_PORT, SERIAL_BAUD_RATE)
        .timeout(Duration::from_millis(SERIAL_TIMEOUT_MS))
        .open()
        .expect("Failed to open serial port");

    // Set up audio output
    let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&stream_handle).unwrap();

    // Start receiving and playing audio
    let serport_clone = serport.try_clone().expect("Failed to clone serial port");
    thread::spawn(move || receive_serial_audio(serport_clone, tx));
    thread::spawn(move || play_receive_audio(rx, sink));

    // Main loop for stats
    let start_time = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(10));
        let state = state.lock().unwrap();
        println!("{:?} BUF: {}", start_time.elapsed().as_secs(), state.buf.len());
    }
}
