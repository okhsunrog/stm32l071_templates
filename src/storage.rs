use defmt::{Format, info};
use embassy_stm32::flash::{Blocking, Flash};
use sequential_storage::{
    cache::NoCache,
    map::{fetch_item, store_item},
    Error as StorageError // Import the error type for the erase function result
};
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

// --- Flash Range Configuration ---
// Define the flash range RELATIVE TO FLASH BASE (0x08000000)
// Use the last 1 KiB (1024 bytes = 8 pages) of a 64KiB flash as an example.
// STM32L071 Page Size = 128 bytes (0x80)
// Relative Start Offset: 0x10000 (64k) - 0x400 (1k) = 0xFC00
// Relative End Offset:   0x10000 (64k)
// Ensure these are page-aligned (0xFC00 % 0x80 == 0, 0x10000 % 0x80 == 0)
//

const MAP_FLASH_RANGE: Range<u32> = 0xFE00..0x10000; // Example for 64KiB Flash
// --- End Flash Range Configuration ---

// Number of flash pages in our range (optional update based on range size)
// const PAGE_COUNT: usize = 8; // 1024 bytes / 128 bytes/page = 8 pages

pub fn async_flash_wrapper<F: NorFlash>(flash: F) -> BlockingAsync<F> {
    embassy_embedded_hal::adapter::BlockingAsync::new(flash)
}

// Storage manager that encapsulates all flash operations
pub struct StorageManager<F: AsyncNorFlash> {
    flash: F,
    // Increased buffer size to prevent potential 'Size' errors.
    data_buffer: [u8; 64],
}

// Define concrete type aliases for STORAGE_MANAGER
type ConcreteFlash = Flash<'static, Blocking>;
pub type AsyncFlash = BlockingAsync<ConcreteFlash>;
pub type ConcreteStorageManager = StorageManager<AsyncFlash>;

// Global instance of the storage manager with concrete type
pub static STORAGE_MANAGER: StaticCell<Mutex<CriticalSectionRawMutex, ConcreteStorageManager>> = StaticCell::new();

// =========================================================================
// IMPORTANT NOTE ON 'Corrupted' ERROR:
// If you encounter `Error saving counter: Corrupted`, it usually means
// the flash area defined by `MAP_FLASH_RANGE` was not erased before the
// first write attempt by `sequential-storage`.
//
// To fix this during development:
// 1. Use the `erase_map_area` function provided below.
// 2. Call it ONCE in your `main.rs` *before* the `StorageManager` is
//    put into the `STORAGE_MANAGER` mutex. See example comment in `main.rs`.
// 3. After the first successful run where the area is erased,
//    **COMMENT OUT THE CALL TO `erase_map_area` in `main.rs`**
//    to prevent erasing your stored data on every boot.
// =========================================================================

impl<F: AsyncNorFlash> StorageManager<F>
where
    F::Error: Format // Ensure the flash error type can be formatted by defmt
{
    pub fn new(flash: F) -> Self {
        Self {
            flash,
            // Ensure buffer size matches struct definition
            data_buffer: [0u8; 64],
        }
    }

    /// Erases the entire flash area designated for the storage map.
    /// Call this ONCE during development setup if you get `Corrupted` errors.
    pub async fn erase_map_area(&mut self) -> Result<(), StorageError<F::Error>> {
        info!("Erasing map storage area (relative range): {:x}..{:x}", MAP_FLASH_RANGE.start, MAP_FLASH_RANGE.end);
        // Use sequential_storage's erase_all for the map range
        sequential_storage::erase_all(&mut self.flash, MAP_FLASH_RANGE.clone()).await?;
        info!("Map storage area erased successfully.");
        Ok(())
    }


    // Initialize storage and load existing state if available
    pub async fn initialize(&mut self) -> Result<AppState, ()> {
        let mut state = AppState::default();

        // --- Optional: Erase call location ---
        // You *could* call erase here, but it's generally better to do it
        // explicitly once in main.rs during setup to avoid erasing on every init.
        // match self.erase_map_area().await {
        //      Ok(_) => info!("Storage area erased during init."),
        //      Err(e) => info!("Failed to erase during init: {}", defmt::Debug2Format(&e)),
        // }
        // --- End Optional Erase ---

        match self.get_counter().await {
            Ok(Some(counter)) => {
                info!("Loaded counter: {}", counter);
                state.counter = counter;
            }
            Ok(None) => {
                info!("No counter found in storage, using default.");
            }
            Err(_) => {
                 info!("Error reading counter, using default.");
                 // Optionally return Err here if loading is critical
            }
        }

        match self.get_mode().await {
             Ok(Some(mode)) => {
                info!("Loaded mode: {}", mode);
                state.mode = mode;
            }
            Ok(None) => {
                info!("No mode found in storage, using default.");
            }
            Err(_) => {
                info!("Error reading mode, using default.");
                // Optionally return Err here if loading is critical
            }
        }
        Ok(state)
    }

    // Get counter value from storage
    pub async fn get_counter(&mut self) -> Result<Option<u32>, ()> {
        match fetch_item::<u32, u32, _>(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(), // Uses the corrected relative range
            &mut NoCache::new(),
            &mut self.data_buffer,
            &KEY_COUNTER,
        )
        .await
        {
            Ok(value) => Ok(value),
            Err(e) => {
                // Log the specific error for debugging reads too
                info!("Error reading counter: {}", defmt::Debug2Format(&e));
                Err(())
            }
        }
    }

    // Save counter value to storage
    pub async fn set_counter(&mut self, counter: u32) -> Result<(), ()> {
        info!("Saving counter: {}", counter);
        match store_item(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(), // Uses the corrected relative range
            &mut NoCache::new(),
            &mut self.data_buffer,
            &KEY_COUNTER,
            &counter,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                // Keep the detailed error logging for Corrupted/Size errors
                info!("Error saving counter: {}", defmt::Debug2Format(&e));
                Err(())
            }
        }
    }

    // Get mode value from storage
    pub async fn get_mode(&mut self) -> Result<Option<u8>, ()> {
        match fetch_item::<u32, u8, _>(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(), // Uses the corrected relative range
            &mut NoCache::new(),
            &mut self.data_buffer,
            &KEY_MODE,
        )
        .await
        {
            Ok(value) => Ok(value),
            Err(e) => {
                // Log the specific error for debugging reads too
                info!("Error reading mode: {}", defmt::Debug2Format(&e));
                Err(())
            }
        }
    }

    // Save mode value to storage
    pub async fn set_mode(&mut self, mode: u8) -> Result<(), ()> {
        info!("Saving mode: {}", mode);
        match store_item(
            &mut self.flash,
            MAP_FLASH_RANGE.clone(), // Uses the corrected relative range
            &mut NoCache::new(),
            &mut self.data_buffer,
            &KEY_MODE,
            &mode,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                // Keep the detailed error logging
                info!("Error saving mode: {}", defmt::Debug2Format(&e));
                Err(())
            }
        }
    }
}
