#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

//mod cli;
mod storage;

use embassy_stm32::flash::Flash;
use embassy_stm32::rcc::{Hse, HseMode, Pll, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use panic_probe as _;

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_stm32::usart::{Config, Uart};
use embassy_stm32::{bind_interrupts, peripherals, usart};
use embassy_time::Timer;
use heapless::String;
use rtt_target::rtt_init_defmt;
use ufmt::uwrite;

use storage::{StorageManager, async_flash_wrapper};

bind_interrupts!(struct Irqs {
    LPUART1 => usart::InterruptHandler<peripherals::LPUART1>;
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
    let storage_manager_mutex = storage::STORAGE_MANAGER.init(
        embassy_sync::mutex::Mutex::new(storage_manager)
    );

    // Initialize and read state
    let state = match storage_manager_mutex.lock().await.initialize().await {
        Ok(state) => {
            info!("Loaded state: counter={}, mode={}", state.counter, state.mode);
            state
        },
        Err(_) => {
            info!("Failed to initialize storage, using defaults");
            storage::AppState::default()
        }
    };

    // Initialize CLI with initial state
    //cli::init(state);

    // Initialize UART for CLI
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let usart = Uart::new_with_de(
        p.LPUART1,
        p.PA3,
        p.PA2,
        Irqs,
        p.PB1,
        p.DMA1_CH2,
        p.DMA1_CH3,
        uart_config,
    )
    .unwrap();

    // Spawn CLI task
    //unwrap!(spawner.spawn(cli::cli_task(usart)));

    // Main task can do other work in parallel
    // For example, let's periodically check the state and perform actions based on mode
    let mut message: String<256> = String::new();
    let mut cnt = 0;

    loop {
        // Wait for state updates directly on the StaticCell containing the Signal
        //cli::STATE_UPDATED.wait().await;

        // Get the current state
        //let state = cli::get_state();
        info!("Main task: state counter={}, mode={}", state.counter, state.mode);

        if state.mode > 0 {
            // Do something based on mode
            message.clear();
            uwrite!(message, "Main task: counter={}, mode={}, cnt={}\r\n",
                state.counter, state.mode, cnt).ok();
            info!("{}", message.as_str());
            cnt += 1;
        }

        // Note: The Timer::after_millis(1000).await was inside the loop but after the wait.
        // This means the loop would only execute roughly once per second *after* a state update.
        // If you want the loop to run every second regardless of state updates,
        // move the Timer outside or restructure the logic.
        // Keeping it here as per original code structure.
        Timer::after_millis(1000).await;
    }
}

