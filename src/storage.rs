//! src/storage.rs

#![deny(missing_docs)]
//! Provides key-value pair persistent storage on flash, inspired by ariel-os-storage.
//! Uses a blocking API, wrapping async sequential-storage calls internally,
//! suitable for hardware with only blocking flash drivers like STM32L0.
//! Uses `defmt` directly for logging.

use core::ops::{Deref, Range};
use embassy_stm32::flash::{Blocking, Error as FlashError, Flash, MAX_ERASE_SIZE};
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex as BlockingMutex}, // Use Mutex directly
    once_lock::OnceLock,
};
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::block_on;
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use sequential_storage::map::{SerializationError, Value};
use defmt;
use postcard::experimental::max_size::MaxSize;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
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
#[derive(Debug)]
struct PostcardValue<T> { value: T }
impl<'d, T: Serialize + Deserialize<'d>> PostcardValue<T> {
    #[allow(dead_code)] pub fn from(value: T) -> Self { Self { value } }
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
        postcard::to_slice(&self.value, buffer).map(|used| used.len()).map_err(|e| match e {
            postcard::Error::SerializeBufferFull => SerializationError::BufferTooSmall,
            _ => SerializationError::Custom(0),
        })
    }
    fn deserialize_from(buffer: &'d [u8]) -> Result<Self, SerializationError> {
        postcard::from_bytes(buffer).map(|value| Self { value }).map_err(|e| match e {
            postcard::Error::DeserializeUnexpectedEnd | postcard::Error::DeserializeBadVarint |
            postcard::Error::DeserializeBadBool | postcard::Error::DeserializeBadChar |
            postcard::Error::DeserializeBadUtf8 | postcard::Error::DeserializeBadOption |
            postcard::Error::DeserializeBadEnum | postcard::Error::DeserializeBadEncoding => SerializationError::InvalidData,
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
    const POSTCARD_MAX_SIZE: usize = 10 + N; // Corrected calculation
}
impl<const N: usize> defmt::Format for StorableString<N> {
    fn format(&self, f: defmt::Formatter) { defmt::write!(f, "{}", self.0.as_str()); }
}
impl<const N: usize> Clone for StorableString<N> {
     fn clone(&self) -> Self { StorableString(self.0.clone()) }
}

// --- Initialization and Setup ---
fn flash_range_from_linker() -> Range<u32> {
    unsafe extern "C" { static __storage_start: u32; static __storage_end: u32; }
    let linker_start = unsafe { core::ptr::addr_of!(__storage_start).read_volatile() };
    let linker_end = unsafe { core::ptr::addr_of!(__storage_end).read_volatile() };
    let start = linker_start.saturating_sub(FLASH_OFFSET);
    let end = linker_end.saturating_sub(FLASH_OFFSET);
    let size = end.saturating_sub(start);
    assert!(linker_start >= FLASH_OFFSET, "Storage start symbol seems below flash base.");
    assert!(end > start, "Storage range invalid: end > start.");
    assert!(size >= MAX_ERASE_SIZE as u32, "Storage range must be >= MAX_ERASE_SIZE.");
    assert_eq!(size as usize % MAX_ERASE_SIZE, 0, "Storage size must be multiple of MAX_ERASE_SIZE.");
    assert_eq!(start % MAX_ERASE_SIZE as u32, 0, "Storage start must be MAX_ERASE_SIZE-aligned.");
    assert_eq!(end % MAX_ERASE_SIZE as u32, 0, "Storage end must be MAX_ERASE_SIZE-aligned.");
    let calculated_pages = size as usize / MAX_ERASE_SIZE;
    assert_eq!(calculated_pages, PAGE_COUNT, "Calculated pages {} != PAGE_COUNT {}", calculated_pages, PAGE_COUNT);
    defmt::info!("Storage: Linker symbols: start=0x{:X}, end=0x{:X}", linker_start, linker_end);
    defmt::info!("Storage: Calculated HAL range: start=0x{:X}, end=0x{:X} ({} bytes, {} pages based on MAX_ERASE_SIZE={})", start, end, size, PAGE_COUNT, MAX_ERASE_SIZE);
    start..end
}

pub fn init(flash: HalFlash) {
    let flash_range = flash_range_from_linker();
    let wrapped_flash = BlockingAsync::new(flash);
    let initial_state = StorageState {
        flash: wrapped_flash,
        cache: sequential_storage::cache::KeyPointerCache::<PAGE_COUNT, CacheKeyType, CACHE_KEYS>::new(),
        flash_range,
    };
    STORAGE.init(BlockingMutex::new(initial_state)); // Initialize the OnceLock
    defmt::info!("Storage: Global instance initialized.");
    // Check marker *after* init ensures STORAGE.try_get() will succeed
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

    // Use try_get() which returns Option<&Mutex...>
    let storage_mutex = STORAGE.try_get()
        .expect("Storage must be initialized before use");

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

pub fn get<V>(key: &str) -> Result<Option<V>, Error<FlashError>>
where
    V: DeserializeOwned + Serialize,
{
    let padded_key =
        pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;

    if key.len() > CACHE_KEY_BUFFER_SIZE {
        defmt::warn!( // Restored message
            "Storage get warning for key '{}': Key length {} exceeds maximum cache key buffer size {}. Key cannot be in cache.",
            key, key.len(), CACHE_KEY_BUFFER_SIZE
        );
    }

    // Use try_get() which returns Option<&Mutex...>
    let storage_mutex = STORAGE.try_get()
        .expect("Storage must be initialized before use");

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
        // Use struct variant pattern Error::Corrupted {}
        Err(Error::Corrupted {}) => {
             defmt::error!("Storage corrupted during fetch for key '{}'", key);
             Err(Error::Corrupted {}) // Construct struct variant
        }
        // Use struct variant pattern Error::Storage { value: flash_err }
        Err(e @ Error::Storage { value: ref flash_err }) => {
             defmt::error!("Storage error during fetch for key '{}': {:?}", key, defmt::Debug2Format(flash_err));
             Err(e)
        }
        // BufferTooSmall is a tuple variant
        Err(e @ Error::BufferTooSmall(size)) => {
             defmt::error!("Buffer too small (size {}) during fetch for key '{}'", size, key);
             Err(e)
         }
         // Add cases for other variants if needed
         Err(e @ Error::FullStorage) => {
             defmt::error!("Storage full during fetch for key '{}'", key);
             Err(e)
         }
         Err(e @ Error::ItemTooBig) => {
             defmt::error!("Item too big during fetch for key '{}'", key);
             Err(e)
         }
         Err(e @ Error::SerializationError(_)) => {
             defmt::error!("Unexpected serialization error during fetch for key '{}'", key);
             Err(Error::Corrupted {}) // Map to Corrupted
         }
         // Note: sequential-storage Error is non-exhaustive
         #[allow(unreachable_patterns)] // May become reachable if Error changes
         Err(_) => {
            defmt::error!("Unknown storage error during fetch for key '{}'", key);
            Err(Error::Corrupted {}) // Map unknown to Corrupted
         }
    }
}

pub fn erase_all() -> Result<(), Error<FlashError>> {
    // Use try_get() which returns Option<&Mutex...>
    let storage_mutex = STORAGE.try_get()
        .expect("Storage must be initialized before use");

    let erase_result = unsafe {
        storage_mutex.lock_mut(|state| {
            // Restore message
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

            // Use struct variant syntax Error::Storage { value: ... }
            result.map_err(|flash_err| Error::Storage { value: flash_err })
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

pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
     if key.len() > CACHE_KEY_BUFFER_SIZE {
        // Restore message
         defmt::warn!(
             "Storage remove called for key '{}' which exceeds cache key buffer size {}. Remove operation may be less efficient.",
             key, CACHE_KEY_BUFFER_SIZE
         );
     }
     defmt::warn!("Storage: remove() called for key '{}', but is currently disabled for this target due to potential performance/driver limitations.", key);
     Ok(())
 }

/*
// remove() implementation if enabled (uses try_get() and correct error syntax)
pub fn remove(key: &str) -> Result<(), Error<FlashError>> {
    let padded_key = pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;
    let storage_mutex = STORAGE.try_get().expect("Storage must be initialized before use");
    unsafe {
        storage_mutex.lock_mut(|state| {
            defmt::info!("Storage: Attempting to remove key '{}'...", key);
            let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
            let remove_future = sequential_storage::map::remove_item::<CacheKeyType, _>(
                &mut state.flash, state.flash_range.clone(), &mut state.cache, &mut buffer, &padded_key,
            );
            let result = block_on(remove_future);
            if result.is_ok() { defmt::info!("Storage: Successfully removed key '{}'.", key); }
            else { defmt::error!("Storage: Failed to remove key '{}': {:?}", key, defmt::Debug2Format(&result)); }
            result
        })
    }
}
*/


// --- User-Defined Data Structures ---
#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format, Clone)]
pub struct Amsg { pub id: u16, pub interval: u16 }
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, MaxSize, defmt::Format)]
#[repr(u8)]
pub enum HeatMode { Off = 0, On = 1, Auto = 2, PwrSave = 3 }
#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, defmt::Format, Clone)]
pub struct HeaterNvdata { pub mode: HeatMode, pub hysteresis: u8, pub threshold: i16 }

// --- Specific Configuration Getters/Setters ---
const KEY_SNUM: &str = "cfg/snum";
const KEY_NAME: &str = "cfg/name";
const KEY_BAUD: &str = "cfg/baud";
const KEY_AMSG: &str = "cfg/amsg";
const KEY_SMOOTH: &str = "cfg/smooth";
const KEY_SENS_INTERVAL: &str = "cfg/sens_int";
const KEY_CORR_DIST: &str = "cfg/corr_dist";
const KEY_HEAT: &str = "cfg/heat";

// Functions remain the same, using the corrected internal API
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