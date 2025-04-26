use defmt::{Format, info};
use embassy_stm32::flash::{Blocking, Flash};
use sequential_storage::{cache::KeyPointerCache, map::{fetch_item, store_item}};
use embassy_embedded_hal::adapter::BlockingAsync;
use embedded_storage::nor_flash::NorFlash;
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use core::ops::Range;
use embassy_sync::mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use static_cell::StaticCell;

// Define constants for our keys (using u32 which implements Key trait)
pub const KEY_COUNTER: u32 = 0;
pub const KEY_MODE: u32 = 1;

// Define the App State that will be kept in memory
#[derive(Format, Clone, Copy, Debug)]
pub struct AppState {
    pub counter: u32,
    pub mode: u8,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            counter: 0,
            mode: 1,
        }
    }
}

// Define the flash range for our map storage
const MAP_FLASH_RANGE: Range<u32> = 0x0800F000..0x08010000;

// Number of flash pages in our range
const PAGE_COUNT: usize = 4; // Adjust based on your flash configuration

pub fn async_flash_wrapper<F: NorFlash>(flash: F) -> BlockingAsync<F> {
    embassy_embedded_hal::adapter::BlockingAsync::new(flash)
}

// Storage manager that encapsulates all flash operations
pub struct StorageManager<F: AsyncNorFlash> {
    flash: F,
    cache: KeyPointerCache<PAGE_COUNT, u32, 8>, // Cache up to 8 keys
    data_buffer: [u8; 64],
}

// Define concrete type aliases for STORAGE_MANAGER
type ConcreteFlash = Flash<'static, Blocking>;
type AsyncFlash = BlockingAsync<ConcreteFlash>;
type ConcreteStorageManager = StorageManager<AsyncFlash>;

// Global instance of the storage manager with concrete type
pub static STORAGE_MANAGER: StaticCell<Mutex<CriticalSectionRawMutex, ConcreteStorageManager>> = StaticCell::new();

impl<F: AsyncNorFlash> StorageManager<F> {
    pub fn new(flash: F) -> Self {
        Self {
            flash,
            cache: KeyPointerCache::new(),
            data_buffer: [0u8; 64],
        }
    }

    // Initialize storage and check if it needs formatting
    pub async fn initialize(&mut self) -> Result<AppState, ()> {
        let mut state = AppState::default();

        if let Ok(Some(counter)) = self.get_counter().await {
            info!("Loaded counter: {}", counter);
            state.counter = counter;
        }

        if let Ok(Some(mode)) = self.get_mode().await {
            info!("Loaded mode: {}", mode);
            state.mode = mode;
        }

        Ok(state)
    }

    // Get counter value from storage
    pub async fn get_counter(&mut self) -> Result<Option<u32>, ()> {
        match fetch_item::<u32, u32, _>(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(),
            &mut self.cache,
            &mut self.data_buffer,
            &KEY_COUNTER,
        )
        .await
        {
            Ok(value) => Ok(value),
            Err(e) => {
                info!("Error reading counter");
                Err(())
            }
        }
    }

    // Save counter value to storage
    pub async fn set_counter(&mut self, counter: u32) -> Result<(), ()> {
        info!("Saving counter: {}", counter);
        match store_item(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(),
            &mut self.cache,
            &mut self.data_buffer,
            &KEY_COUNTER,
            &counter,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                info!("Error saving counter");
                Err(())
            }
        }
    }

    // Get mode value from storage
    pub async fn get_mode(&mut self) -> Result<Option<u8>, ()> {
        match fetch_item::<u32, u8, _>(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(),
            &mut self.cache,
            &mut self.data_buffer,
            &KEY_MODE,
        )
        .await
        {
            Ok(value) => Ok(value),
            Err(_) => {
                info!("Error reading mode");
                Err(())
            }
        }
    }

    // Save mode value to storage
    pub async fn set_mode(&mut self, mode: u8) -> Result<(), ()> {
        info!("Saving mode: {}", mode);
        match store_item(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(),
            &mut self.cache,
            &mut self.data_buffer,
            &KEY_MODE,
            &mode,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                info!("Error saving mode:");
                Err(())
            }
        }
    }
}