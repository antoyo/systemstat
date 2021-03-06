// You are likely to be eaten by a grue.

use std::{io, path, ptr, mem, ffi, slice, time};
use std::collections::BTreeMap;
use libc::{c_void, c_int, c_schar, c_uchar, size_t, uid_t, sysctl, sysctlnametomib};
use data::*;
use super::common::*;
use super::unix;
use super::bsd;

pub struct PlatformImpl;

macro_rules! sysctl_mib {
    ($len:expr, $name:expr) => {
        {
            let mut mib: [c_int; $len] = [0; $len];
            let mut sz: size_t = mib.len();
            let s = ffi::CString::new($name).unwrap();
            unsafe { sysctlnametomib(s.as_ptr(), &mut mib[0], &mut sz) };
            mib
        }
    }
}

macro_rules! sysctl {
    ($mib:expr, $dataptr:expr, $size:expr, $shouldcheck:expr) => {
        {
            let mib = &$mib;
            let mut size = $size;
            if unsafe { sysctl(&mib[0], mib.len() as u32,
                               $dataptr as *mut _ as *mut c_void, &mut size, ptr::null(), 0) } != 0 && $shouldcheck {
                return Err(io::Error::new(io::ErrorKind::Other, "sysctl() failed"))
            }
            size
        }
    };
    ($mib:expr, $dataptr:expr, $size:expr) => {
        sysctl!($mib, $dataptr, $size, true)
    }
}

lazy_static! {
    static ref KERN_CP_TIMES: [c_int; 2] = sysctl_mib!(2, "kern.cp_times");
    static ref V_ACTIVE_COUNT: [c_int; 4] = sysctl_mib!(4, "vm.stats.vm.v_active_count");
    static ref V_INACTIVE_COUNT: [c_int; 4] = sysctl_mib!(4, "vm.stats.vm.v_inactive_count");
    static ref V_WIRE_COUNT: [c_int; 4] = sysctl_mib!(4, "vm.stats.vm.v_wire_count");
    static ref V_CACHE_COUNT: [c_int; 4] = sysctl_mib!(4, "vm.stats.vm.v_cache_count");
    static ref V_FREE_COUNT: [c_int; 4] = sysctl_mib!(4, "vm.stats.vm.v_free_count");
    static ref BATTERY_LIFE: [c_int; 4] = sysctl_mib!(4, "hw.acpi.battery.life");
    static ref BATTERY_TIME: [c_int; 4] = sysctl_mib!(4, "hw.acpi.battery.time");
    static ref ACLINE: [c_int; 3] = sysctl_mib!(3, "hw.acpi.acline");

    static ref CP_TIMES_SIZE: usize = {
        let mut size: usize = 0;
        unsafe { sysctl(&KERN_CP_TIMES[0], KERN_CP_TIMES.len() as u32,
                        ptr::null_mut(), &mut size, ptr::null(), 0) };
        size
    };
}

/// An implementation of `Platform` for FreeBSD.
/// See `Platform` for documentation.
impl Platform for PlatformImpl {
    #[inline(always)]
    fn new() -> Self {
        PlatformImpl
    }

    fn cpu_load(&self) -> io::Result<DelayedMeasurement<Vec<CPULoad>>> {
        let loads = try!(measure_cpu());
        Ok(DelayedMeasurement::new(
                Box::new(move || Ok(loads.iter()
                               .zip(try!(measure_cpu()).iter())
                               .map(|(prev, now)| (*now - prev).to_cpuload())
                               .collect::<Vec<_>>()))))
    }

    fn load_average(&self) -> io::Result<LoadAverage> {
        unix::load_average()
    }

    fn memory(&self) -> io::Result<Memory> {
        let mut active: usize = 0; sysctl!(V_ACTIVE_COUNT, &mut active, mem::size_of::<usize>());
        let mut inactive: usize = 0; sysctl!(V_INACTIVE_COUNT, &mut inactive, mem::size_of::<usize>());
        let mut wired: usize = 0; sysctl!(V_WIRE_COUNT, &mut wired, mem::size_of::<usize>());
        let mut cache: usize = 0; sysctl!(V_CACHE_COUNT, &mut cache, mem::size_of::<usize>(), false);
        let mut free: usize = 0; sysctl!(V_FREE_COUNT, &mut free, mem::size_of::<usize>());
        let pmem = PlatformMemory {
            active: ByteSize::kib(active << *bsd::PAGESHIFT),
            inactive: ByteSize::kib(inactive << *bsd::PAGESHIFT),
            wired: ByteSize::kib(wired << *bsd::PAGESHIFT),
            cache: ByteSize::kib(cache << *bsd::PAGESHIFT),
            free: ByteSize::kib(free << *bsd::PAGESHIFT),
        };
        Ok(Memory {
            total: pmem.active + pmem.inactive + pmem.wired + pmem.cache + pmem.free,
            free: pmem.inactive + pmem.cache + pmem.free,
            platform_memory: pmem,
        })
    }

    fn battery_life(&self) -> io::Result<BatteryLife> {
        let mut life: usize = 0; sysctl!(BATTERY_LIFE, &mut life, mem::size_of::<usize>());
        let mut time: i32 = 0; sysctl!(BATTERY_TIME, &mut time, mem::size_of::<i32>());
        Ok(BatteryLife {
            remaining_capacity: life as f32 / 100.0,
            remaining_time: time::Duration::from_secs(if time < 0 { 0 } else { time as u64 }),
        })
    }

    fn on_ac_power(&self) -> io::Result<bool> {
        let mut on: usize = 0; sysctl!(ACLINE, &mut on, mem::size_of::<usize>());
        Ok(on == 1)
    }

    fn mounts(&self) -> io::Result<Vec<Filesystem>> {
        let mut mptr: *mut statfs = ptr::null_mut();
        let len = unsafe { getmntinfo(&mut mptr, 1 as i32) };
        if len < 1 {
            return Err(io::Error::new(io::ErrorKind::Other, "getmntinfo() failed"))
        }
        let mounts = unsafe { slice::from_raw_parts(mptr, len as usize) };
        Ok(mounts.iter().map(|m| m.to_fs()).collect::<Vec<_>>())
    }

    fn mount_at<P: AsRef<path::Path>>(&self, path: P) -> io::Result<Filesystem> {
        let mut sfs: statfs = unsafe { mem::zeroed() };
        if unsafe { statfs(path.as_ref().to_string_lossy().as_ptr(), &mut sfs) } != 0 {
            return Err(io::Error::new(io::ErrorKind::Other, "statfs() failed"))
        }
        Ok(sfs.to_fs())
    }

    fn networks(&self) -> io::Result<BTreeMap<String, Network>> {
        unix::networks()
    }
}


fn measure_cpu() -> io::Result<Vec<bsd::sysctl_cpu>> {
    let cpus = *CP_TIMES_SIZE / mem::size_of::<bsd::sysctl_cpu>();
    let mut data: Vec<bsd::sysctl_cpu> = Vec::with_capacity(cpus);
    unsafe { data.set_len(cpus) };
    sysctl!(KERN_CP_TIMES, &mut data[0], *CP_TIMES_SIZE);
    Ok(data)
}

#[repr(C)]
struct fsid_t {
    val: [i32; 2],
}

// FreeBSD's native struct. If you want to know what FreeBSD
// thinks about the POSIX statvfs struct, read man 3 statvfs :D
#[repr(C)]
struct statfs {
    f_version: u32,
    f_type: u32,
    f_flags: u64,
    f_bsize: u64,
    f_iosize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: i64,
    f_files: u64,
    f_ffree: i64,
    f_syncwrites: u64,
    f_asyncwrites: u64,
    f_syncreads: u64,
    f_asyncreads: u64,
    f_spare: [u64; 10],
    f_namemax: u32,
    f_owner: uid_t,
    f_fsid: fsid_t,
    f_charspare: [c_schar; 80],
    f_fstypename: [c_schar; 16],
    f_mntfromname: [c_schar; 88],
    f_mntonname: [c_schar; 88],
}

impl statfs {
    fn to_fs(&self) -> Filesystem {
        Filesystem {
            files: self.f_files as usize - self.f_ffree as usize,
            free: ByteSize::b(self.f_bfree as usize * self.f_bsize as usize),
            avail: ByteSize::b(self.f_bavail as usize * self.f_bsize as usize),
            total: ByteSize::b(self.f_blocks as usize * self.f_bsize as usize),
            name_max: self.f_namemax as usize,
            fs_type: unsafe { ffi::CStr::from_ptr(&self.f_fstypename[0]).to_string_lossy().into_owned() },
            fs_mounted_from: unsafe { ffi::CStr::from_ptr(&self.f_mntfromname[0]).to_string_lossy().into_owned() },
            fs_mounted_on: unsafe { ffi::CStr::from_ptr(&self.f_mntonname[0]).to_string_lossy().into_owned() },
        }
    }
}

#[link(name = "c")]
extern "C" {
    fn getmntinfo(mntbufp: *mut *mut statfs, flags: c_int) -> c_int;
    fn statfs(path: *const c_uchar, buf: *mut statfs) -> c_int;
}
