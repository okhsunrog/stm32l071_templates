use core::mem::MaybeUninit; // Import MaybeUninit
use defmt::{unwrap, info};
use embassy_sync::signal::Signal;
use embassy_stm32::usart::BufferedUart;
use embedded_io_async::{Read, Write, ErrorType};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use heapless::String;
use ufmt::uwrite;

// Import the concrete types needed for the function signature
use crate::storage::{AppState, ConcreteStorageManager};

// Declare Signal directly using const fn new()
pub static STATE_UPDATED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// --- Using static mut with MaybeUninit ---
// Holds the actual state data. Initialized at runtime.
// Access MUST be synchronized and within `unsafe` blocks.
static mut STATE_RAW: MaybeUninit<AppState> = MaybeUninit::uninit();

// Mutex to synchronize access to STATE_RAW. This is crucial!
static STATE_MUTEX: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
// --- End unsafe static section ---


/// Available CLI commands
#[derive(Debug)]
pub enum Command {
    Get,
    Set { counter: u32 },
    SetMode { mode: u8 },
    Help,
    Unknown,
}

/// Parse a command from a string
pub fn parse_command(input: &str) -> Command {
    let trimmed_input = input.trim(); // Trim whitespace early
    if trimmed_input.starts_with("get") {
        Command::Get
    } else if trimmed_input.starts_with("set ") {
        // Extract counter value
        if let Some(value_str) = trimmed_input.split_whitespace().nth(1) {
            if let Ok(counter) = value_str.parse() {
                return Command::Set { counter };
            }
        }
        Command::Unknown
    } else if trimmed_input.starts_with("mode ") {
        // Extract mode value
        if let Some(value_str) = trimmed_input.split_whitespace().nth(1) {
            if let Ok(mode) = value_str.parse() {
                return Command::SetMode { mode };
            }
        }
        Command::Unknown
    } else if trimmed_input == "help" {
        Command::Help
    } else if trimmed_input.is_empty() {
        Command::Unknown
    }
     else {
        Command::Unknown
    }
}

/// Generate help text for CLI commands
pub fn get_help_text() -> &'static str {
    "Available commands:\r\n\
     get - Display current counter value and mode\r\n\
     set <value> - Set counter to <value>\r\n\
     mode <value> - Set mode to <value>\r\n\
     help - Show this help text\r\n"
}

/// Initialize CLI state (UNSAFE - writes to static mut)
pub fn init(initial_state: AppState) {
    // We don't need to lock the mutex here IF we guarantee `init`
    // is called only once, before any other task can access STATE_RAW.
    // If other tasks might start concurrently, locking IS necessary even here.
    // For simplicity assuming sequential startup:
    unsafe {
        STATE_RAW.write(initial_state);
    }
    // STATE_UPDATED is already initialized statically.
}

/// Get the current state (UNSAFE - reads static mut)
pub async fn get_state() -> AppState {
    // Lock the mutex to ensure exclusive access while reading
    let _guard = STATE_MUTEX.lock().await;
    // SAFETY: Assumes `init` has been called previously.
    // Reading from static mut requires unsafe.
    // `assume_init_read` is used because we initialized with `write`.
    unsafe { STATE_RAW.assume_init_read() }
}

/// Update the state and notify listeners (UNSAFE - writes static mut)
pub async fn update_state(new_state: AppState) {
    // Lock the mutex to ensure exclusive access while writing
    let _guard = STATE_MUTEX.lock().await;
    // SAFETY: Assumes `init` has been called previously.
    // Writing to static mut requires unsafe.
    unsafe {
        STATE_RAW.write(new_state);
    }

    // Signal that state has been updated
    STATE_UPDATED.signal(());
}


/// Generic function to handle the CLI session logic over any Read+Write stream.
/// Accepts a reference to the initialized StorageManager Mutex.
async fn run_cli_session<T>(
    stream: &mut T,
    storage: &'static Mutex<CriticalSectionRawMutex, ConcreteStorageManager>, // Pass storage manager mutex
)
where
    T: Read + Write + ErrorType + ?Sized,
    <T as ErrorType>::Error: defmt::Format,
{
    // CLI buffer
    let mut rx_buf = [0u8; 64];
    let mut cmd_buf: String<64> = String::new();
    let mut response: String<256> = String::new();

    // Welcome message
    response.clear();
    uwrite!(response, "\r\n===== STM32L071 CLI =====\r\n").ok();
    uwrite!(response, "Type 'help' for available commands\r\n> ").ok();
    unwrap!(stream.write_all(response.as_bytes()).await);

    loop {
        // Read command
        cmd_buf.clear();
        'read_cmd: loop {
            let n = match stream.read(&mut rx_buf).await {
                Ok(n) => n,
                Err(e) => {
                    info!("Error reading from stream: {:?}", e);
                    break 'read_cmd;
                }
            };

            if n == 0 {
                info!("Stream read returned 0 bytes. Closing session.");
                return;
            }

            for i in 0..n {
                let c = rx_buf[i];
                if stream.write_all(&[c]).await.is_err() {
                    info!("Error writing echo to stream. Closing session.");
                    return;
                }
                if c == b'\r' || c == b'\n' {
                    if stream.write_all(b"\r\n").await.is_err() {
                        info!("Error writing newline to stream. Closing session.");
                        return;
                    }
                    break 'read_cmd;
                } else if c == 8 || c == 127 {
                    if !cmd_buf.is_empty() {
                        cmd_buf.pop();
                        if stream.write_all(b"\x08 \x08").await.is_err() {
                            info!("Error writing backspace sequence. Closing session.");
                            return;
                        }
                    }
                } else if c >= 32 && c <= 126 {
                    if cmd_buf.push(c as char).is_err() {
                        info!("Command buffer full.");
                    }
                }
            }
        }

        let trimmed_cmd = cmd_buf.trim();
        if trimmed_cmd.is_empty() {
            response.clear();
            uwrite!(response, "> ").ok();
            if stream.write_all(response.as_bytes()).await.is_err() {
                info!("Error writing prompt. Closing session.");
                return;
            }
            continue;
        }

        info!("Processing command: {}", trimmed_cmd);
        response.clear();

        match parse_command(trimmed_cmd) {
            Command::Get => {
                let state = get_state().await; // Calls unsafe internally
                uwrite!(response, "Counter: {}, Mode: {}\r\n", state.counter, state.mode).ok();
            },
            Command::Set { counter } => {
                match storage.lock().await.set_counter(counter).await {
                    Ok(_) => {
                        let mut new_state = get_state().await; // Calls unsafe internally
                        new_state.counter = counter;
                        update_state(new_state).await; // Calls unsafe internally
                        uwrite!(response, "Counter set to {}\r\n", counter).ok();
                    },
                    Err(_) => {
                        uwrite!(response, "Failed to save counter\r\n").ok();
                    }
                }
            },
            Command::SetMode { mode } => {
                match storage.lock().await.set_mode(mode).await {
                    Ok(_) => {
                        let mut new_state = get_state().await; // Calls unsafe internally
                        new_state.mode = mode;
                        update_state(new_state).await; // Calls unsafe internally
                        uwrite!(response, "Mode set to {}\r\n", mode).ok();
                    },
                    Err(_) => {
                        uwrite!(response, "Failed to save mode\r\n").ok();
                    }
                }
            },
            Command::Help => {
                uwrite!(response, "{}", get_help_text()).ok();
            },
            Command::Unknown => {
                uwrite!(response, "Unknown command: '{}'. Type 'help' for available commands\r\n", trimmed_cmd).ok();
            }
        }

        uwrite!(response, "> ").ok();
        if stream.write_all(response.as_bytes()).await.is_err() {
            info!("Error writing response. Closing session.");
            return;
        }
    }
}

#[embassy_executor::task]
pub async fn cli_task(
    mut uart: BufferedUart<'static>,
    storage: &'static Mutex<CriticalSectionRawMutex, ConcreteStorageManager>,
) {
    info!("CLI Task started.");
    run_cli_session(&mut uart, storage).await;
    info!("CLI Task finished.");
}
