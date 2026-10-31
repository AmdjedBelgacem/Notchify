//! Process helpers: which processes are running, and which *app* a process
//! belongs to.
//!
//! Audio is often produced by helper processes (`Google Chrome Helper`,
//! `com.apple.WebKit.GPU`…). To show the app the user recognises we ask the
//! kernel for the process "responsible" for the helper and walk its executable
//! path up to the outermost `.app` bundle.
//!
//! `responsibility_get_pid_responsible_for_pid` is a private libsystem symbol.
//! It is resolved with `dlsym` so a future removal degrades to "no mapping"
//! instead of a launch failure.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Path of a process's executable.
pub fn executable_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: buffer is PROC_PIDPATHINFO_MAXSIZE bytes as required.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

fn responsible_fn() -> Option<ResponsibleFn> {
    static F: OnceLock<usize> = OnceLock::new();
    let addr = *F.get_or_init(|| {
        // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name.
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr()) as usize }
    });
    // SAFETY: the symbol has this signature in every macOS release that ships it.
    (addr != 0).then(|| unsafe { std::mem::transmute::<usize, ResponsibleFn>(addr) })
}

/// The process macOS holds responsible for `pid` (itself for normal apps).
pub fn responsible_pid(pid: i32) -> i32 {
    match responsible_fn() {
        // SAFETY: plain syscall wrapper taking a pid.
        Some(f) => {
            let r = unsafe { f(pid) };
            if r > 0 { r } else { pid }
        }
        None => pid,
    }
}

/// Every running process id.
pub fn all_pids() -> Vec<i32> {
    // SAFETY: a null buffer asks for the required count.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0i32; n as usize + 32];
    // SAFETY: buffer size passed in bytes.
    let n = unsafe {
        libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * std::mem::size_of::<i32>()) as libc::c_int)
    };
    pids.truncate(n.max(0) as usize);
    pids.retain(|p| *p > 0);
    pids
}

/// Short process name (`p_comm`, up to 32 characters).
pub fn name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 64];
    // SAFETY: buffer of the given length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// Is an app running? Filters by process name first (one cheap syscall per
/// process), then confirms with the executable path. Never launches the app.
pub fn is_app_running(process_name: &str, executable_suffix: &str) -> bool {
    all_pids()
        .into_iter()
        .filter(|p| name(*p).as_deref() == Some(process_name))
        .filter_map(executable_path)
        .any(|p| p.to_string_lossy().ends_with(executable_suffix))
}

/// The outermost `.app` bundle containing `path`, if any.
pub fn outer_app_bundle(path: &Path) -> Option<PathBuf> {
    let mut acc = PathBuf::new();
    for c in path.components() {
        acc.push(c);
        if acc.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            return Some(acc);
        }
    }
    None
}

/// Display name and bundle id read from an app bundle's Info.plist.
pub fn bundle_info(app: &Path) -> (String, Option<String>) {
    let fallback = app.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let Ok(plist::Value::Dictionary(d)) = plist::Value::from_file(app.join("Contents/Info.plist")) else {
        return (fallback, None);
    };
    let s = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_owned).filter(|s| !s.is_empty());
    let name = s("CFBundleDisplayName").or_else(|| s("CFBundleName")).unwrap_or(fallback);
    (name, s("CFBundleIdentifier"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_outermost_bundle() {
        let p = Path::new(
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper",
        );
        assert_eq!(outer_app_bundle(p), Some(PathBuf::from("/Applications/Google Chrome.app")));
        assert_eq!(outer_app_bundle(Path::new("/usr/sbin/coreaudiod")), None);
    }

    #[test]
    fn own_process_is_visible() {
        let me = std::process::id() as i32;
        assert!(all_pids().contains(&me));
        assert!(executable_path(me).is_some());
        assert!(responsible_pid(me) > 0);
    }
}
