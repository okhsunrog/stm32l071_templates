#![no_std]
#![no_main]

use chrono::{NaiveDate, NaiveDateTime};
use defmt::{info, unwrap};
use embassy_stm32::{
    bind_interrupts,
    flash::{Blocking, Flash},
    gpio::{Level, Output, Speed},
    peripherals,
    rcc::{Hse, HseMode, LsConfig, RtcClockSource, Sysclk},
    rtc::{Rtc, RtcConfig},
    time::mhz,
    usart::{self, BufferedUart, Config},
    wdg::IndependentWatchdog as Wdg,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use panic_abort as _;
use rtt_target::{ChannelMode::NoBlockSkip, rtt_init_defmt};

bind_interrupts!(struct Irqs {
    LPUART1 => usart::BufferedInterruptHandler<peripherals::LPUART1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) {
    rtt_init_defmt!(NoBlockSkip, 512);
    let mut config = embassy_stm32::Config::default();
    {
        config.rcc.ls = LsConfig {
            rtc: RtcClockSource::LSI,
            lsi: true,
            lse: None,
        };
        config.rcc.msi = None;
        config.rcc.hse = Some(Hse {
            mode: HseMode::Oscillator,
            freq: mhz(16),
        });
        config.rcc.sys = Sysclk::HSE;
        config.enable_debug_during_sleep = true;
    }
    let p = embassy_stm32::init(config);
    let mut wdt = Wdg::new(p.IWDG, 3_000_000);
    wdt.unleash();

    let mut led1 = Output::new(p.PA7, Level::High, Speed::Low);
    let mut led2 = Output::new(p.PA6, Level::Low, Speed::Low);

    let mut uart_config = Config::default();
    uart_config.baudrate = 57600;
    let mut tx_buf = [0u8; 256];
    let mut rx_buf = [0u8; 256];

    let mut usart = unwrap!(BufferedUart::new_with_de(
        p.LPUART1,
        p.PA3, // RX
        p.PA2, // TX
        p.PB1, // DE
        Irqs,
        &mut tx_buf,
        &mut rx_buf,
        uart_config,
    ));

    let now = NaiveDate::from_ymd_opt(2020, 5, 15)
        .unwrap()
        .and_hms_opt(10, 30, 15)
        .unwrap();

    let mut rtc = Rtc::new(p.RTC, RtcConfig::default());
    info!("Got RTC! {:?}", now.and_utc().timestamp());
    rtc.set_datetime(now.into()).unwrap();

    let f = Flash::new_blocking(p.FLASH);
    flash_test(f).await;

    loop {
        led1.toggle();
        led2.toggle();
        unwrap!(usart.write_all(b"Hello, world!\r\n").await);
        Timer::after(Duration::from_secs(1)).await;
        //let then: NaiveDateTime = rtc.now().unwrap().into();
        //info!("Got RTC! {:?}", then.and_utc().timestamp());
        wdt.pet();
    }
}

async fn flash_test(mut f: Flash<'static, Blocking>) {
    // Using 1KB in the end of the flash for storing data,
    // be sure to exclude it from the memory map for prevent
    // overwriting it with firmware.
    // 8 pages (Page 504 to Page 511), 128 bytes each.
    // Address below is 0x0800 FC00 - 0x0800 0000
    const ADDR: u32 = 0xFC00;

    info!("Reading...");
    let mut buf = [0u8; 8];
    unwrap!(f.blocking_read(ADDR, &mut buf));
    info!("Read: {=[u8]:x}", buf);

    info!("Erasing...");
    unwrap!(f.blocking_erase(ADDR, ADDR + 128));

    info!("Reading...");
    let mut buf = [0u8; 8];
    unwrap!(f.blocking_read(ADDR, &mut buf));
    info!("Read after erase: {=[u8]:x}", buf);

    info!("Writing...");
    unwrap!(f.blocking_write(ADDR, &[1, 2, 3, 4, 5, 6, 7, 8]));

    info!("Reading...");
    let mut buf = [0u8; 8];
    unwrap!(f.blocking_read(ADDR, &mut buf));
    info!("Read: {=[u8]:x}", buf);
    assert_eq!(&buf[..], &[1, 2, 3, 4, 5, 6, 7, 8]);
}
