#![no_std]
#![no_main]

#[cfg(not(feature = "defmt"))]
use panic_halt as _;
#[cfg(feature = "defmt")]
use {defmt_rtt as _, panic_probe as _};

use embassy_stm32::usart::{Uart, Config};
use embassy_stm32::{bind_interrupts, peripherals, usart};
use defmt::info;
use embedded_io::Write;

use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_time::{Duration, Timer};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
    // #[cfg(feature = "log")]
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let mut usart = Uart::new_blocking_with_de(
        p.LPUART1,
        p.PA3,
        p.PA2,
        p.PB1,
        uart_config
    ).unwrap();

    loop {
        info!("Hello, World!");
        usart.write_all(b"Hello, World!\n").unwrap();
        Timer::after(Duration::from_millis(500)).await;
    }
}
