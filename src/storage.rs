//! src/storage.rs

#![deny(missing_docs)]
//! Provides key-value pair persistent storage on flash, inspired by ariel-os-storage.
//! Uses a blocking API, wrapping async sequential-storage calls internally,
//! suitable for hardware with only blocking flash drivers like STM32L0.
//! Uses `defmt` directly for logging.

use core::ops::Range;
use embassy_stm32::flash::{Flash, FlashError, PAGE_SIZE, Blocking}; // Use Blocking HAL Flash
use embassy_sync::{
    blocking_mutex::{ // Use blocking mutex
        raw::CriticalSectionRawMutex,
        Mutex as BlockingMutex,
        MutexGuard as BlockingMutexGuard,
    },
    once_lock::OnceLock,
};
// Import the wrapper to make blocking flash compatible with async traits
use embassy_embedded_hal::adapter::BlockingAsync;
// Import the correct blocker for Embassy tasks
use embassy_futures::block_on;
// Traits required by sequential-storage
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
// Logging directly via defmt
use defmt; // Make defmt macros available
// Serialization/Deserialization
use postcard::experimental::max_size::MaxSize;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
// Fixed-size collections commonly used in embedded
use heapless;

// Re-export the storage Error type for convenience
pub use sequential_storage::Error;

// --- Configuration ---

/// Offset between the microcontroller's memory map (e.g., 0x08000000)
/// and the address base expected by the HAL flash driver (usually 0x0).
const FLASH_OFFSET: u32 = 0x0800_0000;

/// The specific key used to check if storage has been initialized.
const MARKER_KEY: &str = "__INIT_MARKER";
/// The value associated with the marker key.
const MARKER_VALUE: u8 = 0xAA;

/// Size of the buffer used for internal sequential-storage operations.
/// Must be large enough to hold the largest serialized key+value pair plus overhead.
const DATA_BUFFER_SIZE: usize = 256; // ADJUST SIZE AS NEEDED

// --- Storage Geometry (Derived from Linker Script and Target) ---
// Page size for STM32L0 is 128 bytes.
// ** Ensure PAGE_COUNT matches the actual size defined in memory.x / PAGE_SIZE **
const PAGE_COUNT: usize = 8; // Example: For 1KB storage region on STM32L0

// --- Cache Configuration ---
/// Number of key slots in the KeyPointerCache.
const CACHE_KEYS: usize = 16; // Example: Cache pointers for up to 16 unique keys

// --- Type Aliases ---
type HalFlash = Flash<'static, Blocking>;
type WrappedFlash = BlockingAsync<HalFlash>;

// --- Internal State ---
struct StorageState {
    flash: WrappedFlash,
    cache: sequential_storage::cache::KeyPointerCache<PAGE_COUNT, sequential_storage::map::Key<128>, CACHE_KEYS>,
    flash_range: Range<u32>,
}

// --- Global Singleton ---
static STORAGE: OnceLock<BlockingMutex<CriticalSectionRawMutex, StorageState>> = OnceLock::new();

// --- Initialization and Setup ---

/// Gets the flash address [`Range`] for storage from the linker. **Internal Function**.
fn flash_range_from_linker() -> Range<u32> {
    extern "C" {
        static __storage_start: u32;
        static __storage_end: u32;
    }
    unsafe {
        let linker_start = &__storage_start as *const u32 as u32;
        let linker_end = &__storage_end as *const u32 as u32;
        let start = linker_start.saturating_sub(FLASH_OFFSET);
        let end = linker_end.saturating_sub(FLASH_OFFSET);
        let size = end.saturating_sub(start);

        assert!(linker_start >= FLASH_OFFSET, "Storage start symbol seems below flash base.");
        assert!(end > start, "Storage range invalid: end address must be greater than start address.");
        assert!(size >= PAGE_SIZE as u32, "Storage range must be at least one page large.");
        assert_eq!(size as usize % PAGE_SIZE, 0, "Storage range size must be a multiple of page size.");
        assert_eq!(start % PAGE_SIZE as u32, 0, "Storage start address must be page-aligned.");
        assert_eq!(end % PAGE_SIZE as u32, 0, "Storage end address must be page-aligned.");
        assert_eq!(size as usize / PAGE_SIZE, PAGE_COUNT, "Calculated page count from linker symbols does not match PAGE_COUNT constant.");

        // Use defmt for logging
        defmt::info!("Storage: Linker symbols: start=0x{:X}, end=0x{:X}", linker_start, linker_end);
        defmt::info!("Storage: Calculated HAL range: start=0x{:X}, end=0x{:X} ({} bytes, {} pages)", start, end, size, PAGE_COUNT);

        start..end
    }
}

/// Initializes the global storage system. **BLOCKING**.
pub fn init(flash: HalFlash) {
    let flash_range = flash_range_from_linker();

    let wrapped_flash = BlockingAsync::new(flash);

    let initial_state = StorageState {
        flash: wrapped_flash,
        cache: sequential_storage::cache::KeyPointerCache::new(),
        flash_range,
    };

    STORAGE.init(BlockingMutex::new(initial_state));
    defmt::info!("Storage: Global instance initialized.");

    match get::<u8>(MARKER_KEY) {
        Ok(Some(val)) if val == MARKER_VALUE => {
            defmt::info!("Storage: Found valid initialization marker (0x{:02X}).", val);
        }
        Ok(Some(val)) => {
            defmt::warn!("Storage: Found invalid marker (0x{:02X}). Erasing storage...", val);
            erase_all().expect("Storage: Failed to erase storage after finding invalid marker");
            defmt::info!("Storage: Erase complete due to invalid marker.");
        }
        Ok(None) => {
            defmt::info!("Storage: No marker found. Assuming new/corrupt storage. Erasing...");
            erase_all().expect("Storage: Failed to erase uninitialized storage");
            defmt::info!("Storage: Erase complete for initial setup.");
        }
        Err(ref e) => {
            defmt::error!("Storage: Error reading marker ({:?}). Erasing storage...", defmt::Debug2Format(e));
            erase_all().expect("Storage: Failed to erase storage after read error");
            defmt::info!("Storage: Erase complete after read error.");
        }
    }
}

/// Gets exclusive access to the underlying storage state. **BLOCKING**.
pub fn lock() -> BlockingMutexGuard<'static, CriticalSectionRawMutex, StorageState> {
    STORAGE.get().expect("Storage must be initialized before locking").lock()
}

// --- Core API Operations (Blocking) ---

/// Stores a key-value pair into flash memory. **BLOCKING**.
pub fn insert<V>(key: &str, value: &V) -> Result<(), Error<FlashError>>
where
    V: Serialize + MaxSize,
{
    const OVERHEAD_ESTIMATE: usize = 64;
    let key_bytes = key.as_bytes();
    let value_max_size = V::POSTCARD_MAX_SIZE;
    let required_buf_size = key_bytes.len() + value_max_size + OVERHEAD_ESTIMATE;
    let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
    if required_buf_size > buffer.len() {
         defmt::error!("Storage insert failed for key '{}': Estimated buffer size {} exceeds allocated buffer {}", key, required_buf_size, buffer.len());
         return Err(Error::BufferTooSmall);
    }

    let serialized_value = postcard::to_slice(value, &mut buffer[..value_max_size])
        .map_err(|e| {
            defmt::error!("Postcard serialization failed for key '{}': {:?}", key, defmt::Debug2Format(&e));
            Error::Serialization
        })?;

    let mut guard = lock();
    let state = &mut *guard;

    let store_future = sequential_storage::map::store_item(
        &mut state.flash,
        state.flash_range.clone(),
        &mut state.cache,
        &mut buffer,
        key_bytes,
        serialized_value,
    );

    block_on(store_future)
}

/// Retrieves a value from flash memory associated with the given key. **BLOCKING**.
pub fn get<V>(key: &str) -> Result<Option<V>, Error<FlashError>>
where
    V: DeserializeOwned,
{
    let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
    let key_bytes = key.as_bytes();

    let mut guard = lock();
    let state = &mut *guard;

    let fetch_future = sequential_storage::map::fetch_item::<[u8], [u8], _>(
        &mut state.flash,
        state.flash_range.clone(),
        &mut state.cache,
        &mut buffer,
        key_bytes,
    );

    match block_on(fetch_future) {
        Ok(Some(serialized_value)) => {
            drop(guard);
            match postcard::from_bytes::<V>(serialized_value) {
                Ok(value) => Ok(Some(value)),
                Err(e) => {
                    defmt::error!("Postcard deserialization failed for key '{}': {:?}", key, defmt::Debug2Format(&e));
                    Err(Error::Deserialization)
                }
            }
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Erases *all* data within the configured flash storage range. **BLOCKING**.
pub fn erase_all() -> Result<(), Error<FlashError>> {
    let mut guard = lock();
    let state = &mut *guard;

    defmt::info!("Storage: Erasing all data in flash range {:?}", state.flash_range);

    let erase_future = state.flash.erase(state.flash_range.start, state.flash_range.end);

    block_on(erase_future).map_err(Error::Storage)?;
    defmt::info!("Storage: Flash erase completed.");

    state.cache = sequential_storage::cache::KeyPointerCache::new();
    defmt::info!("Storage: Cache reset.");

    drop(guard);

    defmt::info!("Storage: Writing initialization marker...");
    insert(MARKER_KEY, &MARKER_VALUE).expect("Storage: Failed to write marker after erase");
    defmt::info!("Storage: Initialization marker written successfully.");

    Ok(())
}

/// Removes a key-value pair from flash. **BLOCKING**. (Currently Disabled)
#[cfg(not(feature = "enable_stm32_remove"))]
pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
    defmt::warn!("Storage: remove() called for key '{}', but is currently disabled for this target due to potential performance/driver limitations.", key);
    Ok(())
}

/*
// Example implementation if `enable_stm32_remove` feature is active:
#[cfg(feature = "enable_stm32_remove")]
pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
    defmt::info!("Storage: Attempting to remove key '{}'...", key);
    let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
    let key_bytes = key.as_bytes();
    let mut guard = lock();
    let state = &mut *guard;
    let remove_future = sequential_storage::map::remove_item::<[u8], _>(
        &mut state.flash,
        state.flash_range.clone(),
        &mut state.cache,
        &mut buffer,
        key_bytes,
    );
    let result = block_on(remove_future);
    if result.is_ok() {
        defmt::info!("Storage: Successfully removed key '{}'.", key);
    } else {
        defmt::error!("Storage: Failed to remove key '{}': {:?}", key, defmt::Debug2Format(&result));
    }
    result
}
*/

// --- User-Defined Data Structures ---
// (Keep these structs as they were)

#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format)]
pub struct Amsg {
    #[serde(with = "defmt::serde")]
    pub id: u16,
    #[serde(with = "defmt::serde")]
    pub interval: u16,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, MaxSize, defmt::Format)]
#[repr(u8)]
pub enum HeatMode { Off = 0, On = 1, Auto = 2, PwrSave = 3, }

#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format)]
pub struct HeaterNvdata {
    #[serde(with = "defmt::serde")]
    pub mode: HeatMode,
    #[serde(with = "defmt::serde")]
    pub hysteresis: u8,
    #[serde(with = "defmt::serde")]
    pub threshold: i16,
}


// --- Specific Configuration Getters/Setters (Blocking API) ---
// (Keep these functions as they were, using internal keys)

const KEY_SNUM: &str = "cfg/snum";
const KEY_NAME: &str = "cfg/name";
const KEY_BAUD: &str = "cfg/baud";
const KEY_AMSG: &str = "cfg/amsg";
const KEY_SMOOTH: &str = "cfg/smooth";
const KEY_SENS_INTERVAL: &str = "cfg/sens_int";
const KEY_CORR_DIST: &str = "cfg/corr_dist";
const KEY_HEAT: &str = "cfg/heat";

pub fn get_serial_number() -> Result<Option<[u8; 5]>, Error<FlashError>> { get::<[u8; 5]>(KEY_SNUM) }
pub fn set_serial_number(snum: &[u8; 5]) -> Result<(), Error<FlashError>> { insert(KEY_SNUM, snum) }
pub fn get_device_name() -> Result<Option<heapless::String<22>>, Error<FlashError>> { get::<heapless::String<22>>(KEY_NAME) }
pub fn set_device_name(name: &heapless::String<22>) -> Result<(), Error<FlashError>> { insert(KEY_NAME, name) }
pub fn get_baud_rate() -> Result<Option<u32>, Error<FlashError>> { get::<u32>(KEY_BAUD) }
pub fn set_baud_rate(baud: u32) -> Result<(), Error<FlashError>> { insert(KEY_BAUD, &baud) }
pub fn get_amsg() -> Result<Option<Amsg>, Error<FlashError>> { get::<Amsg>(KEY_AMSG) }
pub fn set_amsg(amsg: &Amsg) -> Result<(), Error<FlashError>> { insert(KEY_AMSG, amsg) }
pub fn get_smoothing_factor() -> Result<Option<f32>, Error<FlashError>> { get::<f32>(KEY_SMOOTH) }
pub fn set_smoothing_factor(factor: f32) -> Result<(), Error<FlashError>> { insert(KEY_SMOOTH, &factor) }
pub fn get_sensors_interval() -> Result<Option<u8>, Error<FlashError>> { get::<u8>(KEY_SENS_INTERVAL) }
pub fn set_sensors_interval(interval: u8) -> Result<(), Error<FlashError>> { insert(KEY_SENS_INTERVAL, &interval) }
pub fn get_corr_distance() -> Result<Option<f32>, Error<FlashError>> { get::<f32>(KEY_CORR_DIST) }
pub fn set_corr_distance(distance: f32) -> Result<(), Error<FlashError>> { insert(KEY_CORR_DIST, &distance) }
pub fn get_heater_config() -> Result<Option<HeaterNvdata>, Error<FlashError>> { get::<HeaterNvdata>(KEY_HEAT) }
pub fn set_heater_config(heat_cfg: &HeaterNvdata) -> Result<(), Error<FlashError>> { insert(KEY_HEAT, heat_cfg) }