#![no_std]
#![no_main]

use embassy_stm32::rcc::{Hse, HseMode, LsConfig, Pll, PllSource, Sysclk};
use embassy_stm32::time::Hertz;
use panic_halt as _;

use embassy_stm32::usart::{Uart, Config};
use embedded_io::Write;

use cortex_m_rt::entry;
use ufmt::uwrite;
use heapless::String;
use cortex_m::peripheral::Peripherals as CorePeripherals;
use cortex_m::asm;

const VTABLE_ADDR: u32 = 0x08001100; // FLASH (app) ORIGIN from memory.x

const NOP_LOOPS: u32 = 4_000_000;

#[entry]
fn main() -> ! {
unsafe {
// Obtain Cortex-M core peripherals
let mut core_p = CorePeripherals::steal(); // Use steal() as it's guaranteed available here

core_p.SYST.disable_counter();
    core_p.SYST.disable_interrupt();
    core_p.SCB.vtor.write(VTABLE_ADDR);
    cortex_m::asm::dsb(); // Data Synchronization Barrier
    cortex_m::asm::isb(); // Instruction Synchronization Barrier
}
let mut config = embassy_stm32::Config::default();
{
    config.enable_debug_during_sleep = true;

    config.rcc.hse = Some(Hse{
        freq: Hertz::hz(16_000_000),
        mode: HseMode::Oscillator,
    });
    config.rcc.pll = Some(Pll {
        source: PllSource::HSE,
        mul: embassy_stm32::rcc::PllMul::MUL4,
        div: embassy_stm32::rcc::PllDiv::DIV2,
    });
    config.rcc.hsi = false;
    config.rcc.msi = None;
    config.rcc.sys = Sysclk::PLL1_R;
    config.rcc.ls = LsConfig::off();
}
let p = embassy_stm32::init(config);
let mut uart_config = Config::default();
uart_config.baudrate = 57600;
let mut usart = Uart::new_blocking_with_de(
    p.LPUART1,
    p.PA3,
    p.PA2,
    p.PB1,
    uart_config
).unwrap();

let mut cnt = 0;
let mut message: String<256> = String::new();

loop {
    message.clear();
    uwrite!(message, "Hello, World {}!\r\n", cnt).ok();
    usart.write_all(message.as_bytes()).unwrap();
    cnt += 1;
    for _ in 0..NOP_LOOPS {
        asm::nop(); // Execute a No-Operation instruction
    }
}


}
