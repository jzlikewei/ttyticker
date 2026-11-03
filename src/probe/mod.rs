//! 采集层：按平台分文件，对外只暴露一个 `Probe`。
//!
//! 每个平台把差异消化在自己的实现里，向上只吐 `Snapshot`。
//! catalog / candle / ui 都不认识平台，新增一个平台等于新增一个文件，
//! 上层一行不动。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

unsafe extern "C" {
    fn getpwuid_r(
        uid: u32,
        pwd: *mut std::ffi::c_void,
        buf: *mut u8,
        buflen: usize,
        result: *mut *mut std::ffi::c_void,
    ) -> i32;
}

/// 刷新进程列表。
///
/// 默认 refresh kind 既不含线程列表也不含属主，`Process::tasks()` 与
/// `Process::user_id()` 会一直是 None——席位列全是破折号就是这么来的。
/// 两边平台共用这一个函数，避免只改一边。
pub fn refresh_procs(sys: &mut sysinfo::System) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_tasks()
            // 属主在 exec 之后就定死了，读一次就够，之后不再重复 syscall。
            .with_user(UpdateKind::OnlyIfNotSet),
    );
}

/// 按 UID 把进程聚合成席位，按 CPU 占比从高到低。
///
/// 遍历进程时顺手累加，零额外系统调用；用户名走 `username` 的缓存。
pub fn aggregate_seats(procs: &[crate::snapshot::Proc]) -> Vec<crate::snapshot::Seat> {
    use crate::snapshot::Seat;
    let mut map: HashMap<u32, Seat> = HashMap::new();
    for p in procs {
        let Some(uid) = p.uid else { continue };
        let s = map.entry(uid).or_insert_with(|| Seat {
            uid,
            name: username(uid),
            ..Default::default()
        });
        s.cpu_pct += p.cpu_pct;
        s.mem_bytes += p.mem_bytes;
        s.procs += 1;
    }
    let mut v: Vec<Seat> = map.into_values().collect();
    v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
    v
}

static USER_CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();

/// UID 翻译成用户名，结果缓存。
///
/// 用户表不会秒级变化，缓存命中率接近 100%，省掉每帧几百次 getpwuid_r。
pub fn username(uid: u32) -> String {
    let cache = USER_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(c) = cache.lock() {
        if let Some(n) = c.get(&uid) {
            return n.clone();
        }
    }
    let name = lookup_user(uid).unwrap_or_else(|| format!("uid{uid}"));
    if let Ok(mut c) = cache.lock() {
        c.insert(uid, name.clone());
    }
    name
}

fn lookup_user(uid: u32) -> Option<String> {
    // struct passwd 各平台布局不同，macOS 比 Linux 多 pw_change、pw_class、
    // pw_expire 三个字段。这里只读第一个字段 pw_name（两个平台都在开头），
    // 所以给一块足够大的原始缓冲区，避免内核写满整个结构时越界。
    let mut pwd = [0usize; 16];
    let mut buf = vec![0u8; 1024];
    let mut result: *mut std::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe {
        getpwuid_r(
            uid,
            pwd.as_mut_ptr().cast(),
            buf.as_mut_ptr(),
            buf.len(),
            (&mut result as *mut *mut std::ffi::c_void).cast(),
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let name = pwd[0] as *const std::ffi::c_char;
    if name.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Probe;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Probe;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("perfticker 目前只支持 macOS 与 Linux");
