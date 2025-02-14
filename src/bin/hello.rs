#![no_std]
#![no_main]

#[cfg(not(feature = "defmt"))]
use panic_halt as _;
#[cfg(feature = "defmt")]
use {defmt_rtt as _, panic_probe as _};

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::usart::{BufferedUart, Config, Uart};
use embassy_stm32::{bind_interrupts, peripherals, usart};
use embassy_time::{Duration, Timer};
use embedded_io::{Read, Write};
use heapless::String;
use embedded_io_async::BufRead;

bind_interrupts!(struct Irqs {
    USART1 => usart::BufferedInterruptHandler<peripherals::USART1>;
});

static LASER_COMMAND: [u8; 4] = [b'D', b'F', b'M', b'S'];

#[embassy_executor::task]
async fn laser_task(mut laser: BufferedUart<'static>, mut laser_control: Output<'static>) {
    for command in LASER_COMMAND.into_iter().cycle() {
        //laser_control.set_high();
        Timer::after(Duration::from_millis(2000)).await;
        laser.write_all(&[command]).unwrap();
        laser.flush().unwrap();
        info!("Sending command: {}", command as char);
        laser.flush().unwrap();
        Timer::after(Duration::from_millis(5000)).await;
        let mut read_buf = [0u8; 100];
        let result = laser.read(&mut read_buf);
        match result {
            Ok(len) => {
                // Convert byte slice to string and print
                if let Ok(str_data) = core::str::from_utf8(&read_buf[..len]) {
                    info!("Received: {}", str_data);
                } else {
                    // Fallback to printing as ASCII chars
                    let ascii_str: String<100> = read_buf[..len]
                        .iter()
                        .map(|&b| b as char)
                        .collect();
                    info!("Received (ASCII): {}", ascii_str);
                }
            }
            Err(err) => {
                info!("Error: {}", err);
            }
        }
        //laser_control.set_low();
        Timer::after(Duration::from_millis(1000)).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    let mut stm32_config = embassy_stm32::Config::default();
    {
        use embassy_stm32::{rcc::*, time::hz};
        stm32_config.rcc.hse = Some(Hse {
            freq: hz(16_000_000),
            mode: HseMode::Oscillator,
        });
        stm32_config.rcc.sys = Sysclk::HSE;
    }
    let p = embassy_stm32::init(stm32_config);
    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    // let mut usart =
    //     Uart::new_blocking_with_de(p.LPUART1, p.PA3, p.PA2, p.PB1, uart_config).unwrap();
    let mut laser_gpio = Output::new(p.PB12, Level::High, Speed::Low);

    static mut tx_buf: [u8; 1024] = [0u8; 1024];
    static mut rx_buf: [u8; 1024] = [0u8; 1024];
    let mut laser_conf = Config::default();
    laser_conf.baudrate = 19200;
    laser_conf.assume_noise_free = false;
    let mut laser_usart = BufferedUart::new(
        p.USART1,
        Irqs,
        p.PA10,
        p.PA9,
        unsafe { &mut tx_buf },
        unsafe { &mut rx_buf },
        laser_conf,
    )
    .unwrap();
    spawner.spawn(laser_task(laser_usart, laser_gpio)).unwrap();

    loop {
        // info!("Hello, World!");
        // usart.write_all(b"Hello, World!\n").unwrap();
        Timer::after(Duration::from_millis(500)).await;
    }
}
