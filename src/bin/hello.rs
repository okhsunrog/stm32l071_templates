#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use embassy_stm32::{
    gpio::{Level, Output, Speed},
    rcc::{Hse, HseMode, LsConfig, Sysclk},
    time::mhz,
};
use panic_abort as _;
use embassy_time::{Duration, Timer};

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
    let mut led1 = Output::new(p.PA7, Level::High, Speed::Low);
    let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

    // blink
    loop {
        led1.toggle();
        led2.toggle();
        Timer::after(Duration::from_secs(1)).await;
    }
}
