#![no_std]
#![no_main]

use cortex_m_rt::entry;
use embassy_stm32::{
    gpio::{Level, Output, Speed},
    rcc::{Hse, HseMode, LsConfig, Sysclk},
    time::mhz,
};
use panic_abort as _;
use embassy_time::Delay;
use embedded_hal::delay::DelayNs;

#[entry]
fn main() -> ! {
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
    //let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

    // blink
    loop {
        led1.toggle();
        //led2.toggle();
        Delay.delay_ms(1000);
    }
}
