//! src/storage.rs

#![deny(missing_docs)]
//! Provides key-value pair persistent storage on flash, inspired by ariel-os-storage.
//! Uses a blocking API, wrapping async sequential-storage calls internally,
//! suitable for hardware with only blocking flash drivers like STM32L0.
//! Uses `defmt` directly for logging.

use core::ops::{Deref, Range}; // Removed DerefMut
// Use Blocking HAL Flash and its associated Error type and MAX_ERASE_SIZE constant
use embassy_stm32::flash::{Blocking, Error as FlashError, Flash, MAX_ERASE_SIZE};
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex as BlockingMutex}, // Keep Mutex
    once_lock::OnceLock,                                                      // Keep OnceLock
};
// Import the wrapper to make blocking flash compatible with async traits
use embassy_embedded_hal::adapter::BlockingAsync;
// Import the correct blocker for Embassy tasks
use embassy_futures::block_on;
// Traits required by sequential-storage
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use sequential_storage::map::{SerializationError, Value}; // Import traits/types for PostcardValue wrapper
                                                          // Logging directly via defmt
use defmt; // Make defmt macros available
          // Serialization/Deserialization
use postcard::experimental::max_size::MaxSize; // Needs feature "experimental-derive" in Cargo.toml
use serde::{de::DeserializeOwned, Deserialize, Serialize}; // Added DeserializeOwned import
// Fixed-size collections commonly used in embedded
use heapless;

// Re-export the storage Error type for convenience
pub use sequential_storage::Error;

// --- Configuration ---

const FLASH_OFFSET: u32 = 0x0800_0000;
const MARKER_KEY: &str = "__INIT_MARKER";
const MARKER_VALUE: u8 = 0xAA;
const DATA_BUFFER_SIZE: usize = 256;
const PAGE_COUNT: usize = 8;
const CACHE_KEYS: usize = 16;
const CACHE_KEY_BUFFER_SIZE: usize = 64;

// --- Type Aliases ---
type HalFlash = Flash<'static, Blocking>;
type WrappedFlash = BlockingAsync<HalFlash>;
type CacheKeyType = [u8; CACHE_KEY_BUFFER_SIZE];

// --- Internal State ---
struct StorageState {
    flash: WrappedFlash,
    cache: sequential_storage::cache::KeyPointerCache<PAGE_COUNT, CacheKeyType, CACHE_KEYS>,
    flash_range: Range<u32>,
}

// --- Global Singleton ---
static STORAGE: OnceLock<BlockingMutex<CriticalSectionRawMutex, StorageState>> = OnceLock::new();

// --- Helper Function ---
/// Converts a &str key into a fixed-size array, padding with 0s.
/// Returns None if the key is too long for the cache key buffer.
fn pad_key(key: &str) -> Option<CacheKeyType> {
    if key.len() > CACHE_KEY_BUFFER_SIZE {
        None
    } else {
        let mut padded = [0u8; CACHE_KEY_BUFFER_SIZE];
        padded[..key.len()].copy_from_slice(key.as_bytes());
        Some(padded)
    }
}

// --- Postcard Value Wrapper ---

/// A `Value` wrapper serialized using Postcard.
#[derive(Debug)]
struct PostcardValue<T> {
    value: T,
}

impl<'d, T: Serialize + Deserialize<'d>> PostcardValue<T> {
    #[allow(dead_code)]
    pub fn from(value: T) -> Self { Self { value } }
    pub fn into_inner(self) -> T { self.value }
}

impl<'d, T: Serialize + Deserialize<'d>> From<T> for PostcardValue<T> {
    fn from(other: T) -> PostcardValue<T> { PostcardValue::from(other) }
}

impl<T> Deref for PostcardValue<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { &self.value }
}

impl<'d, T: Serialize + Deserialize<'d>> Value<'d> for PostcardValue<T> {
    fn serialize_into(&self, buffer: &mut [u8]) -> Result<usize, SerializationError> {
        postcard::to_slice(&self.value, buffer)
            .map(|used| used.len())
            .map_err(|e| match e {
                postcard::Error::SerializeBufferFull => SerializationError::BufferTooSmall,
                _ => SerializationError::Custom(0),
            })
    }

    fn deserialize_from(buffer: &'d [u8]) -> Result<Self, SerializationError> {
        postcard::from_bytes(buffer)
            .map(|value| Self { value })
            .map_err(|e| match e {
                postcard::Error::DeserializeUnexpectedEnd
                | postcard::Error::DeserializeBadVarint
                | postcard::Error::DeserializeBadBool
                | postcard::Error::DeserializeBadChar
                | postcard::Error::DeserializeBadUtf8
                | postcard::Error::DeserializeBadOption
                | postcard::Error::DeserializeBadEnum
                | postcard::Error::DeserializeBadEncoding => SerializationError::InvalidData,
                _ => SerializationError::Custom(0),
            })
    }
}

// --- Newtype Wrapper for Storable String ---
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorableString<const N: usize>(pub heapless::String<N>);

impl<const N: usize> core::ops::Deref for StorableString<N> {
    type Target = heapless::String<N>;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl<const N: usize> Serialize for StorableString<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer { self.0.serialize(serializer) }
}
impl<'de, const N: usize> Deserialize<'de> for StorableString<N> {
     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: serde::Deserializer<'de> { heapless::String::<N>::deserialize(deserializer).map(StorableString) }
}
impl<const N: usize> postcard::experimental::max_size::MaxSize for StorableString<N> {
    // Safe upper bound: Max varint len (10 for u64/usize) + N bytes for string data
    const POSTCARD_MAX_SIZE: usize = 10 + N;
}
impl<const N: usize> defmt::Format for StorableString<N> {
    fn format(&self, f: defmt::Formatter) { defmt::write!(f, "{}", self.0.as_str()); }
}
impl<const N: usize> Clone for StorableString<N> {
     fn clone(&self) -> Self { StorableString(self.0.clone()) }
}

// --- Initialization and Setup ---

/// Gets the flash address [`Range`] for storage from the linker. **Internal Function**.
fn flash_range_from_linker() -> Range<u32> {
    unsafe extern "C" {
        static __storage_start: u32;
        static __storage_end: u32;
    }
    let linker_start = unsafe { core::ptr::addr_of!(__storage_start).read_volatile() };
    let linker_end = unsafe { core::ptr::addr_of!(__storage_end).read_volatile() };
    let start = linker_start.saturating_sub(FLASH_OFFSET);
    let end = linker_end.saturating_sub(FLASH_OFFSET);
    let size = end.saturating_sub(start);
    assert!(linker_start >= FLASH_OFFSET, "Storage start symbol seems below flash base.");
    assert!(end > start, "Storage range invalid: end address must be greater than start address.");
    assert!(size >= MAX_ERASE_SIZE as u32, "Storage range must be at least MAX_ERASE_SIZE large.");
    assert_eq!(size as usize % MAX_ERASE_SIZE, 0, "Storage range size must be a multiple of MAX_ERASE_SIZE.");
    assert_eq!(start % MAX_ERASE_SIZE as u32, 0, "Storage start address must be MAX_ERASE_SIZE-aligned.");
    assert_eq!(end % MAX_ERASE_SIZE as u32, 0, "Storage end address must be MAX_ERASE_SIZE-aligned.");
    let calculated_pages = size as usize / MAX_ERASE_SIZE;
    assert_eq!(calculated_pages, PAGE_COUNT, "Calculated page count {} from linker symbols (size={}) does not match PAGE_COUNT constant {}", calculated_pages, size, PAGE_COUNT);
    defmt::info!("Storage: Linker symbols: start=0x{:X}, end=0x{:X}", linker_start, linker_end);
    defmt::info!("Storage: Calculated HAL range: start=0x{:X}, end=0x{:X} ({} bytes, {} pages based on MAX_ERASE_SIZE={})", start, end, size, PAGE_COUNT, MAX_ERASE_SIZE);
    start..end
}

/// Initializes the global storage system. **BLOCKING**.
pub fn init(flash: HalFlash) {
    let flash_range = flash_range_from_linker();
    let wrapped_flash = BlockingAsync::new(flash);
    let initial_state = StorageState {
        flash: wrapped_flash,
        cache: sequential_storage::cache::KeyPointerCache::<PAGE_COUNT, CacheKeyType, CACHE_KEYS>::new(),
        flash_range,
    };
    STORAGE.init(BlockingMutex::new(initial_state));
    defmt::info!("Storage: Global instance initialized.");
    match get::<u8>(MARKER_KEY) {
        Ok(Some(val)) if val == MARKER_VALUE => defmt::info!("Storage: Found valid initialization marker (0x{:02X}).", val),
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

// --- Core API Operations (Blocking) ---

/// Stores a key-value pair into flash memory. **BLOCKING**.
pub fn insert<V>(key: &str, value: &V) -> Result<(), Error<FlashError>>
where
    V: Serialize + MaxSize + Clone + DeserializeOwned,
{
    let padded_key =
        pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;

    const OVERHEAD_ESTIMATE: usize = 64;
    let value_max_size = V::POSTCARD_MAX_SIZE;
    let required_buf_size_estimate = CACHE_KEY_BUFFER_SIZE + value_max_size + OVERHEAD_ESTIMATE;

    if required_buf_size_estimate > DATA_BUFFER_SIZE {
        defmt::error!("Storage insert failed for key '{}': Estimated buffer size {} exceeds allocated buffer {}", key, required_buf_size_estimate, DATA_BUFFER_SIZE);
        return Err(Error::BufferTooSmall(required_buf_size_estimate));
    }

    let postcard_value = PostcardValue::from(value.clone());

    let storage_mutex = STORAGE.get().expect("Storage must be initialized before use");

    unsafe {
        storage_mutex.lock_mut(|state| {
            let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
            let store_future = sequential_storage::map::store_item(
                &mut state.flash,
                state.flash_range.clone(),
                &mut state.cache,
                &mut buffer,
                &padded_key,
                &postcard_value,
            );
            block_on(store_future)
        })
    }
}

/// Retrieves a value from flash memory associated with the given key. **BLOCKING**.
pub fn get<V>(key: &str) -> Result<Option<V>, Error<FlashError>>
where
    V: DeserializeOwned + Serialize,
{
    let padded_key =
        pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;

    if key.len() > CACHE_KEY_BUFFER_SIZE {
        // Restore original log message
        defmt::warn!(
            "Storage get warning for key '{}': Key length {} exceeds maximum cache key buffer size {}. Key cannot be in cache.",
            key, key.len(), CACHE_KEY_BUFFER_SIZE
        );
    }

    let storage_mutex = STORAGE.get().expect("Storage must be initialized before use");

    let fetch_result = unsafe {
        storage_mutex.lock_mut(|state| {
            let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
            let fetch_future = sequential_storage::map::fetch_item::<CacheKeyType, PostcardValue<V>, _>(
                &mut state.flash,
                state.flash_range.clone(),
                &mut state.cache,
                &mut buffer,
                &padded_key,
            );
            block_on(fetch_future)
        })
    };

    match fetch_result {
        Ok(Some(fetched_postcard_value)) => {
            Ok(Some(fetched_postcard_value.into_inner()))
        }
        Ok(None) => Ok(None),
        // Use correct tuple variant pattern Error::Corrupted(reason)
        Err(Error::Corrupted(reason)) => {
             defmt::error!("Storage corrupted during fetch for key '{}': {}", key, reason);
             Err(Error::Corrupted(reason)) // Pass the reason along
        }
        // Use correct tuple variant pattern Error::Storage(flash_err)
        Err(e @ Error::Storage(ref flash_err)) => { // Use ref flash_err to avoid moving it
             defmt::error!("Storage error during fetch for key '{}': {:?}", key, defmt::Debug2Format(flash_err));
             Err(e) // Return the original error `e`
        }
        Err(e @ Error::BufferTooSmall(size)) => {
             defmt::error!("Buffer too small (size {}) during fetch for key '{}'", size, key);
             Err(e)
         }
    }
}

/// Erases *all* data within the configured flash storage range. **BLOCKING**.
pub fn erase_all() -> Result<(), Error<FlashError>> {
    let storage_mutex = STORAGE.get().expect("Storage must be initialized before use");

    let erase_result = unsafe {
        storage_mutex.lock_mut(|state| {
            // Restore original log message
            defmt::info!(
                "Storage: Erasing all data in flash range {:?}..{:?}",
                state.flash_range.start,
                state.flash_range.end
            );

            let erase_future = state.flash.erase(state.flash_range.start, state.flash_range.end);
            let result = block_on(erase_future);
            defmt::info!("Storage: Flash erase completed.");

            state.cache = sequential_storage::cache::KeyPointerCache::<PAGE_COUNT, CacheKeyType, CACHE_KEYS>::new();
            defmt::info!("Storage: Cache reset.");

            // Use correct tuple variant syntax Error::Storage(value)
            result.map_err(|flash_err| Error::Storage(flash_err))
        })
    };

    if erase_result.is_ok() {
        defmt::info!("Storage: Writing initialization marker...");
        insert(MARKER_KEY, &MARKER_VALUE).map_err(|e| {
             defmt::error!("Storage: FAILED to write marker after erase: {:?}", defmt::Debug2Format(&e));
             e
        })?;
         defmt::info!("Storage: Initialization marker written successfully.");
    }

    erase_result
}

/// Removes a key-value pair from flash. **BLOCKING**. (Currently Disabled)
pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
     if key.len() > CACHE_KEY_BUFFER_SIZE {
         // Restore original log message
         defmt::warn!(
             "Storage remove called for key '{}' which exceeds cache key buffer size {}. Remove operation may be less efficient.",
             key, CACHE_KEY_BUFFER_SIZE
         );
     }
     defmt::warn!("Storage: remove() called for key '{}', but is currently disabled for this target due to potential performance/driver limitations.", key);
     Ok(())
 }

/*
// Example implementation if `enable_stm32_remove` feature is active:
// Commented out cfg check as feature isn't defined
// #[cfg(feature = "enable_stm32_remove")]
pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
    let padded_key = pad_key(key)
        .ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?; // Key too long

    let storage_mutex = STORAGE.get().expect("Storage must be initialized before use");

    unsafe {
        storage_mutex.lock_mut(|state| {
            defmt::info!("Storage: Attempting to remove key '{}'...", key);
            let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];

            let remove_future = sequential_storage::map::remove_item::<CacheKeyType, _>(
                &mut state.flash,
                state.flash_range.clone(),
                &mut state.cache,
                &mut buffer,
                &padded_key,
            );
            let result = block_on(remove_future);
            if result.is_ok() {
                defmt::info!("Storage: Successfully removed key '{}'.", key);
            } else {
                defmt::error!("Storage: Failed to remove key '{}': {:?}", key, defmt::Debug2Format(&result));
            }
            result
        }) // Returns Result<(), Error<FlashError>>
    } // End unsafe lock_mut closure
}
*/


// --- User-Defined Data Structures ---

#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format, Clone)]
pub struct Amsg {
    pub id: u16,
    pub interval: u16,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, MaxSize, defmt::Format)]
#[repr(u8)]
pub enum HeatMode { Off = 0, On = 1, Auto = 2, PwrSave = 3, }

#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format, Clone)]
pub struct HeaterNvdata {
    pub mode: HeatMode,
    pub hysteresis: u8,
    pub threshold: i16,
}

// --- Specific Configuration Getters/Setters (Blocking API) ---

const KEY_SNUM: &str = "cfg/snum";
const KEY_NAME: &str = "cfg/name";
const KEY_BAUD: &str = "cfg/baud";
const KEY_AMSG: &str = "cfg/amsg";
const KEY_SMOOTH: &str = "cfg/smooth";
const KEY_SENS_INTERVAL: &str = "cfg/sens_int";
const KEY_CORR_DIST: &str = "cfg/corr_dist";
const KEY_HEAT: &str = "cfg/heat";

// Functions remain the same, using the new internal API structure
pub fn get_serial_number() -> Result<Option<[u8; 5]>, Error<FlashError>> { get::<[u8; 5]>(KEY_SNUM) }
pub fn set_serial_number(snum: &[u8; 5]) -> Result<(), Error<FlashError>> { insert(KEY_SNUM, snum) }
pub fn get_device_name() -> Result<Option<StorableString<22>>, Error<FlashError>> { get::<StorableString<22>>(KEY_NAME) }
pub fn set_device_name(name: &StorableString<22>) -> Result<(), Error<FlashError>> { insert(KEY_NAME, name) }
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