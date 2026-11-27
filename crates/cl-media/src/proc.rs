//! OS part of the FFmpeg child processes: no orphans after the app is killed,
//! low priority, pause/resume and CPU time. Without extra crates (direct
//! system calls), each with a silent fallback if the system cannot do it.

use std::process::{Child, Command};
use std::time::Duration;

/// Called before every start of an FFmpeg/FFprobe (from `command`).
/// `adopt`: assign the child to the job via process search (otherwise [`raise_priority`] does it directly).
pub(crate) fn prepare(cmd: &mut Command, adopt: bool) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(win::CREATE_NO_WINDOW); // don't let a console window flash up
        if adopt {
            win::watch();
        }
    }
    #[cfg(not(windows))]
    let _ = adopt;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        extern "C" {
            fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
        }
        // the child ends with the parent process (PR_SET_PDEATHSIG = 1, SIGKILL = 9). Note: Linux
        // reports the end of the *starting thread* – so start long-running processes from threads
        // that live at least as long (UI thread or a waiting worker thread).
        // SAFETY: only one async-signal-safe system call between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                prctl(1, 9, 0, 0, 0);
                Ok(())
            });
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = cmd;
}

/// Starts the process with below-normal priority (background work like proxies).
pub fn lower_priority(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(win::CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        extern "C" {
            fn nice(inc: i32) -> i32;
        }
        // SAFETY: nice() is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                nice(10);
                Ok(())
            });
        }
    }
    #[cfg(not(any(windows, unix)))]
    let _ = cmd;
}

/// Above-normal priority for the playback processes (frame streams, player single frames,
/// audio windows): under full load from other programs they otherwise get too little CPU time and
/// the picture freezes while the audio runs. `false` if the system does not allow it (on
/// Linux/macOS raising needs privileges – there it stays at normal). On Windows the process
/// is assigned directly to the job object as well (ends with the app, no process search per start).
pub fn raise_priority(child: &Child) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
        win::assign(child.as_raw_handle());
        // SAFETY: valid process handle of the not yet reaped child.
        unsafe { win::SetPriorityClass(child.as_raw_handle(), ABOVE_NORMAL_PRIORITY_CLASS) != 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        false
    }
}

/// Threads of a raised playback process: half the cores (at least 1). With all
/// cores above normal priority two software streams would otherwise crowd out everything else – including the
/// UI thread and the audio feed that fetch the frames.
pub fn playback_threads(cores: usize) -> usize {
    (cores / 2).max(1)
}

/// Thread limit for a playback process (call before the first `-i`). `hw`: decoding on
/// the GPU – then the decoder stays unlimited, only the filters are limited.
pub fn limit_threads(cmd: &mut Command, hw: bool) {
    let n = playback_threads(std::thread::available_parallelism().map_or(2, |n| n.get())).to_string();
    cmd.args(["-filter_threads", &n, "-filter_complex_threads", &n]);
    if !hw {
        cmd.args(["-threads", &n]);
    }
}

/// Pauses a process (`true`) or resumes it. `false` if the system cannot do it.
pub fn suspend(child: &Child, on: bool) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: valid process handle of the not yet reaped child.
        unsafe {
            let h = child.as_raw_handle();
            (if on { win::NtSuspendProcess(h) } else { win::NtResumeProcess(h) }) >= 0
        }
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        let (stop, cont) = (17, 19);
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
        let (stop, cont) = (19, 18);
        // SAFETY: signal to our own child.
        unsafe { kill(child.id() as i32, if on { stop } else { cont }) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (child, on);
        false
    }
}

/// CPU time used (user + system) by a child process, as far as obtainable.
pub fn cpu_time(child: &Child) -> Option<Duration> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let mut t = [0u64; 4];
        // SAFETY: four FILETIME fields (64 bits each), valid handle.
        let ok = unsafe {
            win::GetProcessTimes(
                child.as_raw_handle(),
                &mut t[0],
                &mut t[1],
                &mut t[2],
                &mut t[3],
            )
        };
        (ok != 0).then(|| Duration::from_nanos((t[2] + t[3]) * 100))
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        None
    }
}

#[cfg(windows)]
pub(crate) mod win {
    use std::ffi::c_void;
    use std::sync::{Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        min_ws: usize,
        max_ws: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        pri: i32,
        flags: u32,
        exe: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attr: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *mut c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(h: Handle) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snap: Handle, e: *mut ProcessEntry) -> i32;
        fn Process32NextW(snap: Handle, e: *mut ProcessEntry) -> i32;
        fn GetCurrentProcessId() -> u32;
        pub fn SetPriorityClass(h: Handle, class: u32) -> i32;
        pub fn GetProcessTimes(h: Handle, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
        #[cfg(test)]
        pub fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        #[cfg(test)]
        pub fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
    }
    #[link(name = "ntdll")]
    extern "system" {
        pub fn NtSuspendProcess(h: Handle) -> i32;
        pub fn NtResumeProcess(h: Handle) -> i32;
    }

    /// Job object with "kill on job close": the handle lives until the process ends; when
    /// Windows closes it (also on a hard kill), all assigned FFmpeg processes end.
    pub(super) fn job() -> Option<usize> {
        static JOB: OnceLock<Option<usize>> = OnceLock::new();
        *JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info = ExtendedLimit::default();
            info.basic.limit_flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            let ok = SetInformationJobObject(
                h,
                9, // JobObjectExtendedLimitInformation
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<ExtendedLimit>() as u32,
            );
            (ok != 0).then_some(h as usize)
        })
    }

    /// Assigns a process (handle of our own child) directly to the job.
    pub fn assign(process: Handle) {
        if let Some(job) = job() {
            // SAFETY: valid handles; if it fails, the process just stays without a job.
            unsafe { AssignProcessToJobObject(job as Handle, process) };
        }
    }

    /// Assigns all running FFmpeg/FFprobe children of this process to the job. Only these:
    /// other children (update installer, browser) should outlive the app.
    pub fn adopt_children() {
        let Some(job) = job() else { return };
        unsafe {
            let snap = CreateToolhelp32Snapshot(2, 0); // TH32CS_SNAPPROCESS
            if snap as isize == -1 {
                return;
            }
            let me = GetCurrentProcessId();
            let mut e: ProcessEntry = std::mem::zeroed();
            e.size = std::mem::size_of::<ProcessEntry>() as u32;
            let mut more = Process32FirstW(snap, &mut e) != 0;
            while more {
                if e.parent == me && is_ffmpeg(&e.exe) {
                    // PROCESS_SET_QUOTA | PROCESS_TERMINATE
                    let p = OpenProcess(0x0100 | 0x0001, 0, e.pid);
                    if !p.is_null() {
                        AssignProcessToJobObject(job as Handle, p);
                        CloseHandle(p);
                    }
                }
                more = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
    }

    fn is_ffmpeg(exe: &[u16; 260]) -> bool {
        let n = exe.iter().position(|c| *c == 0).unwrap_or(exe.len());
        let s = String::from_utf16_lossy(&exe[..n]).to_ascii_lowercase();
        s == "ffmpeg.exe" || s == "ffprobe.exe"
    }

    static HINT: Mutex<Option<Instant>> = Mutex::new(None);
    static WAKE: Condvar = Condvar::new();

    /// An FFmpeg is about to start: the watcher thread searches for new children several times
    /// in the next 1.5 s (at once, then less often) and assigns them to the job. Not for
    /// the playback processes: `raise_priority` assigns those directly.
    /// shortcut: whoever is killed in the first milliseconds after the start can still
    /// leave an orphan; assigning directly would need `raw_attribute` (still unstable).
    pub fn watch() {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            let _ = std::thread::Builder::new()
                .name("cl-ffmpeg-job".into())
                .spawn(|| loop {
                    let since = {
                        let mut g = HINT.lock().unwrap_or_else(|e| e.into_inner());
                        while g.is_none() {
                            g = WAKE.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                        let since = g.map(|t| t.elapsed()).unwrap_or_default();
                        if since > Duration::from_millis(1500) {
                            *g = None;
                            continue;
                        }
                        since
                    };
                    adopt_children();
                    std::thread::sleep(scan_pause(since));
                });
        });
        *HINT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        WAKE.notify_one();
    }

    /// Interval of the search runs: dense shortly after a start, then less often.
    pub(crate) fn scan_pause(since_hint: Duration) -> Duration {
        Duration::from_millis(match since_hint.as_millis() {
            0..=50 => 10,
            51..=300 => 25,
            _ => 200,
        })
    }
}

#[cfg(test)]
mod tests {
    /// Playback processes run above normal priority (Windows); elsewhere it silently stays at normal.
    #[test]
    fn playback_processes_can_be_raised() {
        if !crate::test_tools() {
            return;
        }
        let mut c = crate::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-re", "-f", "lavfi", "-i", "nullsrc=s=16x16:r=5", "-t", "3", "-f", "null", "-"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert_eq!(super::raise_priority(&c), cfg!(windows));
        let _ = c.kill();
        let _ = c.wait();
    }

    /// Review finding: raised playback processes ran with all cores.
    #[test]
    fn raised_processes_use_half_the_cores() {
        use super::playback_threads as t;
        assert_eq!((t(1), t(2), t(3), t(4), t(8), t(16)), (1, 1, 1, 2, 4, 8));
        let args = |hw| {
            let mut c = std::process::Command::new("ffmpeg");
            super::limit_threads(&mut c, hw);
            c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>()
        };
        let n = t(std::thread::available_parallelism().map_or(2, |n| n.get())).to_string();
        let sw = args(false);
        assert!(sw.windows(2).any(|w| w[0] == "-threads" && w[1] == n), "{sw:?}");
        assert!(sw.windows(2).any(|w| w[0] == "-filter_complex_threads" && w[1] == n));
        // hardware: don't limit the decoder, only the filters
        let hw = args(true);
        assert!(!hw.contains(&"-threads".to_string()) && hw.contains(&"-filter_threads".to_string()), "{hw:?}");
    }

    /// Review finding: playback processes too (streams, single frames, audio windows) only got
    /// into the job via the process search. Now they are in it right after `raise_priority`.
    #[cfg(windows)]
    #[test]
    fn playback_processes_join_the_job_directly() {
        use std::os::windows::io::AsRawHandle;
        if !crate::test_tools() {
            return;
        }
        let mut c = crate::playback_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-re", "-f", "lavfi", "-i", "nullsrc=s=16x16:r=5", "-t", "3", "-f", "null", "-"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        super::raise_priority(&c);
        let job = super::win::job().expect("job object") as *mut std::ffi::c_void;
        let mut inside = 0;
        // SAFETY: valid handles
        let ok = unsafe { super::win::IsProcessInJob(c.as_raw_handle(), job, &mut inside) };
        let _ = c.kill();
        let _ = c.wait();
        assert!(ok != 0 && inside != 0, "not in the job");
    }

    #[cfg(windows)]
    #[test]
    fn scan_is_dense_right_after_a_start() {
        use std::time::Duration;
        let p = super::win::scan_pause;
        assert!(p(Duration::ZERO) < p(Duration::from_millis(100)));
        assert!(p(Duration::from_millis(100)) < p(Duration::from_secs(1)));
    }

    /// Helper process for `ffmpeg_dies_with_its_owner`: starts an endless FFmpeg, reports
    /// its PID and then waits forever. Not run by hand: that test starts it with
    /// `CUTLINE_ORPHAN_HELPER=1` (without the variable it returns at once).
    #[test]
    #[ignore]
    fn orphan_helper() {
        if std::env::var_os("CUTLINE_ORPHAN_HELPER").is_none() {
            return;
        }
        let mut c = crate::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-re", "-f", "lavfi", "-i", "nullsrc=s=16x16:r=5", "-f", "null", "-"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(windows)]
        super::win::adopt_children();
        // write directly: the test harness would hold back `println!` until the end
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "PID={}", c.id());
        let _ = o.flush();
        let _ = c.wait();
    }

    /// Hard kill of the owner (TerminateProcess) also ends its FFmpeg.
    #[cfg(windows)]
    #[test]
    fn ffmpeg_dies_with_its_owner() {
        use std::io::BufRead;
        if !crate::test_tools() {
            return;
        }
        let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "proc::tests::orphan_helper", "--ignored", "--nocapture", "--test-threads=1"])
            .env("CUTLINE_ORPHAN_HELPER", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(helper.stdout.take().unwrap());
        let pid: u32 = out
            .lines()
            .map_while(Result::ok)
            .find_map(|l| l.split("PID=").nth(1).and_then(|p| p.trim().parse().ok()))
            .expect("helper process reports no PID");
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        }
        // SAFETY: SYNCHRONIZE handle to the FFmpeg, opened before the owner is killed
        let h = unsafe { OpenProcess(0x0010_0000, 0, pid) };
        assert!(!h.is_null(), "FFmpeg {pid} is not running");
        helper.kill().unwrap();
        let _ = helper.wait();
        let r = unsafe { super::win::WaitForSingleObject(h, 5000) };
        unsafe { CloseHandle(h) };
        assert_eq!(r, 0, "FFmpeg lives on after its owner ended");
    }
}
