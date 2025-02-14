#![no_std]

pub mod fmt;

use core::cell::RefCell;
use cortex_m::interrupt::Mutex;
use embassy_stm32::mode::Blocking;
#[cfg(feature = "log")]
use {
    embassy_stm32::{
        peripherals,
        usart::{Config, InterruptHandler, Uart},
        bind_interrupts,
    },
    log::SetLoggerError,
};


//
// #[cfg(feature = "log")]
// pub struct UartLogger {
//     uart: Uart<'static, peripherals::LPUART1, Blocking>,
// }
//
// #[cfg(feature = "log")]
// impl log::Log for UartLogger {
//     fn enabled(&self, _metadata: &log::Metadata) -> bool {
//         true
//     }
//
//     fn log(&self, record: &log::Record) {
//         use core::fmt::Write;
//         let mut buffer = heapless::String::<128>::new();
//         writeln!(buffer, "{}: {}", record.level(), record.args()).unwrap();
//         self.uart.blocking_write(buffer.as_bytes()).unwrap();
//     }
//
//     fn flush(&self) {}
// }
//
// #[cfg(feature = "log")]
// pub fn setup_uart(
//     lpuart: peripherals::LPUART1,
//     tx: peripherals::PA2,
//     rx: peripherals::PA3,
//     de: peripherals::PB1,
// ) -> Uart<'static, Blocking> {
//     let mut config = Config::default();
//     config.baudrate = 57600;
//     Uart::new_blocking_with_de(lpuart, rx, tx, de, config).unwrap()
// }
//
// #[cfg(feature = "log")]
// pub fn init_logger(uart: Uart<'static, Blocking>) -> Result<(), SetLoggerError> {
//     static LOGGER: Mutex<RefCell<Option<UartLogger>>> = Mutex::new(RefCell::new(None));
//
//     critical_section::with(|cs| {
//         LOGGER.borrow(cs).replace(Some(UartLogger { uart }));
//     });
//
//     log::set_logger(unsafe { &LOGGER }).map(|()| log::set_max_level(log::LevelFilter::Info))
// }