#![no_std]
#![no_main]

use defmt::info;
use embassy_stm32::{
    gpio::{Level, Output, Speed},
    rcc::{Hse, HseMode, LsConfig, Sysclk},
    time::mhz,
};
use embassy_time::{Duration, Timer};
use panic_abort as _;
use rtt_target::{ChannelMode::NoBlockSkip, rtt_init_defmt};

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    let mut config = embassy_stm32::Config::default();
    {
        config.rcc.ls = LsConfig::off();
        config.rcc.msi = None;
        config.rcc.hse = Some(Hse {
            mode: HseMode::Oscillator,
            freq: mhz(16),
        });
        config.rcc.sys = Sysclk::HSE;
    }
    let p = embassy_stm32::init(config);
    rtt_init_defmt!(NoBlockSkip, 512);

    let mut led1 = Output::new(p.PA7, Level::High, Speed::Low);
    let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

    // blink
    loop {
        led1.toggle();
        led2.toggle();
        info!("Hello, world!");
        Timer::after(Duration::from_secs(1)).await;
    }
}
