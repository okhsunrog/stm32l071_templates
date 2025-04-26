#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod cli;
mod storage;

use embassy_stm32::flash::Flash;
use embassy_stm32::rcc::{Hse, HseMode, Pll, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use panic_probe as _;

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_stm32::usart::{Config, BufferedUart};
use embassy_stm32::{bind_interrupts, peripherals, usart};
// use embassy_time::Timer; // Timer is no longer used in the loop
use heapless::String;
use rtt_target::rtt_init_defmt;
use ufmt::uwrite;

use storage::async_flash_wrapper;

bind_interrupts!(struct Irqs {
    LPUART1 => usart::BufferedInterruptHandler<peripherals::LPUART1>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // the C booloader disables interrupts, so we need to re-enable them
    unsafe { cortex_m::interrupt::enable() };
    rtt_init_defmt!();
    let mut config = embassy_stm32::Config::default();
    {
        config.rcc.hse = Some(Hse {
            freq: Hertz::mhz(16),
            mode: HseMode::Oscillator,
        });
        config.rcc.pll = Some(Pll {
            source: PllSource::HSE,
            mul: embassy_stm32::rcc::PllMul::MUL4,
            div: embassy_stm32::rcc::PllDiv::DIV2,
        });
        config.rcc.sys = Sysclk::PLL1_R;
    }
    let p = embassy_stm32::init(config);

    // Initialize flash
    let flash = async_flash_wrapper(Flash::new_blocking(p.FLASH));

    // Create and initialize the storage manager
    let storage_manager = storage::StorageManager::new(flash);
    // Get the static reference to the initialized Mutex
    let storage_manager_mutex = storage::STORAGE_MANAGER.init(
        embassy_sync::mutex::Mutex::new(storage_manager)
    );

    // Initialize and read state from storage
    let initial_state = match storage_manager_mutex.lock().await.initialize().await {
        Ok(state) => {
            info!("Loaded state: counter={}, mode={}", state.counter, state.mode);
            state
        },
        Err(_) => {
            info!("Failed to initialize/load storage, using defaults");
            storage::AppState::default()
        }
    };

    // Initialize CLI state (in-memory state mutex and update signal)
    // This calls STATE.init() and STATE_UPDATED.init() internally
    cli::init(initial_state);

    // Initialize UART for CLI
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    static mut TX_BUF: [u8; 256] = [0; 256];
    static mut RX_BUF: [u8; 256] = [0; 256];
    // Use unsafe to get mutable references to static buffers
    let (tx_buf, rx_buf) = unsafe { (&mut TX_BUF, &mut RX_BUF) };

    let usart = BufferedUart::new_with_de(
        p.LPUART1,
        Irqs,
        p.PA3, // RX
        p.PA2, // TX
        p.PB1, // DE/RE - Adjust pin if different or not used
        tx_buf,
        rx_buf,
        uart_config,
    )
    .unwrap();

    // Spawn CLI task, passing the storage manager mutex reference
    unwrap!(spawner.spawn(cli::cli_task(usart, storage_manager_mutex)));

    // Main task can do other work in parallel
    // For example, let's periodically react to state changes
    let mut message: String<256> = String::new();
    let mut cnt = 0;

    loop {
        // Wait for state updates using the initialized Signal
        // STATE_UPDATED dereferences to the Signal, so call .wait() directly.
        cli::STATE_UPDATED.wait().await;

        // Get the current state (now async because it locks the state mutex)
        let state = cli::get_state().await;
        info!("Main task notified: state counter={}, mode={}", state.counter, state.mode);

        // Example action based on state
        if state.mode > 0 {
            message.clear();
            uwrite!(message, "Main task action: counter={}, mode={}, cnt={}\r\n",
                state.counter, state.mode, cnt).ok();
            info!("{}", message.as_str());
            cnt += 1;
        }

        // No Timer delay here, the loop naturally blocks on wait() until a signal
    }
}
