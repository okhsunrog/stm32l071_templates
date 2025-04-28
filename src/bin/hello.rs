#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

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

bind_interrupts!(struct Irqs {
    LPUART1 => usart::InterruptHandler<peripherals::LPUART1>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // the C booloader disables interrupts, so we need to re-enable them
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
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let mut usart = Uart::new_with_de(
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
    let mut cnt = 0;
    let mut message: String<256> = String::new();

    loop {
        message.clear();
        uwrite!(message, "Hello, World {}!\r\n", cnt).ok();
        info!("Hello RTT! {}...", cnt);
        unwrap!(usart.write(message.as_bytes()).await);
        cnt += 1;
        Timer::after_millis(300).await;
    }
}
