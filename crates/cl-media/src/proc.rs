//! Betriebssystem-Teil der FFmpeg-Kindprozesse: keine Waisen nach hartem Beenden der App,
//! niedrige Priorität, Anhalten/Fortsetzen und CPU-Zeit. Ohne zusätzliche Crates (direkte
//! Systemaufrufe), jeweils mit stillem Rückfall, wenn das System es nicht kann.

use std::process::{Child, Command};
use std::time::Duration;

/// Wird vor jedem Start eines FFmpeg/FFprobe aufgerufen (aus `command`).
pub(crate) fn prepare(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(win::CREATE_NO_WINDOW); // kein Konsolenfenster aufblitzen lassen
        win::watch();
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        extern "C" {
            fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
        }
        // Kind endet mit dem Elternprozess (PR_SET_PDEATHSIG = 1, SIGKILL = 9). Achtung: Linux
        // meldet das Ende des *startenden Threads* – lange laufende Prozesse daher aus Threads
        // starten, die mindestens so lange leben (UI-Thread oder ein wartender Arbeits-Thread).
        // SAFETY: nur ein async-signal-sicherer Systemaufruf zwischen fork und exec.
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

/// Startet den Prozess mit Priorität unter normal (Hintergrundarbeit wie Proxys).
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
        // SAFETY: nice() ist async-signal-sicher.
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

/// Priorität über normal für die Prozesse der Wiedergabe (Bildströme, Einzelbilder des Players,
/// Ton-Fenster): unter Volllast durch andere Programme bekommen sie sonst zu wenig Rechenzeit und
/// das Bild bleibt stehen, obwohl der Ton läuft. `false`, wenn das System es nicht erlaubt (unter
/// Linux/macOS braucht Anheben Rechte – dort bleibt es bei normal).
pub fn raise_priority(child: &Child) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
        // SAFETY: gültiges Prozess-Handle des noch nicht abgeholten Kindes.
        unsafe { win::SetPriorityClass(child.as_raw_handle(), ABOVE_NORMAL_PRIORITY_CLASS) != 0 }
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        false
    }
}

/// Hält einen Prozess an (`true`) oder setzt ihn fort. `false`, wenn das System es nicht kann.
pub fn suspend(child: &Child, on: bool) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: gültiges Prozess-Handle des noch nicht abgeholten Kindes.
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
        // SAFETY: Signal an das eigene Kind.
        unsafe { kill(child.id() as i32, if on { stop } else { cont }) == 0 }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (child, on);
        false
    }
}

/// Verbrauchte CPU-Zeit (Benutzer + System) eines Kindprozesses, soweit ermittelbar.
pub fn cpu_time(child: &Child) -> Option<Duration> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let mut t = [0u64; 4];
        // SAFETY: vier FILETIME-Felder (je 64 Bit), gültiges Handle.
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
    }
    #[link(name = "ntdll")]
    extern "system" {
        pub fn NtSuspendProcess(h: Handle) -> i32;
        pub fn NtResumeProcess(h: Handle) -> i32;
    }

    /// Job-Objekt mit „kill on job close“: Das Handle lebt bis zum Prozessende, schließt
    /// Windows es (auch beim harten Beenden), enden alle zugeordneten FFmpeg-Prozesse.
    fn job() -> Option<usize> {
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

    /// Ordnet alle laufenden FFmpeg/FFprobe-Kinder dieses Prozesses dem Job zu. Nur diese:
    /// andere Kinder (Installer der Aktualisierung, Browser) sollen die App überleben.
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

    /// Ein FFmpeg wird gleich gestartet: der Wächter-Thread sucht in den nächsten 1,5 s
    /// mehrmals nach neuen Kindern (sofort, dann seltener) und ordnet sie dem Job zu.
    /// shortcut: Wer in den ersten Millisekunden nach dem Start hart beendet wird, kann noch
    /// eine Waise hinterlassen; ein direktes Zuordnen bräuchte `raw_attribute` (noch instabil).
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

    /// Abstand der Suchläufe: kurz nach einem Start dicht, danach seltener.
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
    /// Wiedergabe-Prozesse laufen über normaler Priorität (Windows); anderswo bleibt es still bei normal.
    #[test]
    fn playback_processes_can_be_raised() {
        if !crate::ffmpeg_available() {
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

    #[cfg(windows)]
    #[test]
    fn scan_is_dense_right_after_a_start() {
        use std::time::Duration;
        let p = super::win::scan_pause;
        assert!(p(Duration::ZERO) < p(Duration::from_millis(100)));
        assert!(p(Duration::from_millis(100)) < p(Duration::from_secs(1)));
    }

    /// Hilfsprozess für `ffmpeg_dies_with_its_owner`: startet ein endloses FFmpeg, meldet
    /// dessen PID und wartet dann für immer.
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
        // direkt schreiben: `println!` hielte der Test-Rahmen bis zum Ende zurück
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "PID={}", c.id());
        let _ = o.flush();
        let _ = c.wait();
    }

    /// Hartes Beenden des Besitzers (TerminateProcess) beendet auch sein FFmpeg.
    #[cfg(windows)]
    #[test]
    fn ffmpeg_dies_with_its_owner() {
        use std::io::BufRead;
        if !crate::ffmpeg_available() {
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
            .expect("Hilfsprozess meldet keine PID");
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        }
        // SAFETY: SYNCHRONIZE-Handle auf das FFmpeg, vor dem Beenden des Besitzers geöffnet
        let h = unsafe { OpenProcess(0x0010_0000, 0, pid) };
        assert!(!h.is_null(), "FFmpeg {pid} läuft nicht");
        helper.kill().unwrap();
        let _ = helper.wait();
        let r = unsafe { super::win::WaitForSingleObject(h, 5000) };
        unsafe { CloseHandle(h) };
        assert_eq!(r, 0, "FFmpeg lebt nach dem Ende seines Besitzers weiter");
    }
}
