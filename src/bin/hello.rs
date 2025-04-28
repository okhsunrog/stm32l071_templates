#![no_std]
#![no_main]

use cortex_m_rt::entry;
use embassy_stm32::{gpio::{Level, Output, Speed}, rcc::{LsConfig, MSIRange, Sysclk}};
use panic_abort as _;

#[entry]
fn main() -> ! {
    let mut config = embassy_stm32::Config::default();
    // {
    //     config.rcc.ls = LsConfig::off();
    //     config.rcc.msi = None;
    //     config.rcc.hsi = true;
    //     config.rcc.sys = Sysclk::HSI;
    // }
    {
        config.rcc.ls = LsConfig::off();
        config.rcc.msi = None;
    }
    let p = embassy_stm32::init(config);
    let mut led = Output::new(p.PA6, Level::Low, Speed::Low);

    // blink
    loop {
        led.toggle();
        cortex_m::asm::delay(1_000_000);
    }
}
