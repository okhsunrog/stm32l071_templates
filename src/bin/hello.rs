#![no_std]
#![no_main]

use defmt::{info, unwrap};
use embassy_stm32::{
    bind_interrupts,
    gpio::{Level, Output, Speed},
    peripherals,
    rcc::{Hse, HseMode, LsConfig, Sysclk},
    time::mhz,
    usart::{self, BufferedUart, Config},
    wdg::IndependentWatchdog as Wdg,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use panic_abort as _;
use rtt_target::{ChannelMode::NoBlockSkip, rtt_init_defmt};

bind_interrupts!(struct Irqs {
    LPUART1 => usart::BufferedInterruptHandler<peripherals::LPUART1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    rtt_init_defmt!(NoBlockSkip, 512);
    let mut config = embassy_stm32::Config::default();
    {
        config.rcc.ls = LsConfig::off();
        config.rcc.msi = None;
        config.rcc.hse = Some(Hse {
            mode: HseMode::Oscillator,
            freq: mhz(16),
        });
        config.rcc.sys = Sysclk::HSE;
        config.enable_debug_during_sleep = true;
    }
    let p = embassy_stm32::init(config);
    let mut wdt = Wdg::new(p.IWDG, 3_000_000);
    wdt.unleash();

    let mut led1 = Output::new(p.PA7, Level::High, Speed::Low);
    let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

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

    // blink
    loop {
        led1.toggle();
        led2.toggle();
        info!("Hello, world!");
        unwrap!(usart.write_all(b"Hello, world!\r\n").await);
        Timer::after(Duration::from_secs(1)).await;
        wdt.pet();
    }
}
