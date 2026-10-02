//! What the runner needs from the OS: which CPUs to give the servers and
//! the load, starting a process pinned to its CPUs, the processes it
//! started in turn, their CPU time and memory, and stopping them; for the
//! real traffic suite, room for its connections and timers fine enough to
//! send on time.

// Every call here is a plain query or setting on a process, with buffers
// sized and owned right at the call.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::io;
use std::process::{Child, Command};

pub use imp::*;

/// `[0, 1, 2, 5]` → `0-2,5`.
pub fn list(cpus: &[usize]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < cpus.len() {
        let mut j = i;
        while j + 1 < cpus.len() && cpus[j + 1] == cpus[j] + 1 {
            j += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        out += &if i == j {
            cpus[i].to_string()
        } else {
            format!("{}-{}", cpus[i], cpus[j])
        };
        i = j + 1;
    }
    out
}

/// Splits whole cores in two halves, so a server and the load never share
/// a core's hyperthreads. `cores` holds each core's logical CPUs.
fn halves(mut cores: Vec<Vec<usize>>) -> (Vec<usize>, Vec<usize>) {
    cores.sort();
    if cores.len() < 2 {
        // One core with several threads: split the threads instead.
        let all: Vec<usize> = cores.concat();
        let (a, b) = all.split_at(all.len() / 2);
        return (a.to_vec(), b.to_vec());
    }
    let (a, b) = cores.split_at(cores.len() / 2);
    let (mut a, mut b) = (a.concat(), b.concat());
    a.sort();
    b.sort();
    (a, b)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::os::unix::process::CommandExt;

    /// Cores from sysfs, among the CPUs this process may run on (a container
    /// can be limited to some).
    pub fn split_cpus() -> (Vec<usize>, Vec<usize>) {
        let allowed = affinity();
        let mut cores: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for &cpu in &allowed {
            let read = |f: &str| {
                std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/topology/{f}"))
                    .map(|s| s.trim().to_string())
            };
            let key = (
                read("physical_package_id").unwrap_or_default(),
                read("core_id").unwrap_or_else(|_| cpu.to_string()),
            );
            cores.entry(key).or_default().push(cpu);
        }
        halves(cores.into_values().collect())
    }

    fn affinity() -> Vec<usize> {
        // SAFETY: `set` is a plain bit set owned here; the kernel writes at
        // most its size.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            if libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut set) != 0 {
                let n = std::thread::available_parallelism().map_or(1, |n| n.get());
                return (0..n).collect();
            }
            (0..libc::CPU_SETSIZE as usize)
                .filter(|&c| libc::CPU_ISSET(c, &set))
                .collect()
        }
    }

    fn set_affinity(cpus: &[usize]) -> io::Result<()> {
        // SAFETY: as in `affinity`. Only touches the calling thread.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            for &c in cpus {
                libc::CPU_SET(c, &mut set);
            }
            if libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &set) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Pins this thread, and so every thread it starts from now on.
    pub fn pin_self(cpus: &[usize]) {
        set_affinity(cpus).expect("pinning the runner");
    }

    /// Starts `cmd` pinned from its first instruction, before a runtime
    /// sizes its thread pool, in a process group of its own so that
    /// everything it starts can be stopped together.
    pub fn spawn_pinned(cmd: &mut Command, cpus: &[usize]) -> io::Result<Child> {
        let cpus = cpus.to_vec();
        cmd.process_group(0);
        // SAFETY: the closure runs in the child between fork and exec and
        // only makes a system call; `cpus` was allocated before the fork.
        unsafe { cmd.pre_exec(move || set_affinity(&cpus)) };
        cmd.spawn()
    }

    pub fn kill(child: &mut Child) {
        // SAFETY: a signal to the process group `spawn_pinned` created.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
    }

    /// Asks `child` to stop, as systemd, Docker and Kubernetes do: SIGTERM
    /// to it alone (a process manager passes it on to its workers).
    pub fn terminate(child: &Child) {
        // SAFETY: a signal to a process this one started.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    }

    /// `pid` and every process under it.
    pub fn tree(pid: u32) -> Vec<u32> {
        let mut parent: Vec<(u32, u32)> = Vec::new();
        for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Ok(p) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if let Some(f) = stat(p) {
                parent.push((p, f[1].parse().unwrap_or(0)));
            }
        }
        let mut ids = vec![pid];
        let mut i = 0;
        while i < ids.len() {
            let id = ids[i];
            ids.extend(parent.iter().filter(|&&(_, pp)| pp == id).map(|&(p, _)| p));
            i += 1;
        }
        ids
    }

    /// The fields of `/proc/<pid>/stat` after the command name: `[0]` is
    /// the state, `[1]` the parent, `[11]` and `[12]` user and kernel time.
    fn stat(pid: u32) -> Option<Vec<String>> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &s[s.rfind(')')? + 1..];
        Some(rest.split_whitespace().map(str::to_string).collect())
    }

    /// Seconds of (total, kernel) CPU time per process.
    pub fn cpu_times(pids: &[u32]) -> HashMap<u32, (f64, f64)> {
        // SAFETY: a read of a constant.
        let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        pids.iter()
            .filter_map(|&p| {
                let f = stat(p)?;
                let (user, kernel): (f64, f64) = (f[11].parse().ok()?, f[12].parse().ok()?);
                Some((p, ((user + kernel) / tick, kernel / tick)))
            })
            .collect()
    }

    /// Bytes of the most memory `pid` has had resident.
    pub fn peak_memory(pid: u32) -> u64 {
        status_kb(pid, "VmHWM:") * 1024
    }

    /// Bytes `pid` has resident now.
    pub fn memory(pid: u32) -> u64 {
        status_kb(pid, "VmRSS:") * 1024
    }

    /// A `kB` figure of `/proc/<pid>/status`.
    fn status_kb(pid: u32, key: &str) -> u64 {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let kb = status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok());
        kb.unwrap_or(0)
    }

    /// Raises this process's open-file limit to its hard limit (the servers
    /// it starts inherit it) and returns the limit it has.
    pub fn raise_open_files() -> Option<u64> {
        // SAFETY: `r` is owned here, and only it is read and written.
        unsafe {
            let mut r: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) != 0 {
                return None;
            }
            r.rlim_cur = r.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &r);
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) != 0 {
                return None;
            }
            Some(r.rlim_cur as u64)
        }
    }

    /// Timers here are fine already.
    pub fn fine_timers() {}
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessAffinityMask, GetProcessTimes, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, SetProcessAffinityMask,
    };
    use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;

    /// Windows numbers a core's hyperthreads next to each other, so cores
    /// are consecutive pairs when there are more logical CPUs than cores.
    pub fn split_cpus() -> (Vec<usize>, Vec<usize>) {
        let n = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(64);
        let per_core = if n > 1 && n.is_multiple_of(2) { 2 } else { 1 };
        halves(
            (0..n)
                .collect::<Vec<_>>()
                .chunks(per_core)
                .map(<[usize]>::to_vec)
                .collect(),
        )
    }

    fn mask(cpus: &[usize]) -> usize {
        cpus.iter().fold(0, |m, &c| m | 1 << c)
    }

    fn set_affinity(cpus: &[usize]) -> io::Result<()> {
        // SAFETY: the pseudo handle of this process needs no closing.
        if unsafe { SetProcessAffinityMask(GetCurrentProcess(), mask(cpus)) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Pins every thread of this process.
    pub fn pin_self(cpus: &[usize]) {
        set_affinity(cpus).expect("pinning the runner");
    }

    /// A child inherits its parent's affinity, so the runner moves itself
    /// to the server's CPUs for the launch. That pins the child from its
    /// first instruction, before a runtime sizes its thread pool, and any
    /// process it starts in turn.
    pub fn spawn_pinned(cmd: &mut Command, cpus: &[usize]) -> io::Result<Child> {
        let mut own: usize = 0;
        let mut system: usize = 0;
        // SAFETY: two out-parameters owned here.
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut own, &mut system) };
        set_affinity(cpus)?;
        let child = cmd.spawn();
        // SAFETY: as in `set_affinity`.
        unsafe { SetProcessAffinityMask(GetCurrentProcess(), own) };
        child
    }

    pub fn kill(child: &mut Child) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .output();
        let _ = child.wait();
    }

    /// `pid` and every process under it.
    pub fn tree(pid: u32) -> Vec<u32> {
        let mut parent: Vec<(u32, u32)> = Vec::new();
        // SAFETY: the snapshot handle is closed below; `e` is sized as the
        // API requires before the first call.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap != INVALID_HANDLE_VALUE {
                let mut e: PROCESSENTRY32W = std::mem::zeroed();
                e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
                let mut ok = Process32FirstW(snap, &mut e);
                while ok != 0 {
                    parent.push((e.th32ProcessID, e.th32ParentProcessID));
                    ok = Process32NextW(snap, &mut e);
                }
                CloseHandle(snap);
            }
        }
        let mut ids = vec![pid];
        let mut i = 0;
        while i < ids.len() {
            let id = ids[i];
            // Pid 0 is the idle process, which is its own parent.
            ids.extend(
                parent
                    .iter()
                    .filter(|&&(p, pp)| pp == id && p != id && p != 0)
                    .map(|&(p, _)| p),
            );
            i += 1;
        }
        ids
    }

    /// Runs `f` on a query handle to `pid`, closed afterwards.
    fn with_process<T>(pid: u32, f: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
        // SAFETY: the handle is checked, used only inside `f`, then closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let r = f(h);
            CloseHandle(h);
            r
        }
    }

    /// Seconds of (total, kernel) CPU time per process. The total is
    /// counted in cycles: `GetProcessTimes` charges each 15.6 ms clock tick
    /// to the thread running at it, so a server that wakes briefly between
    /// ticks, as under a light load, shows next to none. The kernel's share
    /// is still that sampled time.
    pub fn cpu_times(pids: &[u32]) -> HashMap<u32, (f64, f64)> {
        let secs =
            |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64 / 1e7;
        let hz = cycles_per_second();
        pids.iter()
            .filter_map(|&p| {
                let times = with_process(p, |h| {
                    let [mut created, mut exited, mut kernel, mut user] = [FILETIME {
                        dwLowDateTime: 0,
                        dwHighDateTime: 0,
                    }; 4];
                    let mut cycles = 0;
                    // SAFETY: five out-parameters owned here.
                    let ok = unsafe {
                        GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0
                            && QueryProcessCycleTime(h, &mut cycles) != 0
                    };
                    let total = match hz {
                        Some(hz) => cycles as f64 / hz,
                        None => secs(user) + secs(kernel),
                    };
                    ok.then(|| (total, secs(kernel)))
                })?;
                Some((p, times))
            })
            .collect()
    }

    /// The rate of the time stamp counter, which Windows counts cycles in,
    /// measured once against the clock over 50 ms.
    fn cycles_per_second() -> Option<f64> {
        static HZ: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
        *HZ.get_or_init(|| {
            #[cfg(target_arch = "x86_64")]
            {
                use std::arch::x86_64::_rdtsc;
                let t = std::time::Instant::now();
                // SAFETY: reads the time stamp counter, which every x86-64 has.
                let c = unsafe { _rdtsc() };
                std::thread::sleep(std::time::Duration::from_millis(50));
                let c = unsafe { _rdtsc() } - c;
                Some(c as f64 / t.elapsed().as_secs_f64())
            }
            #[cfg(not(target_arch = "x86_64"))]
            None
        })
    }

    /// Bytes of the most memory `pid` has had resident.
    pub fn peak_memory(pid: u32) -> u64 {
        counters(pid).map_or(0, |c| c.PeakWorkingSetSize as u64)
    }

    /// Bytes `pid` has resident now.
    pub fn memory(pid: u32) -> u64 {
        counters(pid).map_or(0, |c| c.WorkingSetSize as u64)
    }

    fn counters(pid: u32) -> Option<PROCESS_MEMORY_COUNTERS> {
        with_process(pid, |h| {
            // SAFETY: `c` is sized as the API requires and owned here.
            let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
            c.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = unsafe { K32GetProcessMemoryInfo(h, &mut c, c.cb) };
            (ok != 0).then_some(c)
        })
    }

    /// Windows has no open-file limit to raise.
    pub fn raise_open_files() -> Option<u64> {
        None
    }

    /// Asks for 1 ms timers: by default a wait ends on a 15.6 ms tick, which
    /// would make every user's request look that late.
    pub fn fine_timers() {
        // SAFETY: a setting of this process's timer resolution.
        unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(list(&[0, 1, 2, 5, 7, 8]), "0-2,5,7-8");
        assert_eq!(list(&[]), "");
    }

    #[test]
    fn whole_cores_per_half() {
        // Linux numbering: core k is CPUs k and k+4.
        let cores = (0..4).map(|k| vec![k, k + 4]).collect();
        assert_eq!(halves(cores), (vec![0, 1, 4, 5], vec![2, 3, 6, 7]));
        assert_eq!(halves(vec![vec![0, 1]]), (vec![0], vec![1]));
    }
}
