//! Raw bindings to the Mach, libproc, IOKit and CoreFoundation APIs that the
//! sampler needs. Kept deliberately small: only what Tessera calls.

#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};

pub type kern_return_t = c_int;
pub type mach_port_t = c_uint;
pub type natural_t = c_uint;
pub type integer_t = c_int;
pub type mach_msg_type_number_t = natural_t;
pub type vm_size_t = usize;
pub type vm_address_t = usize;
pub type io_object_t = mach_port_t;
pub type io_iterator_t = io_object_t;
pub type io_registry_entry_t = io_object_t;

pub const KERN_SUCCESS: kern_return_t = 0;

pub const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
pub const CPU_STATE_MAX: usize = 4;
pub const CPU_STATE_USER: usize = 0;
pub const CPU_STATE_SYSTEM: usize = 1;
pub const CPU_STATE_IDLE: usize = 2;
pub const CPU_STATE_NICE: usize = 3;

pub const HOST_VM_INFO64: c_int = 4;

#[repr(C, align(8))]
#[derive(Default, Clone, Copy, Debug)]
pub struct vm_statistics64 {
    pub free_count: natural_t,
    pub active_count: natural_t,
    pub inactive_count: natural_t,
    pub wire_count: natural_t,
    pub zero_fill_count: u64,
    pub reactivations: u64,
    pub pageins: u64,
    pub pageouts: u64,
    pub faults: u64,
    pub cow_faults: u64,
    pub lookups: u64,
    pub hits: u64,
    pub purges: u64,
    pub purgeable_count: natural_t,
    pub speculative_count: natural_t,
    pub decompressions: u64,
    pub compressions: u64,
    pub swapins: u64,
    pub swapouts: u64,
    pub compressor_page_count: natural_t,
    pub throttled_count: natural_t,
    pub external_page_count: natural_t,
    pub internal_page_count: natural_t,
    pub total_uncompressed_pages_in_compressor: u64,
}

pub const HOST_VM_INFO64_COUNT: mach_msg_type_number_t =
    (size_of::<vm_statistics64>() / size_of::<integer_t>()) as mach_msg_type_number_t;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct mach_timebase_info_data_t {
    pub numer: u32,
    pub denom: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct xsw_usage {
    pub xsu_total: u64,
    pub xsu_avail: u64,
    pub xsu_used: u64,
    pub xsu_pagesize: u32,
    pub xsu_encrypted: c_int,
}

pub const RUSAGE_INFO_V2: c_int = 2;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct rusage_info_v2 {
    pub ri_uuid: [u8; 16],
    pub ri_user_time: u64,
    pub ri_system_time: u64,
    pub ri_pkg_idle_wkups: u64,
    pub ri_interrupt_wkups: u64,
    pub ri_pageins: u64,
    pub ri_wired_size: u64,
    pub ri_resident_size: u64,
    pub ri_phys_footprint: u64,
    pub ri_proc_start_abstime: u64,
    pub ri_proc_exit_abstime: u64,
    pub ri_child_user_time: u64,
    pub ri_child_system_time: u64,
    pub ri_child_pkg_idle_wkups: u64,
    pub ri_child_interrupt_wkups: u64,
    pub ri_child_pageins: u64,
    pub ri_child_elapsed_abstime: u64,
    pub ri_diskio_bytesread: u64,
    pub ri_diskio_byteswritten: u64,
}

pub const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

unsafe extern "C" {
    pub static mach_task_self_: mach_port_t;
    pub fn mach_host_self() -> mach_port_t;
    pub fn host_processor_info(
        host: mach_port_t,
        flavor: c_int,
        out_processor_count: *mut natural_t,
        out_processor_info: *mut *mut integer_t,
        out_processor_info_cnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    pub fn host_statistics64(
        host: mach_port_t,
        flavor: c_int,
        host_info_out: *mut integer_t,
        host_info_out_cnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    pub fn host_page_size(host: mach_port_t, out_page_size: *mut vm_size_t) -> kern_return_t;
    pub fn vm_deallocate(
        target: mach_port_t,
        address: vm_address_t,
        size: vm_size_t,
    ) -> kern_return_t;
    pub fn mach_timebase_info(info: *mut mach_timebase_info_data_t) -> kern_return_t;

    pub fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
    pub fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    pub fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    pub fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
}

// ---------------------------------------------------------------------------
// CoreFoundation / IOKit (only the handful of calls used for the registry).

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFTypeID = usize;
pub type CFIndex = isize;

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kCFNumberDoubleType: CFIndex = 13;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    pub fn CFDictionaryGetTypeID() -> CFTypeID;
    pub fn CFNumberGetTypeID() -> CFTypeID;
    pub fn CFDataGetTypeID() -> CFTypeID;
    pub fn CFStringGetTypeID() -> CFTypeID;
    pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
    pub fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        s: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    pub fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32)
    -> u8;
    pub fn CFNumberGetValue(number: CFTypeRef, the_type: CFIndex, value: *mut c_void) -> u8;
    pub fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
    pub fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    pub fn IOServiceGetMatchingServices(
        main_port: mach_port_t,
        matching: CFDictionaryRef,
        existing: *mut io_iterator_t,
    ) -> kern_return_t;
    pub fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
    pub fn IOObjectRelease(object: io_object_t) -> kern_return_t;
    pub fn IORegistryEntryCreateCFProperties(
        entry: io_registry_entry_t,
        properties: *mut CFMutableDictionaryRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> kern_return_t;
    pub fn IORegistryEntryCreateCFProperty(
        entry: io_registry_entry_t,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> CFTypeRef;
    pub fn IORegistryEntryFromPath(
        main_port: mach_port_t,
        path: *const c_char,
    ) -> io_registry_entry_t;
    pub fn IORegistryEntryGetChildIterator(
        entry: io_registry_entry_t,
        plane: *const c_char,
        iterator: *mut io_iterator_t,
    ) -> kern_return_t;
}

/// The host port, fetched once (each `mach_host_self` call adds a send right).
pub fn host() -> mach_port_t {
    use std::sync::OnceLock;
    static HOST: OnceLock<mach_port_t> = OnceLock::new();
    *HOST.get_or_init(|| unsafe { mach_host_self() })
}

pub fn task_self() -> mach_port_t {
    unsafe { mach_task_self_ }
}

/// Nanoseconds per Mach absolute-time tick, as a ratio.
pub fn timebase() -> (u64, u64) {
    use std::sync::OnceLock;
    static TB: OnceLock<(u64, u64)> = OnceLock::new();
    *TB.get_or_init(|| {
        let mut info = mach_timebase_info_data_t::default();
        unsafe { mach_timebase_info(&mut info) };
        if info.denom == 0 {
            (1, 1)
        } else {
            (info.numer as u64, info.denom as u64)
        }
    })
}

pub fn mach_to_ns(ticks: u64) -> u64 {
    let (numer, denom) = timebase();
    ((ticks as u128 * numer as u128) / denom as u128) as u64
}

// ---------------------------------------------------------------------------
// sysctl helpers

pub fn sysctl_raw<T: Copy + Default>(name: &str) -> Option<T> {
    let cname = CString::new(name).ok()?;
    let mut value = T::default();
    let mut len = size_of::<T>();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            (&mut value as *mut T).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == size_of::<T>()).then_some(value)
}

pub fn sysctl_i32(name: &str) -> Option<i32> {
    sysctl_raw::<i32>(name)
}

pub fn sysctl_u64(name: &str) -> Option<u64> {
    // Some keys are 32-bit, some 64-bit; accept either width.
    let cname = CString::new(name).ok()?;
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    match len {
        4 => Some(u32::from_ne_bytes(buf[..4].try_into().ok()?) as u64),
        8 => Some(u64::from_ne_bytes(buf)),
        _ => None,
    }
}

pub fn sysctl_string(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut len = 0usize;
    unsafe {
        if libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        let s = CStr::from_bytes_until_nul(&buf)
            .ok()?
            .to_string_lossy()
            .into_owned();
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// CoreFoundation value helpers

pub struct CfString(pub CFStringRef);

impl CfString {
    pub fn new(s: &str) -> Self {
        let c = CString::new(s).expect("no interior NUL");
        Self(unsafe {
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), kCFStringEncodingUTF8)
        })
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

/// Look up `key` in a CFDictionary. The result is borrowed (not retained).
///
/// # Safety
/// `dict` must be null or a valid CFDictionary.
pub unsafe fn dict_get(dict: CFDictionaryRef, key: &str) -> CFTypeRef {
    if dict.is_null() {
        return std::ptr::null();
    }
    let k = CfString::new(key);
    unsafe { CFDictionaryGetValue(dict, k.0) }
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn cf_is_dict(v: CFTypeRef) -> bool {
    !v.is_null() && unsafe { CFGetTypeID(v) == CFDictionaryGetTypeID() }
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn cf_number_i64(v: CFTypeRef) -> Option<i64> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFNumberGetTypeID() } {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe { CFNumberGetValue(v, kCFNumberSInt64Type, (&mut out as *mut i64).cast()) };
    (ok != 0).then_some(out)
}

/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn cf_data_bytes(v: CFTypeRef) -> Option<Vec<u8>> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFDataGetTypeID() } {
        return None;
    }
    unsafe {
        let len = CFDataGetLength(v);
        let ptr = CFDataGetBytePtr(v);
        if ptr.is_null() || len <= 0 {
            return Some(Vec::new());
        }
        Some(std::slice::from_raw_parts(ptr, len as usize).to_vec())
    }
}

/// All IOKit services matching `class`. The caller owns the handles and
/// must `IOObjectRelease` them.
pub fn matching_services(class: &str) -> Vec<io_object_t> {
    let Ok(cclass) = CString::new(class) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    unsafe {
        let matching = IOServiceMatching(cclass.as_ptr());
        if matching.is_null() {
            return out;
        }
        let mut iter: io_iterator_t = 0;
        if IOServiceGetMatchingServices(0, matching, &mut iter) != KERN_SUCCESS {
            return out;
        }
        loop {
            let service = IOIteratorNext(iter);
            if service == 0 {
                break;
            }
            out.push(service);
        }
        IOObjectRelease(iter);
    }
    out
}

/// One property of a registry entry, retained (release with `CFRelease`).
pub fn entry_property(entry: io_registry_entry_t, key: &str) -> CFTypeRef {
    let k = CfString::new(key);
    unsafe { IORegistryEntryCreateCFProperty(entry, k.0, std::ptr::null(), 0) }
}

/// Read a property of a registry entry addressed by path, e.g.
/// `IODeviceTree:/product` / `product-name`.
pub fn registry_property_bytes(path: &str, key: &str) -> Option<Vec<u8>> {
    let cpath = CString::new(path).ok()?;
    unsafe {
        let entry = IORegistryEntryFromPath(0, cpath.as_ptr());
        if entry == 0 {
            return None;
        }
        let k = CfString::new(key);
        let value = IORegistryEntryCreateCFProperty(entry, k.0, std::ptr::null(), 0);
        IOObjectRelease(entry);
        if value.is_null() {
            return None;
        }
        let out = cf_data_bytes(value);
        CFRelease(value);
        out
    }
}

/// Visit the children of a registry entry in the device tree plane, handing
/// each child's properties to `f`.
pub fn for_each_registry_child(path: &str, mut f: impl FnMut(CFDictionaryRef)) {
    let Ok(cpath) = CString::new(path) else {
        return;
    };
    let plane = c"IODeviceTree";
    unsafe {
        let entry = IORegistryEntryFromPath(0, cpath.as_ptr());
        if entry == 0 {
            return;
        }
        let mut iter: io_iterator_t = 0;
        if IORegistryEntryGetChildIterator(entry, plane.as_ptr(), &mut iter) == KERN_SUCCESS {
            loop {
                let child = IOIteratorNext(iter);
                if child == 0 {
                    break;
                }
                let mut props: CFMutableDictionaryRef = std::ptr::null_mut();
                if IORegistryEntryCreateCFProperties(child, &mut props, std::ptr::null(), 0)
                    == KERN_SUCCESS
                    && !props.is_null()
                {
                    f(props);
                    CFRelease(props);
                }
                IOObjectRelease(child);
            }
            IOObjectRelease(iter);
        }
        IOObjectRelease(entry);
    }
}
