#![no_std]
#![no_main]

use cortex_m_rt::entry;
use embassy_stm32::pac::{self, gpio::vals};
use panic_abort as _;

const TICK_HZ: f32 = 2_000_000.0;
const DELAY_S: f32 = 1.0;
const DELAY: u32 = (DELAY_S * TICK_HZ) as u32;

#[entry]
fn main() -> ! {
    // Enable GPIOA clock
    let rcc = pac::RCC;
    rcc.gpioenr().modify(|w| w.set_gpioaen(true));
    rcc.gpiorstr().modify(|w| {
        w.set_gpioarst(true);
        w.set_gpioarst(false);
    });
    let gpioa = pac::GPIOA;
    const LED_PIN: usize = 6;

    gpioa
        .pupdr()
        .modify(|w| w.set_pupdr(LED_PIN, vals::Pupdr::FLOATING));
    gpioa
        .otyper()
        .modify(|w| w.set_ot(LED_PIN, vals::Ot::PUSH_PULL));
    gpioa
        .moder()
        .modify(|w| w.set_moder(LED_PIN, vals::Moder::OUTPUT));

    // blink loop
    loop {
        pac::GPIOA.bsrr().write(|w| w.set_bs(LED_PIN, true));
        cortex_m::asm::delay(DELAY);
        pac::GPIOA.bsrr().write(|w| w.set_br(LED_PIN, true));
        cortex_m::asm::delay(DELAY);
    }
}
