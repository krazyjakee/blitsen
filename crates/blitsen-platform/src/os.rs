//! What machine this is: processor, memory, storage volumes and OS identity.
//!
//! The web has no spelling for any of it. `navigator.hardwareConcurrency` is
//! the nearest thing and answers one number, deliberately coarsened; a page
//! cannot ask what the processor is called, how much memory is installed, or
//! what is mounted. So this is `blitsen/os` rather than a shim over something
//! standard — the entry condition for a native module (TECH.md §9).
//!
//! Backed by `sysinfo`, which implements each fact per platform behind one API,
//! so Linux, Windows and macOS answer the same shape. Facts sysinfo cannot get
//! on a platform come back `None` and reach JavaScript as `null`: a machine
//! that does not report its kernel version is not an error, and inventing a
//! string for it would be worse than saying so.
//!
//! Present on Android, and the only `blitsen/*` module in this crate that is. The
//! reason is that the paragraph above already answers the question Android asks.
//! `sysinfo` reads the same `/proc/stat`, `/proc/meminfo` and `/proc/mounts`
//! there as on any other Linux, so [`cpu`] and [`memory`] are the same facts
//! from the same source; the halves Android genuinely restricts — process
//! enumeration behind `hidepid`, the user database — are the ones already left
//! out of the feature list in `Cargo.toml`, so nothing here reaches for them.
//! Where a fact is missing it arrives as `None`, which is the contract a Linux
//! machine that will not report its kernel version already gets.
//!
//! What is worth writing down rather than assuming is that two of these read
//! *differently* there without reading wrongly. [`storage`] lists what is
//! mounted, and on Android that is the system's own volumes — `/data`, the
//! emulated external storage — rather than disks a user would recognise, most of
//! them unwritable by this application; whether a path can be written is
//! `node:fs`'s answer to give and not this module's to pre-empt. [`Cpu::usage`]
//! is a share of cores a governor may have parked without telling this process,
//! so a low reading there is not the same claim about the machine that a low
//! reading on a desktop is. Both remain true statements about what is mounted
//! and what ran, which is all this module ever promised (#147).
//!
//! Two instances are kept for the life of the thread rather than built per
//! call. That is not only an allocation: CPU usage is a *delta* between two
//! samples, so a fresh `System` per call would measure nothing and report zero
//! forever. See [`cpu`].
//!
//! [`batteries`] is the one reading here that `sysinfo` does not make — it
//! carries no battery feature to turn on — so it is a second library, chosen on
//! the same argument as the first: power is a different source on every
//! platform and one crate already implements all of them. It is also the one
//! reading absent on Android, where that crate does not compile and where the
//! answer belongs to `BatteryManager` over JNI.

use std::cell::RefCell;

use serde::Serialize;
use sysinfo::{Disks, RefreshKind, System};

#[cfg(not(target_os = "android"))]
use crate::PlatformError;

thread_local! {
    // Built refreshing nothing: every getter below refreshes what it needs, so
    // a baseline taken here would only be a sample thrown away microseconds
    // later by the first real call. What the first call then reports is
    // [`cpu`]'s to explain, and it is not zero.
    static SYSTEM: RefCell<System> = RefCell::new(System::new_with_specifics(RefreshKind::nothing()));
    static DISKS: RefCell<Disks> = RefCell::new(Disks::new_with_refreshed_list());
}

/// One logical processor: a hardware thread as the OS schedules onto it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Core {
    /// What the OS calls it — `cpu0`, `CPU 0`, and so on.
    pub name: String,
    /// Current clock in MHz, or 0 where the platform does not report one.
    pub frequency: u64,
    /// Share of this core used since the previous sample, 0–100.
    pub usage: f32,
}

/// The processor, as a spec sheet plus a live sample.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cpu {
    /// Marketing name: "AMD Ryzen 9 5950X 16-Core Processor", or `None` where
    /// the platform does not carry one — arm64 Windows has no
    /// `ProcessorNameString` in the registry, and reads as nothing rather than
    /// as an empty name (#137).
    pub brand: Option<String>,
    /// Vendor string as the silicon reports it: "GenuineIntel", "AuthenticAMD",
    /// or `None` where the platform does not report one.
    pub vendor: Option<String>,
    /// Instruction set architecture: "x86_64", "aarch64".
    pub architecture: String,
    /// Physical cores, or `None` where the platform will not say — which is not
    /// the same as one, so it is not defaulted to one.
    pub physical_cores: Option<usize>,
    /// Logical processors, which is `cores.len()`.
    pub logical_cores: usize,
    /// Usage across the whole package since the previous sample, 0–100.
    pub usage: f32,
    /// Per-core detail, in the order the OS enumerates them.
    pub cores: Vec<Core>,
}

/// Installed and in-use memory, in bytes throughout — no unit is implied by a
/// field name, because a caller that guesses wrong is off by 1024.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    /// Physical memory installed.
    pub total: u64,
    /// What a new allocation could get, which is not `total - used`: it counts
    /// reclaimable cache the kernel would evict on demand.
    pub available: u64,
    /// Physical memory in use.
    pub used: u64,
    /// Swap configured.
    pub swap_total: u64,
    /// Swap in use.
    pub swap_used: u64,
}

/// A mounted filesystem, which is what a user means by "a drive".
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// The device or volume label the OS reports.
    pub name: String,
    /// Where it is mounted: `/`, `/home`, `C:\`.
    pub mount_point: String,
    /// Filesystem driver: "ext4", "apfs", "NTFS".
    pub file_system: String,
    /// `"ssd"`, `"hdd"`, or `"unknown"` where the platform will not classify it.
    pub kind: &'static str,
    /// Capacity in bytes.
    pub total: u64,
    /// Free bytes a caller could write.
    pub available: u64,
    /// Whether the medium can be ejected.
    pub removable: bool,
    /// Whether the mount refuses writes.
    pub read_only: bool,
}

/// The operating system and this boot of it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    /// OS name: "Ubuntu", "Windows", "Darwin".
    pub name: Option<String>,
    /// The long form where one exists: "Ubuntu 24.04.1 LTS".
    pub long_name: Option<String>,
    /// OS release: "24.04", "11".
    pub os_version: Option<String>,
    /// Kernel release: "6.8.0-124-generic".
    pub kernel_version: Option<String>,
    /// Distribution identifier — `ID` from os-release on Linux, and the OS name
    /// elsewhere.
    pub distribution_id: String,
    /// This machine's hostname.
    pub host_name: Option<String>,
    /// Seconds since boot.
    pub uptime: u64,
    /// Boot time as a Unix timestamp in seconds.
    pub boot_time: u64,
}

/// Samples the processor.
///
/// Usage is the share of each core busy *since the previous call*, which makes
/// the first call the exception: it has no previous call to measure from, so
/// what it reports is a baseline against the counters' own origin — on Linux,
/// where those counters are `/proc/stat` and run from boot, that is the average
/// since boot. It is a real number rather than noise, but it is not the number
/// the second call gives, so a monitor discards it and starts from the second.
///
/// Sampling faster than `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` (200 ms) is what
/// the platform will not resolve, rather than a rate this function enforces.
pub fn cpu() -> Cpu {
    SYSTEM.with_borrow_mut(|system| {
        system.refresh_cpu_all();
        let cores: Vec<Core> = system
            .cpus()
            .iter()
            .map(|cpu| Core {
                name: cpu.name().to_owned(),
                frequency: cpu.frequency(),
                usage: cpu.cpu_usage(),
            })
            .collect();
        // Taken from the first core rather than a per-core string: these are one
        // package's threads and the strings are identical, and an empty machine
        // list would otherwise have to invent a brand.
        let first = system.cpus().first();
        Cpu {
            brand: first.and_then(|cpu| reported(cpu.brand())),
            vendor: first.and_then(|cpu| reported(cpu.vendor_id())),
            architecture: normalized_architecture(System::cpu_arch()),
            physical_cores: System::physical_core_count(),
            logical_cores: cores.len(),
            usage: system.global_cpu_usage(),
            cores,
        }
    })
}

/// A string the platform actually answered with, or `None`.
///
/// `sysinfo` fills a field it could not read with an empty string, which is a
/// third state this module does not have: everything else it cannot answer is
/// an `Option` that reaches JavaScript as `null`. An application showing the
/// processor name needs to tell "this machine does not report one" from "the
/// bridge has not read it yet", and `""` says neither (#137).
fn reported(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Normalises an instruction set name to the one vocabulary `Cpu::architecture`
/// documents.
///
/// `sysinfo` answers with whatever the platform calls the machine, and the
/// platforms disagree about the same silicon: macOS says `arm64` where Linux
/// says `aarch64`, and Windows says `AMD64` where both say `x86_64`. An
/// application that branches on this should not have to know which host it is
/// reading, so the spelling is chosen here — Rust's own `cfg(target_arch)`
/// names, because they are the ones the rest of the codebase is written in.
fn normalized_architecture(arch: String) -> String {
    match arch.to_ascii_lowercase().as_str() {
        "arm64" => "aarch64".to_owned(),
        "amd64" | "x64" => "x86_64".to_owned(),
        "x86" | "i386" | "i686" => "x86".to_owned(),
        _ => arch,
    }
}

/// Reads memory and swap.
pub fn memory() -> Memory {
    SYSTEM.with_borrow_mut(|system| {
        system.refresh_memory();
        Memory {
            total: system.total_memory(),
            available: system.available_memory(),
            used: system.used_memory(),
            swap_total: system.total_swap(),
            swap_used: system.used_swap(),
        }
    })
}

/// Lists the mounted volumes.
///
/// The list is refreshed rather than rebuilt, and volumes that went away are
/// dropped from it, so a removable disk unplugged between calls disappears
/// instead of lingering with stale free space.
pub fn storage() -> Vec<Volume> {
    DISKS.with_borrow_mut(|disks| {
        disks.refresh(true);
        disks
            .list()
            .iter()
            .map(|disk| Volume {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                kind: match disk.kind() {
                    sysinfo::DiskKind::SSD => "ssd",
                    sysinfo::DiskKind::HDD => "hdd",
                    sysinfo::DiskKind::Unknown(_) => "unknown",
                },
                total: disk.total_space(),
                available: disk.available_space(),
                removable: disk.is_removable(),
                read_only: disk.is_read_only(),
            })
            .collect()
    })
}

/// Reads the operating system's identity and this boot of it.
pub fn host() -> Host {
    Host {
        name: System::name(),
        long_name: System::long_os_version(),
        os_version: System::os_version(),
        kernel_version: System::kernel_version(),
        distribution_id: System::distribution_id(),
        host_name: System::host_name(),
        uptime: System::uptime(),
        boot_time: System::boot_time(),
    }
}

/// One battery, as the machine's own power meter reads it.
///
/// Every field is a share, a count or a duration in seconds. Nothing here is an
/// energy: the drivers report those in µWh, µJ and mAh depending on the machine,
/// and a number whose unit depends on the host is not a fact this module can
/// hand out.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    /// Charge as a share of what this battery holds today, 0–1. This is the
    /// number the desktop shows near the clock, taken from the controller
    /// rather than divided out of the energy readings, because many drivers
    /// report it more precisely than the two numbers behind it.
    pub level: f64,
    /// `"charging"`, `"discharging"`, `"full"`, `"empty"`, or `"unknown"` where
    /// the controller will not say — which is a state the platforms really do
    /// report, and not a failure to read.
    pub state: &'static str,
    /// Seconds until full, or `None` when the platform does not estimate one.
    /// Absent whenever the battery is not charging, and absent on a charging
    /// battery whose driver reports no rate.
    pub time_to_full: Option<f64>,
    /// Seconds until empty, on the same terms as [`Battery::time_to_full`].
    pub time_to_empty: Option<f64>,
    /// What it holds today as a share of what it held new. Above 1 on a battery
    /// whose design figure is conservative, which is a reading rather than an
    /// error, so it is not clamped.
    pub health: f64,
    /// Charge cycles the controller has counted, or `None` where it counts none.
    pub cycle_count: Option<u32>,
    /// Manufacturer, or `None` where the platform does not name one.
    pub vendor: Option<String>,
    /// Model name, or `None` where the platform does not name one.
    pub model: Option<String>,
}

/// Lists the batteries installed in this machine.
///
/// An empty list is an answer rather than a refusal: it is what a desktop with
/// no battery reports, and it is the reading this module could not make before
/// a real power source was behind it. A machine that cannot be asked at all —
/// no power service, a platform that refuses the query — is the error case, and
/// it is an error rather than an empty list precisely so that the two cannot be
/// confused (#98).
///
/// Peripherals are not in it. A wireless mouse or a keyboard publishes its cell
/// next to the machine's own on Linux, and only the ones the platform scopes to
/// the system are batteries this machine runs on.
///
/// Nothing is cached between calls, unlike the readings above: this is a level
/// rather than a delta, so there is no previous sample for a second one to be
/// measured against, and the platform handle is opened and dropped around the
/// one reading it is needed for.
#[cfg(not(target_os = "android"))]
pub fn batteries() -> Result<Vec<Battery>, PlatformError> {
    use starship_battery::units::{ratio::ratio, time::second};

    let manager = starship_battery::Manager::new().map_err(|error| {
        PlatformError::new(format!("this system reports no power source: {error}"))
    })?;
    let batteries = manager.batteries().map_err(|error| {
        PlatformError::new(format!("the batteries could not be enumerated: {error}"))
    })?;
    batteries
        .map(|battery| {
            // One unreadable battery fails the whole list rather than being
            // skipped: a reading with a battery silently missing from it is the
            // same shape as a machine that has one fewer, and an application
            // deciding whether to warn about power cannot tell those apart.
            let battery = battery.map_err(|error| {
                PlatformError::new(format!("a battery could not be read: {error}"))
            })?;
            Ok(Battery {
                level: f64::from(battery.state_of_charge().get::<ratio>()),
                state: battery_state(battery.state()),
                time_to_full: battery
                    .time_to_full()
                    .map(|time| f64::from(time.get::<second>())),
                time_to_empty: battery
                    .time_to_empty()
                    .map(|time| f64::from(time.get::<second>())),
                health: f64::from(battery.state_of_health().get::<ratio>()),
                cycle_count: battery.cycle_count(),
                vendor: battery.vendor().and_then(reported),
                model: battery.model().and_then(reported),
            })
        })
        .collect()
}

/// The one vocabulary [`Battery::state`] documents.
///
/// Written out rather than derived from the library's own `Display`, which is a
/// human-facing string it is free to reword: these five words are the API, and
/// an application branching on them should not have to track a dependency's
/// formatting. `Unknown` is carried through as itself because it is what a
/// controller mid-transition really reports, and calling it "discharging" would
/// be inventing the answer it declined to give.
#[cfg(not(target_os = "android"))]
fn battery_state(state: starship_battery::State) -> &'static str {
    match state {
        starship_battery::State::Charging => "charging",
        starship_battery::State::Discharging => "discharging",
        starship_battery::State::Empty => "empty",
        starship_battery::State::Full => "full",
        starship_battery::State::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every assertion here is a fact about the machine running the test rather
    // than about a fixture, so each one is chosen to hold on any machine that
    // can run a test at all: something is mounted, memory is installed, and at
    // least one core is executing this.
    // The vocabulary, not the machine: `sysinfo` reports the platform's own
    // spelling and the platforms disagree about identical silicon, so this is
    // the check that an application reading `architecture` gets one answer per
    // instruction set rather than one per operating system.
    #[test]
    fn the_architecture_is_reported_in_one_vocabulary() {
        for (reported, expected) in [
            ("arm64", "aarch64"),
            ("aarch64", "aarch64"),
            ("AMD64", "x86_64"),
            ("x86_64", "x86_64"),
            ("i686", "x86"),
        ] {
            assert_eq!(normalized_architecture(reported.to_owned()), expected);
        }
        // An unfamiliar architecture is passed through rather than blanked: a
        // name this does not recognise is still better than an empty string.
        assert_eq!(normalized_architecture("riscv64".to_owned()), "riscv64");
        // And what this host actually answers agrees with what it was compiled
        // for, which is the claim the mapping exists to make true.
        assert_eq!(cpu().architecture, std::env::consts::ARCH);
    }

    #[test]
    fn the_processor_reports_at_least_one_core() {
        let cpu = cpu();
        assert!(cpu.logical_cores >= 1, "{cpu:?}");
        assert_eq!(cpu.logical_cores, cpu.cores.len());
        assert!(!cpu.architecture.is_empty());
        // Usage is a range check rather than a value: the first sample is a
        // baseline against the counters' origin and the second measures the gap
        // between the two calls, and neither is a number a test can predict on
        // a machine it does not control.
        assert!((0.0..=100.0).contains(&cpu.usage), "{}", cpu.usage);
        for core in &cpu.cores {
            assert!((0.0..=100.0).contains(&core.usage), "{core:?}");
        }
        assert!((0.0..=100.0).contains(&super::cpu().usage));
        // A name the platform does not carry is absent rather than empty, so a
        // caller never has to treat `""` as a third state (#137). Which hosts
        // have one is not something a test can require — arm64 Windows does
        // not — so the claim is about the shape, not the presence.
        for name in [&cpu.brand, &cpu.vendor] {
            assert!(name.as_deref() != Some(""), "{cpu:?}");
        }
    }

    #[test]
    fn memory_is_installed_and_used_fits_inside_it() {
        let memory = memory();
        assert!(memory.total > 0, "{memory:?}");
        assert!(memory.used <= memory.total, "{memory:?}");
        assert!(memory.available <= memory.total, "{memory:?}");
        assert!(memory.swap_used <= memory.swap_total, "{memory:?}");
    }

    #[test]
    fn a_volume_is_mounted_and_its_free_space_fits_on_it() {
        let volumes = storage();
        assert!(!volumes.is_empty(), "no volume is mounted");
        for volume in &volumes {
            assert!(!volume.mount_point.is_empty(), "{volume:?}");
            assert!(volume.available <= volume.total, "{volume:?}");
            assert!(
                ["ssd", "hdd", "unknown"].contains(&volume.kind),
                "{volume:?}"
            );
        }
    }

    #[test]
    fn the_host_has_booted() {
        let host = host();
        assert!(host.boot_time > 0, "{host:?}");
        assert!(!host.distribution_id.is_empty(), "{host:?}");
    }

    // The vocabulary, on a machine that may have no battery to read: which
    // states this host can produce is not something a test can arrange, so the
    // mapping is checked directly and the reading below checks the rest.
    #[cfg(not(target_os = "android"))]
    #[test]
    fn every_battery_state_has_one_word_for_it() {
        for (state, expected) in [
            (starship_battery::State::Charging, "charging"),
            (starship_battery::State::Discharging, "discharging"),
            (starship_battery::State::Empty, "empty"),
            (starship_battery::State::Full, "full"),
            (starship_battery::State::Unknown, "unknown"),
        ] {
            assert_eq!(battery_state(state), expected);
        }
    }

    // Whether this machine has a battery is not something a test can require —
    // the same suite runs on a laptop and on a rack — so what is asserted is
    // that asking succeeded and that every battery answered describes a real
    // one. A desktop's empty list is a pass, and it is the reading that would
    // otherwise be indistinguishable from a failure to ask (#98).
    #[cfg(not(target_os = "android"))]
    #[test]
    fn every_battery_reports_a_charge_it_could_hold() {
        let batteries = batteries().expect("this machine can be asked about power");
        for battery in &batteries {
            assert!((0.0..=1.0).contains(&battery.level), "{battery:?}");
            assert!(battery.health > 0.0, "{battery:?}");
            assert!(
                ["charging", "discharging", "empty", "full", "unknown"].contains(&battery.state),
                "{battery:?}"
            );
            // Absent rather than empty, the same rule the processor's names
            // follow, so `null` never has to be told from `""` (#137).
            for name in [&battery.vendor, &battery.model] {
                assert!(name.as_deref() != Some(""), "{battery:?}");
            }
            // An estimate is for the direction the battery is actually going.
            // Both at once would be a controller answering a question it was
            // not asked, and neither is what a battery holding steady says.
            assert!(
                battery.time_to_full.is_none() || battery.time_to_empty.is_none(),
                "{battery:?}"
            );
            for estimate in [battery.time_to_full, battery.time_to_empty]
                .into_iter()
                .flatten()
            {
                assert!(estimate > 0.0, "{battery:?}");
            }
        }
    }
}
