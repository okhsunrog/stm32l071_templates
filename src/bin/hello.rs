#![no_std]
#![no_main]

use embassy_stm32::rcc::{Hse, HseMode, Pll, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use panic_halt as _;

use embassy_stm32::usart::{Config, Uart};
use embedded_io::Write;

use cortex_m_rt::entry;
use embassy_time::Delay;
use embedded_hal::delay::DelayNs;
use heapless::String;
use ufmt::uwrite;

#[entry]
fn main() -> ! {
    let mut config = embassy_stm32::Config::default();
    {
        config.enable_debug_during_sleep = true;
        config.rcc.hse = Some(Hse {
            freq: Hertz::hz(16_000_000),
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
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let mut usart =
        Uart::new_blocking_with_de(p.LPUART1, p.PA3, p.PA2, p.PB1, uart_config).unwrap();

    let mut cnt = 0;
    let mut message: String<256> = String::new();

    loop {
        message.clear();
        uwrite!(message, "Hello, World {}!\r\n", cnt).ok();
        usart.write_all(message.as_bytes()).unwrap();
        cnt += 1;
        Delay.delay_ms(200);
    }
}
