#![no_std]
#![no_main]

// Include the storage module we created
mod storage;

#[allow(unused_imports)]
use chrono::{NaiveDate, NaiveDateTime};
use defmt::{info, unwrap};
use embassy_stm32::{
    bind_interrupts,
    flash::Flash, // We need the Blocking Flash for storage::init
    gpio::{Level, Output, Speed},
    peripherals,
    rcc::{Hse, HseMode, LsConfig, RtcClockSource, Sysclk},
    rtc::{Rtc, RtcConfig},
    time::mhz,
    usart::{self, BufferedUart, Config},
    wdg::IndependentWatchdog as Wdg,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use panic_abort as _; // Or your preferred panic handler
use rtt_target::{ChannelMode::NoBlockSkip, rtt_init_defmt};

// Import specific types from storage if needed for examples
use storage::{HeatMode, HeaterNvdata};
// Import heapless if using String/Vec in examples
use heapless::String;

bind_interrupts!(struct Irqs {
    LPUART1 => usart::BufferedInterruptHandler<peripherals::LPUART1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    // Initialize RTT logging first
    // This also sets up the `log` facade backend used by storage.rs
    rtt_init_defmt!(NoBlockSkip, 512);

    info!("System Booting...");

    // Configure clocks and peripherals
    let mut config = embassy_stm32::Config::default();
    {
        config.rcc.ls = LsConfig {
            rtc: RtcClockSource::LSI, // Or LSE if available and configured
            lsi: true,
            lse: None,
        };
        config.rcc.msi = None;
        config.rcc.hse = Some(Hse {
            mode: HseMode::Oscillator,
            freq: mhz(16), // Match your HSE crystal frequency
        });
        config.rcc.sys = Sysclk::HSE;
        config.enable_debug_during_sleep = true; // Optional: for debugging sleep modes
    }
    let p = embassy_stm32::init(config);
    info!("Peripherals Initialized.");

    // Initialize Watchdog
    let mut wdt = Wdg::new(p.IWDG, 3_000_000); // ~3 second timeout with LSI
    wdt.unleash();
    info!("Watchdog Unleashed.");

    // Initialize LEDs
    let mut led1 = Output::new(p.PA7, Level::High, Speed::Low);
    let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

    // Initialize UART
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let mut tx_buf = [0u8; 256];
    let mut rx_buf = [0u8; 256];
    let mut usart = unwrap!(BufferedUart::new_with_de(
        p.LPUART1,
        p.PA3, // RX
        p.PA2, // TX
        p.PB1, // DE
        Irqs,
        &mut tx_buf,
        &mut rx_buf,
        uart_config,
    ));
    info!("UART Initialized.");

    // Initialize RTC
    let now_dt = NaiveDate::from_ymd_opt(2024, 5, 3)
        .unwrap()
        .and_hms_opt(18, 57, 00)
        .unwrap();
    let mut rtc = Rtc::new(p.RTC, RtcConfig::default());
    rtc.set_datetime(now_dt.into()).unwrap();

    // --- Initialize Persistent Storage ---
    // Get the blocking flash peripheral instance
    let flash_peripheral = Flash::new_blocking(p.FLASH);
    // Call the blocking init function from our storage module
    storage::init(flash_peripheral);
    info!("Storage Initialized.");
    // `storage::init` handles checking the marker and erasing if necessary.
    // --- Storage Initialized ---

    // --- Run Storage Example ---
    // Call the function that demonstrates using the storage API
    storage_example(); // This function uses the blocking storage API internally
    info!("Storage Example Finished.");
    // --- Storage Example Done ---

    info!("Entering main loop...");
    loop {
        led1.toggle();
        led2.toggle();

        unwrap!(usart.write_all(b"Hello world!").await);

        Timer::after(Duration::from_secs(1)).await;
        // Pet the watchdog regularly
        wdt.pet();
    }
}

/// Function to demonstrate usage of the persistent storage module.
/// This function uses the **blocking** API calls from `storage.rs`.
fn storage_example() {
    info!("--- Running Storage Example ---");

    // 1. Simple Counter (using generic get/insert)
    let counter_key = "app/run_count";
    match storage::get::<u32>(counter_key) {
        Ok(Some(count)) => {
            info!("Current run count from storage: {}", count);
            let next_count = count.wrapping_add(1);
            info!("Incrementing run count to: {}", next_count);
            unwrap!(
                storage::insert(counter_key, &next_count),
                "Failed to save counter"
            );
        }
        Ok(None) => {
            info!("No run count found, initializing to 1.");
            unwrap!(
                storage::insert(counter_key, &1u32),
                "Failed to save initial counter"
            );
        }
        Err(e) => {
            info!("Error reading counter: {:?}", defmt::Debug2Format(&e));
            // Decide how to handle read errors, maybe default/reset?
            info!("Setting counter to 0 due to read error.");
            unwrap!(
                storage::insert(counter_key, &0u32),
                "Failed to save reset counter"
            );
        }
    }

    // --- In src/main.rs, inside storage_example() ---

    // 2. Device Name (String, using specific helpers)
    match storage::get_device_name() {
        // This now returns Result<Option<StorableString<22>>, ...>
        Ok(Some(name_wrapper)) => {
            // name_wrapper is StorableString<22>
            // Access the inner heapless::String using Deref .0 or as_str()
            info!("Read device name: '{}'", name_wrapper.as_str());
            // Optionally modify and save back
            // let mut new_name_wrapper = name_wrapper.clone(); // Clone the wrapper
            // new_name_wrapper.0.push_str("!").ok(); // Modify inner string
            // info!("Updating device name to: '{}'", new_name_wrapper.as_str());
            // unwrap!(storage::set_device_name(&new_name_wrapper)); // Pass the wrapper ref
        }
        Ok(None) => {
            info!("No device name found. Setting default.");
            // Create the heapless::String first
            let default_heapless_name: String<22> = String::try_from("STM32L071 Device").unwrap();
            // Wrap it in StorableString
            let default_storable_name = storage::StorableString(default_heapless_name);
            // Pass the wrapper ref to set_device_name
            unwrap!(
                storage::set_device_name(&default_storable_name),
                "Failed to set default name"
            );
        }
        Err(e) => {
            info!("Error reading device name: {:?}", defmt::Debug2Format(&e));
        }
    }

    // 3. Heater Configuration (Struct, using specific helpers)
    let default_heat_cfg = HeaterNvdata {
        mode: HeatMode::Off,
        hysteresis: 5,  // Example: 0.5 C
        threshold: 200, // Example: 20.0 C
    };
    match storage::get_heater_config() {
        Ok(Some(cfg)) => {
            info!(
                "Read heater config: Mode={:?}, Hys={}, Thr={}",
                cfg.mode, cfg.hysteresis, cfg.threshold
            );
            // Example: Change mode if currently Off
            if cfg.mode == HeatMode::Off {
                let mut new_cfg = cfg;
                new_cfg.mode = HeatMode::Auto; // Change to Auto
                new_cfg.threshold = 225; // Set new threshold 22.5 C
                info!(
                    "Heater was Off, changing to Auto with threshold {}",
                    new_cfg.threshold
                );
                unwrap!(
                    storage::set_heater_config(&new_cfg),
                    "Failed to update heater config"
                );
            }
        }
        Ok(None) => {
            info!(
                "No heater config found. Setting default: {:?}",
                default_heat_cfg
            );
            unwrap!(
                storage::set_heater_config(&default_heat_cfg),
                "Failed to set default heater config"
            );
        }
        Err(e) => {
            info!("Error reading heater config: {:?}", defmt::Debug2Format(&e));
            info!("Setting default heater config due to read error.");
            unwrap!(
                storage::set_heater_config(&default_heat_cfg),
                "Failed to set default heater config after error"
            );
        }
    }

    // 4. Read a non-existent key
    match storage::get::<f64>("app/calibration_factor") {
        Ok(None) => {
            info!("Successfully confirmed 'app/calibration_factor' does not exist.");
        }
        Ok(Some(_)) => {
            info!("Error: Non-existent key 'app/calibration_factor' unexpectedly found!");
        }
        Err(e) => {
            info!(
                "Error reading non-existent key: {:?}",
                defmt::Debug2Format(&e)
            );
            // This might happen if storage is corrupted, but usually should just return Ok(None)
        }
    }

    // 5. Optional: Test erase_all (Use with extreme caution!)
    // Uncomment the following lines ONLY if you want to wipe the storage during testing.
    // info!("!!! WARNING: Erasing all stored data... !!!");
    // unwrap!(storage::erase_all(), "Failed to erase all storage");
    // info!("Storage erase complete. Check logs on next boot.");
    // // After erasing, trying to read the counter should yield None
    // match storage::get::<u32>(counter_key) {
    //     Ok(None) => info!("Confirmed counter is None after erase."),
    //     _ => info!("Error: Counter was not None after erase!"),
    // }

    info!("--- Storage Example Finished ---");
}

// The old flash_test function is removed as it conflicts with managed storage.
// fn flash_test(...) { ... }
