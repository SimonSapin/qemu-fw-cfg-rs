//! A Rust library for reading fw_cfg from QEMU.
//!
//! # Supported architectures
//!
//! As of today, this crate only supported x86 and x86_64. However, it is possible
//! to add support for other platforms, such as ARM.
//!
//! # Examples
//! ```
//! use qemu_fw_cfg::FwCfg;
//!
//! // Verify that we are inside QEMU.
//! if running_in_qemu() {
//!     // Create a new `FwCfg` instance.
//!     let fw_cfg = unsafe { FwCfg::new().unwrap() };
//!     // Retrieve information of a file.
//!     let file = fw_cfg.find_file("etc/igd-opregion").unwrap();
//!     // Read data from the file.
//!     let data = fw_cfg.read_file(&file);
//! }
//! ```

#![no_std]

#[cfg(feature = "alloc")]
#[macro_use]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use core::fmt;
use core::mem::size_of;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[path = "x86.rs"]
mod arch;

mod selector_keys {
    pub const SIGNATURE: u16 = 0x0000;
    pub const DIR: u16 = 0x0019;
}

const SIGNATURE_DATA: &[u8] = b"QEMU";

/// An enum type for [`FwCfg`] errors.
#[derive(Debug)]
#[non_exhaustive]
pub enum FwCfgError {
    /// Invalid signature returned from QEMU fw_cfg I/O port
    InvalidSignature,
}

/// A struct for accessing QEMU fw_cfg.
#[derive(Debug)]
pub struct FwCfg(Mode);

#[derive(Debug)]
enum Mode {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    IOPort,
    MemoryMapped(MemoryMappedDevice),
}

impl FwCfg {
    /// Build `FwCfg` for the x86/x86-64 I/O port.
    ///
    /// # Safety
    ///
    /// This may only be called when running inside QEMU
    /// since I/O ports are accessed without additional checks.
    ///
    /// Only one `FwCfg` value may exist at the same time
    /// since it accesses a global shared stateful resource.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    pub unsafe fn new_for_x86() -> Result<FwCfg, FwCfgError> {
        Self::new_for_mode(Mode::IOPort)
    }

    /// Build `FwCfg` for the device memory-mapped at the give base pointer.
    ///
    /// # Safety
    ///
    /// The pointer must point to a valid fw_cfg device.
    ///
    /// Only one `FwCfg` value may exist at the same time for that pointer.
    pub unsafe fn new_memory_mapped(base_ptr: *mut ()) -> Result<FwCfg, FwCfgError> {
        let device = MemoryMappedDevice::new(base_ptr);
        Self::new_for_mode(Mode::MemoryMapped(device))
    }

    unsafe fn new_for_mode(mode: Mode) -> Result<FwCfg, FwCfgError> {
        let mut fw_cfg = FwCfg(mode);

        let mut signature = [0u8; SIGNATURE_DATA.len()];
        fw_cfg.select(selector_keys::SIGNATURE);
        fw_cfg.read(&mut signature);

        if signature != SIGNATURE_DATA {
            return Err(FwCfgError::InvalidSignature);
        }

        Ok(fw_cfg)
    }

    /// Return an iterator of all files in the fw_cfg directory
    pub fn iter_files(&mut self) -> impl Iterator<Item = FwCfgFile> + '_ {
        self.select(selector_keys::DIR);

        let count = {
            let mut buf = [0u8; size_of::<u32>()];
            self.read(&mut buf);
            u32::from_be_bytes(buf)
        };
        (0..count).map(move |_| {
            let mut file = FwCfgFile::default();
            self.read(file.as_mut_bytes());
            file
        })
    }

    /// Find one or more files by their name.
    ///
    /// Each tuple in `entries` must consisted of file name and a space for
    /// `Option<FwCfgFile>`. If a file is found, the result will be stored by
    /// replacing the value in `Option<FwCfgFile>` of the corresponding tuple,
    /// otherwise it will retained the same value as before.
    ///
    /// # Examples
    /// ```
    /// use qemu_fw_cfg::FwCfg;
    ///
    /// let fw_cfg = unsafe { FwCfg::new().unwrap() };
    /// let mut files = [
    ///     ("etc/igd-opregion", None),
    ///     ("opt/another/file.txt", None),
    /// ];
    /// fw_cfg.find_files(&mut files);
    /// ```
    pub fn find_files(&mut self, entries: &mut [(&str, Option<FwCfgFile>)]) {
        for file in self.iter_files() {
            let mut changed = false;

            for (name, ret) in entries.iter_mut() {
                if file.name() == *name {
                    *ret = Some(file.clone());
                    changed = true;
                }
            }

            if changed && entries.iter().all(|entry| entry.1.is_some()) {
                return;
            }
        }
    }

    /// Find a single file by its name. Returns `None` if the file is missing.
    ///
    /// # Examples
    /// ```
    /// use qemu_fw_cfg::FwCfg;
    ///
    /// let fw_cfg = unsafe { FwCfg::new().unwrap() };
    /// let file = fw_cfg.find_file("etc/igd-opregion").unwrap();
    /// ```
    pub fn find_file(&mut self, name: &str) -> Option<FwCfgFile> {
        let mut entries = [(name, None)];
        self.find_files(&mut entries);
        entries[0].1.take()
    }

    /// Read a file and fill its data in `buffer`.
    ///
    /// If the size of `buffer` is greater or equals to the size of the file,
    /// then it will fill the entire data in `buffer[0..file.size()]`, otherwise
    /// it will only fill up to `buffer.len()`.
    pub fn read_file_to_buffer(&mut self, file: &FwCfgFile, buffer: &mut [u8]) {
        let len = file.size().min(buffer.len());
        self.select(file.key());
        self.read(&mut buffer[..len]);
    }

    /// Read a file and return the data in `Vec<u8>`.
    #[cfg(feature = "alloc")]
    pub fn read_file(&mut self, file: &FwCfgFile) -> Vec<u8> {
        let mut buf = vec![0u8; file.size()];
        self.select(file.key());
        self.read(&mut buf);
        buf
    }

    fn select(&mut self, key: u16) {
        match &mut self.0 {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Mode::IOPort => unsafe { arch::write_selector(key) },
            Mode::MemoryMapped(device) => device.write_selector(key),
        }
    }

    fn read(&mut self, buffer: &mut [u8]) {
        match &mut self.0 {
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            Mode::IOPort => unsafe { arch::read_data(buffer) },
            Mode::MemoryMapped(device) => device.read_data(buffer),
        }
    }
}

const _: () = assert!(size_of::<FwCfgFile>() == 64);

/// A struct that contains information of a fw_cfg file.
#[derive(Clone, PartialEq, Eq)]
// NOTE: The memory layout of this struct must match this exactly:
// https://gitlab.com/qemu-project/qemu/-/blob/v7.0.0/docs/specs/fw_cfg.txt#L132-137
#[repr(C)]
pub struct FwCfgFile {
    size_be: u32,
    key_be: u16,
    _reserved: u16,
    name_bytes: [u8; 56],
}

// Can’t be derived because of:
// https://github.com/rust-lang/rust/issues/88744
// https://github.com/rust-lang/rust/issues/61415
impl Default for FwCfgFile {
    fn default() -> Self {
        Self {
            size_be: 0,
            key_be: 0,
            _reserved: 0,
            name_bytes: [0; 56],
        }
    }
}

impl FwCfgFile {
    /// The size of this file.
    pub fn size(&self) -> usize {
        u32::from_be(self.size_be) as usize
    }

    fn key(&self) -> u16 {
        u16::from_be(self.key_be)
    }

    /// The name of this file.
    pub fn name(&self) -> &str {
        let bytes = self.name_bytes.split(|&b| b == b'\x00').next().unwrap();
        core::str::from_utf8(bytes).unwrap()
    }

    fn as_mut_bytes(&mut self) -> &mut [u8; size_of::<Self>()] {
        let ptr: *mut Self = self;
        let ptr: *mut [u8; size_of::<Self>()] = ptr.cast();
        unsafe { &mut *ptr }
    }
}

impl fmt::Debug for FwCfgFile {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("FwCfgFile")
            .field("key", &self.key())
            .field("size", &self.size())
            .field("name", &self.name())
            .finish()
    }
}

#[derive(Debug)]
struct MemoryMappedDevice {
    base_ptr: *mut (),
}

impl MemoryMappedDevice {
    unsafe fn new(base_ptr: *mut ()) -> Self {
        Self { base_ptr }
    }

    fn register<T>(&self, offset_in_bytes: usize) -> *mut T {
        let offset = offset_in_bytes / size_of::<T>();
        unsafe { self.base_ptr.cast::<T>().add(offset) }
    }

    fn write_selector(&mut self, key: u16) {
        // https://gitlab.com/qemu-project/qemu/-/blob/v7.0.0/docs/specs/fw_cfg.txt#L87
        let selector_offset = 8;
        let selector_ptr = self.register::<u16>(selector_offset);
        unsafe { selector_ptr.write_volatile(key.to_be()) }
    }

    fn read_data(&mut self, data: &mut [u8]) {
        // https://gitlab.com/qemu-project/qemu/-/blob/v7.0.0/docs/specs/fw_cfg.txt#L88
        let data_offset = 0;
        let data_ptr = self.register::<usize>(data_offset);
        for chunk in data.chunks_mut(size_of::<usize>()) {
            let word = unsafe { data_ptr.read_volatile() };
            // https://gitlab.com/qemu-project/qemu/-/blob/v7.0.0/docs/specs/fw_cfg.txt#L53
            // "string-preserving" means native-endian
            let bytes = word.to_ne_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}
