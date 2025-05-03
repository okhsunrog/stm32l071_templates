//! src/storage.rs

#![deny(missing_docs)]
#![allow(clippy::type_complexity)]
#![allow(dead_code)]

//! Provides key-value pair persistent storage on flash (optimized version).

use core::ops::{Deref, Range};
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::block_on;
use embassy_stm32::flash::{Blocking, Error as FlashError, Flash}; // MAX_ERASE_SIZE removed
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex},
    once_lock::OnceLock,
};
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use sequential_storage::{map::{SerializationError, Value}, cache::KeyPointerCache};
use postcard;
use defmt;
use heapless;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
    cache: KeyPointerCache<PAGE_COUNT, CacheKeyType, CACHE_KEYS>,
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
struct PostcardValue<T> {
    value: T,
}
impl<'d, T: Serialize + Deserialize<'d>> PostcardValue<T> {
    #[allow(dead_code)]
    pub fn from(value: T) -> Self {
        Self { value }
    }
    pub fn into_inner(self) -> T {
        self.value
    }
}
impl<'d, T: Serialize + Deserialize<'d>> From<T> for PostcardValue<T> {
    fn from(other: T) -> PostcardValue<T> {
        PostcardValue::from(other)
    }
}
impl<T> Deref for PostcardValue<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}
impl<'d, T: Serialize + Deserialize<'d>> Value<'d> for PostcardValue<T> {
    fn serialize_into(&self, buffer: &mut [u8]) -> Result<usize, SerializationError> {
        postcard::to_slice(&self.value, buffer)
            .map(|used| used.len())
            .map_err(|_e| {
                SerializationError::InvalidData
            })
    }
    fn deserialize_from(buffer: &'d [u8]) -> Result<Self, SerializationError> {
        postcard::from_bytes(buffer)
            .map(|value| Self { value })
            .map_err(|_e| {
                SerializationError::InvalidData
            })
    }
}

// --- Newtype Wrapper for Storable String ---
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorableString<const N: usize>(pub heapless::String<N>);
impl<const N: usize> core::ops::Deref for StorableString<N> {
    type Target = heapless::String<N>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<const N: usize> Serialize for StorableString<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}
impl<'de, const N: usize> Deserialize<'de> for StorableString<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        heapless::String::<N>::deserialize(deserializer).map(StorableString)
    }
}
impl<const N: usize> postcard::experimental::max_size::MaxSize for StorableString<N> {
    const POSTCARD_MAX_SIZE: usize = 10 + N;
}
// Add defmt::Format back
impl<const N: usize> defmt::Format for StorableString<N> {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(f, "{}", self.0.as_str());
    }
}
impl<const N: usize> Clone for StorableString<N> {
    fn clone(&self) -> Self {
        StorableString(self.0.clone())
    }
}

// --- Initialization and Setup ---
fn flash_range_from_linker() -> Range<u32> {
    unsafe extern "C" {
        static __storage_start: u32;
        static __storage_end: u32;
    }
    let linker_start = unsafe { core::ptr::addr_of!(__storage_start).read_volatile() };
    let linker_end = unsafe { core::ptr::addr_of!(__storage_end).read_volatile() };
    let start = linker_start.saturating_sub(FLASH_OFFSET);
    let end = linker_end.saturating_sub(FLASH_OFFSET);
    // Assumes flash range from linker is correct and aligned (no asserts)
    start..end
}

pub fn init(flash: HalFlash) -> Result<(), Error<FlashError>> {
    let flash_range = flash_range_from_linker();
    let wrapped_flash = BlockingAsync::new(flash);
    let initial_state = StorageState {
        flash: wrapped_flash,
        cache:
            KeyPointerCache::<PAGE_COUNT, CacheKeyType, CACHE_KEYS>::new(
            ),
        flash_range,
    };
    let _ = STORAGE.init(BlockingMutex::new(initial_state));

    match get::<u8>(MARKER_KEY) {
        Ok(Some(val)) if val == MARKER_VALUE => Ok(()),
        Ok(Some(_)) | Ok(None) | Err(_) => {
            erase_all()?;
            Ok(())
        }
    }
}

// --- Core API Operations (Blocking) ---
pub fn insert<V>(key: &str, value: &V) -> Result<(), Error<FlashError>>
where
    V: Serialize + MaxSize + Clone + DeserializeOwned,
{
    let padded_key = pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;

    const OVERHEAD_ESTIMATE: usize = 64;
    let value_max_size = V::POSTCARD_MAX_SIZE;
    let required_buf_size_estimate = CACHE_KEY_BUFFER_SIZE + value_max_size + OVERHEAD_ESTIMATE;

    if required_buf_size_estimate > DATA_BUFFER_SIZE {
        return Err(Error::BufferTooSmall(required_buf_size_estimate));
    }

    let postcard_value = PostcardValue::from(value.clone());

    let storage_mutex = STORAGE
        .try_get().unwrap();

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
    let padded_key = pad_key(key).ok_or_else(|| Error::BufferTooSmall(CACHE_KEY_BUFFER_SIZE))?;

    let storage_mutex = STORAGE.try_get().unwrap();

    let fetch_result = unsafe {
        storage_mutex.lock_mut(|state| {
            let mut buffer: [u8; DATA_BUFFER_SIZE] = [0; DATA_BUFFER_SIZE];
            let fetch_future =
                sequential_storage::map::fetch_item::<CacheKeyType, PostcardValue<V>, _>(
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
        Ok(Some(fetched_postcard_value)) => Ok(Some(fetched_postcard_value.into_inner())),
        Ok(None) => Ok(None),
        Err(Error::Corrupted {}) => Err(Error::Corrupted {}),
        Err(Error::Storage { value: flash_err }) => Err(Error::Storage { value: flash_err }),
        Err(e @ Error::BufferTooSmall(_)) => Err(e),
        Err(e @ Error::FullStorage) => Err(e),
        Err(e @ Error::ItemTooBig) => Err(e),
        Err(Error::SerializationError(_)) => Err(Error::Corrupted {}),
        // Add catch-all for non-exhaustive enum
        Err(_) => Err(Error::Corrupted {}),
    }
}

pub fn erase_all() -> Result<(), Error<FlashError>> {
    let storage_mutex = STORAGE
        .try_get().unwrap();

    unsafe {
        storage_mutex.lock_mut(|state| {
            let erase_future = state
                .flash
                .erase(state.flash_range.start, state.flash_range.end);
            let result = block_on(erase_future);
            state.cache = KeyPointerCache::<
                PAGE_COUNT,
                CacheKeyType,
                CACHE_KEYS,
            >::new();
            result.map_err(|flash_err| Error::Storage { value: flash_err })
        })
    }?;

    insert(MARKER_KEY, &MARKER_VALUE)?;
    Ok(())
}

// --- `remove` function is entirely removed ---

// --- User-Defined Data Structures ---
// Add defmt::Format back
#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, Clone, defmt::Format)]
pub struct Amsg {
    pub id: u16,
    pub interval: u16,
}

// Add defmt::Format back
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, MaxSize, defmt::Format)]
#[repr(u8)]
pub enum HeatMode {
    Off = 0,
    On = 1,
    Auto = 2,
    PwrSave = 3,
}

// Add defmt::Format back
#[derive(Serialize, Deserialize, Debug, PartialEq, MaxSize, Clone, defmt::Format)]
pub struct HeaterNvdata {
    pub mode: HeatMode,
    pub hysteresis: u8,
    pub threshold: i16,
}

// --- Specific Configuration Getters/Setters ---
// (No changes here, they use the corrected core API)
const KEY_SNUM: &str = "cfg/snum";
const KEY_NAME: &str = "cfg/name";
const KEY_BAUD: &str = "cfg/baud";
const KEY_AMSG: &str = "cfg/amsg";
const KEY_SMOOTH: &str = "cfg/smooth";
const KEY_SENS_INTERVAL: &str = "cfg/sens_int";
const KEY_CORR_DIST: &str = "cfg/corr_dist";
const KEY_HEAT: &str = "cfg/heat";

pub fn get_serial_number() -> Result<Option<[u8; 5]>, Error<FlashError>> {
    get::<[u8; 5]>(KEY_SNUM)
}
pub fn set_serial_number(snum: &[u8; 5]) -> Result<(), Error<FlashError>> {
    insert(KEY_SNUM, snum)
}
pub fn get_device_name() -> Result<Option<StorableString<22>>, Error<FlashError>> {
    get::<StorableString<22>>(KEY_NAME)
}
pub fn set_device_name(name: &StorableString<22>) -> Result<(), Error<FlashError>> {
    insert(KEY_NAME, name)
}
pub fn get_baud_rate() -> Result<Option<u32>, Error<FlashError>> {
    get::<u32>(KEY_BAUD)
}
pub fn set_baud_rate(baud: u32) -> Result<(), Error<FlashError>> {
    insert(KEY_BAUD, &baud)
}
pub fn get_amsg() -> Result<Option<Amsg>, Error<FlashError>> {
    get::<Amsg>(KEY_AMSG)
}
pub fn set_amsg(amsg: &Amsg) -> Result<(), Error<FlashError>> {
    insert(KEY_AMSG, amsg)
}
pub fn get_smoothing_factor() -> Result<Option<f32>, Error<FlashError>> {
    get::<f32>(KEY_SMOOTH)
}
pub fn set_smoothing_factor(factor: f32) -> Result<(), Error<FlashError>> {
    insert(KEY_SMOOTH, &factor)
}
pub fn get_sensors_interval() -> Result<Option<u8>, Error<FlashError>> {
    get::<u8>(KEY_SENS_INTERVAL)
}
pub fn set_sensors_interval(interval: u8) -> Result<(), Error<FlashError>> {
    insert(KEY_SENS_INTERVAL, &interval)
}
pub fn get_corr_distance() -> Result<Option<f32>, Error<FlashError>> {
    get::<f32>(KEY_CORR_DIST)
}
pub fn set_corr_distance(distance: f32) -> Result<(), Error<FlashError>> {
    insert(KEY_CORR_DIST, &distance)
}
pub fn get_heater_config() -> Result<Option<HeaterNvdata>, Error<FlashError>> {
    get::<HeaterNvdata>(KEY_HEAT)
}
pub fn set_heater_config(heat_cfg: &HeaterNvdata) -> Result<(), Error<FlashError>> {
    insert(KEY_HEAT, heat_cfg)
}
