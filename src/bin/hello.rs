#![no_std]
#![no_main]

use embassy_stm32::rcc::{Hse, HseMode, Pll, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use panic_probe as _;

use embassy_stm32::usart::{Config, Uart};
use embedded_io::Write;

use cortex_m_rt::{entry, exception};
use embassy_stm32::{bind_interrupts, peripherals, usart};
use embassy_time::Delay;
use embedded_hal::delay::DelayNs;
use heapless::String;
use rtt_target::rtt_init_defmt;
use ufmt::uwrite;

bind_interrupts!(struct Irqs {
    LPUART1 => usart::InterruptHandler<peripherals::LPUART1>;
});

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
    rtt_init_defmt!();
    let p = embassy_stm32::init(config);
    // the C booloader disables interrupts, so we need to re-enable them
    unsafe { cortex_m::interrupt::enable() };
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
        defmt::println!("Hello RTT! {}...", cnt);
        usart.write_all(message.as_bytes()).unwrap();
        cnt += 1;
        Delay.delay_ms(100);
        // if (cnt > 10) {
        //     panic!("test panic");
        // }
    }
}

// HardFault handler
#[exception]
unsafe fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    defmt::error!("!!! HARD FAULT !!!, frame:  {:?}", defmt::Debug2Format(ef));
    loop {
        cortex_m::asm::bkpt(); // Optional breakpoint instruction
    }
}

#[exception]
unsafe fn DefaultHandler(_irqn: i16) -> ! {
    defmt::error!("!!! Default Handler triggered for IRQn: {} !!!", _irqn);
    loop {
        cortex_m::asm::bkpt();
    }
}
