use defmt::{unwrap, info};
use embassy_sync::signal::Signal;
use embassy_stm32::{usart::Uart, peripherals};
use embedded_io_async::{Read, Write, ErrorType}; // Import ErrorType
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use heapless::String;
use ufmt::uwrite;
use static_cell::StaticCell;

use crate::storage::{AppState, STORAGE_MANAGER};

// Signal to notify that state has been updated
pub static STATE_UPDATED: StaticCell<Signal<CriticalSectionRawMutex, ()>> = StaticCell::new();

// In-memory copy of the state for quick access
static STATE: StaticCell<AppState> = StaticCell::new();

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
    // Remove this line: let input = input.input();

    if input.starts_with("get") {
        Command::Get
    } else if input.starts_with("set ") {
        // Extract counter value
        if let Some(value_str) = input.split_whitespace().nth(1) {
            if let Ok(counter) = value_str.parse() {
                return Command::Set { counter };
            }
        }
        Command::Unknown
    } else if input.starts_with("mode ") {
        // Extract mode value
        if let Some(value_str) = input.split_whitespace().nth(1) {
            if let Ok(mode) = value_str.parse() {
                return Command::SetMode { mode };
            }
        }
        Command::Unknown
    } else if input == "help" {
        Command::Help
    } else {
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

/// Initialize CLI state
pub fn init(initial_state: AppState) {
    // Initialize the in-memory state. `init` returns a mutable reference.
    let state_ref = STATE.init(initial_state);
    *state_ref = initial_state; // Assign the value
    // Initialize the state updated signal
    STATE_UPDATED.init(Signal::new());
}

/// Get the current state
pub fn get_state() -> AppState {
    // Access the value directly after initialization. Dereference because AppState is Copy.
    *STATE // Dereference the initialized StaticCell
}

/// Update the state and notify listeners
pub async fn update_state(state: AppState) {
    // Update the in-memory state directly
    *STATE = state; // Assign through the initialized StaticCell
    // Signal that state has been updated directly
    STATE_UPDATED.signal(()); // Access signal inside StaticCell directly
}

/// Generic function to handle the CLI session logic over any Read+Write stream.
/// This function is NOT an Embassy task itself.
async fn run_cli_session<T>(stream: &mut T)
where
    T: Read + Write + ErrorType + ?Sized, // Add ErrorType bound
    <T as ErrorType>::Error: defmt::Format, // Require the error to be formattable
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
            // Use the generic stream's read method
            let n = match stream.read(&mut rx_buf).await {
                Ok(n) => n,
                Err(e) => {
                    info!("Error reading from stream: {:?}", e);
                    // Decide how to handle the error, e.g., break the loop
                    break 'read_cmd; // Exit command reading loop on error
                }
            };

            if n == 0 { // Handle EOF or closed connection
                info!("Stream read returned 0 bytes. Closing session.");
                return;
            }

            for i in 0..n {
                let c = rx_buf[i];

                // Echo character back using the generic stream's write_all method
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
                } else if c == 8 || c == 127 { // Backspace/Delete
                    if !cmd_buf.is_empty() {
                        cmd_buf.pop();
                         // Backspace sequence
                        if stream.write_all(b"\x08 \x08").await.is_err() {
                            info!("Error writing backspace sequence. Closing session.");
                            return;
                        }
                    }
                } else if c >= 32 && c <= 126 { // Printable ASCII
                    if cmd_buf.push(c as char).is_err() {
                         // Buffer full, ignore character or handle differently
                         info!("Command buffer full.");
                    }
                }
            }
        }

        // If read loop was exited due to error, cmd_buf might be empty or incomplete
        if cmd_buf.is_empty() && response.is_empty() { // Check if response is also empty to avoid sending "> " prompt unnecessarily
             // If command buffer is empty (e.g., only Enter was pressed or read error occurred)
             // Write the prompt again if the stream is still valid
             response.clear();
             uwrite!(response, "> ").ok();
             if stream.write_all(response.as_bytes()).await.is_err() {
                 info!("Error writing prompt. Closing session.");
                 return;
             }
             continue; // Skip command processing and wait for next input
        }


        // Process command
        if !cmd_buf.is_empty() {
            info!("Processing command: {}", cmd_buf.as_str());
            response.clear();

            match parse_command(&cmd_buf) {
                Command::Get => {
                    let state = get_state();
                    uwrite!(response, "Counter: {}, Mode: {}\r\n", state.counter, state.mode).ok();
                },
                Command::Set { counter } => {
                    let mut state = get_state();
                    state.counter = counter;
                    // Access Mutex inside StaticCell directly
                    match STORAGE_MANAGER.lock().await.set_counter(counter).await {
                        Ok(_) => {
                            uwrite!(response, "Counter set to {}\r\n", counter).ok();
                            update_state(state).await;
                        },
                        Err(_) => {
                            uwrite!(response, "Failed to save counter\r\n").ok();
                        }
                    }
                },
                Command::SetMode { mode } => {
                    let mut state = get_state();
                    state.mode = mode;
                    // Access Mutex inside StaticCell directly
                    match STORAGE_MANAGER.lock().await.set_mode(mode).await {
                        Ok(_) => {
                            uwrite!(response, "Mode set to {}\r\n", mode).ok();
                            update_state(state).await;
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
                    uwrite!(response, "Unknown command. Type 'help' for available commands\r\n").ok();
                }
            }

            // Add the prompt for the next command
            uwrite!(response, "> ").ok();
            // Write response using the generic stream
            if stream.write_all(response.as_bytes()).await.is_err() {
                info!("Error writing response. Closing session.");
                return; // Exit if writing fails
            }
        } else if response.is_empty() {
            // If command was empty but no error occurred during read,
            // ensure the prompt is shown for the next input.
            response.clear();
            uwrite!(response, "> ").ok();
            if stream.write_all(response.as_bytes()).await.is_err() {
                info!("Error writing prompt after empty command. Closing session.");
                return;
            }
        }
        // Clear response buffer for the next iteration in case it wasn't used
        // (e.g., if cmd_buf was empty but response wasn't cleared above)
        // Actually, response is cleared at the start of processing, so this might be redundant.
        // response.clear();
    }
}


/// CLI task that handles user interaction.
/// This MUST keep the concrete Uart type because it's an Embassy task.
#[embassy_executor::task]
pub async fn cli_task(
    // Correct signature: Lifetime and Mode generic parameters
    mut uart: Uart<'static, embassy_stm32::mode::Async>,
) {
    info!("CLI Task started.");
    // Call the generic helper function, passing the concrete Uart instance
    run_cli_session(&mut uart).await;
    // This task will likely run forever in run_cli_session unless an error occurs there
    info!("CLI Task finished."); // Should not typically be reached unless run_cli_session returns
}

