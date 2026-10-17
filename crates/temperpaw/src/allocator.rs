//! jemalloc's built-in options (its `malloc_conf` C string).
//!
//! By default freed pages are purged only as the same arena keeps allocating,
//! so a burst's high-water stays resident. Here dirty pages decay within a
//! second and muzzy pages at once. On Linux a background thread does the
//! purging; jemalloc supports those threads only there (on macOS asking for
//! one leaves the allocator half-initialized). `_RJEM_MALLOC_CONF` in the
//! environment is read after this and still overrides it.

/// A pointer to jemalloc's options string, laid out as the `const char *` it reads.
#[repr(transparent)]
pub struct JemallocConf(*const std::ffi::c_char);

// SAFETY: the pointer is to a 'static C string literal that is never written.
unsafe impl Sync for JemallocConf {}

/// Read by jemalloc at startup under its prefixed name.
#[cfg(target_os = "linux")]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: JemallocConf =
    JemallocConf(c"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0".as_ptr());

/// Read by jemalloc at startup under its prefixed name.
#[cfg(not(target_os = "linux"))]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: JemallocConf =
    JemallocConf(c"dirty_decay_ms:1000,muzzy_decay_ms:0".as_ptr());

/// jemalloc's own byte counts, recorded once a minute beside the process's
/// resident memory. When resident memory rises, `allocated` says whether the
/// program holds more, and `resident` minus `active` plus `retained` how much
/// the allocator keeps that the program has freed.
pub fn spawn_allocator_metrics_loop() {
    use tikv_jemalloc_ctl::{epoch, stats};

    let meter = opentelemetry::global::meter("temperpaw.allocator");
    let gauge = |name: &'static str, description: &'static str| {
        meter
            .u64_gauge(name)
            .with_unit("By")
            .with_description(description)
            .build()
    };
    let counts = [
        (
            gauge("jemalloc_allocated_bytes", "Bytes the program holds."),
            stats::allocated::read as fn() -> tikv_jemalloc_ctl::Result<usize>,
        ),
        (
            gauge(
                "jemalloc_active_bytes",
                "Bytes in pages holding the program's allocations.",
            ),
            stats::active::read,
        ),
        (
            gauge(
                "jemalloc_resident_bytes",
                "Bytes jemalloc keeps resident, its metadata included.",
            ),
            stats::resident::read,
        ),
        (
            gauge(
                "jemalloc_retained_bytes",
                "Bytes jemalloc keeps mapped but returned to the system.",
            ),
            stats::retained::read,
        ),
        (
            gauge(
                "jemalloc_metadata_bytes",
                "Bytes of jemalloc's own bookkeeping.",
            ),
            stats::metadata::read,
        ),
    ];
    tokio::spawn(async move {
        // determinism-ok: background task for process health metrics
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            // The counts are a snapshot that only moves when the epoch does.
            if epoch::advance().is_err() {
                continue;
            }
            for (gauge, read) in &counts {
                if let Ok(bytes) = read() {
                    gauge.record(bytes as u64, &[]);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use tikv_jemalloc_ctl::{epoch, stats};

    fn allocated() -> usize {
        epoch::advance().expect("epoch advances");
        stats::allocated::read().expect("jemalloc keeps stats")
    }

    #[test]
    fn allocated_bytes_follow_what_the_program_holds() {
        let before = allocated();
        let held = vec![1_u8; 64 * 1024 * 1024];
        let holding = allocated();
        assert!(
            holding >= before + held.len(),
            "allocated {before} then {holding} while holding {} bytes",
            held.len()
        );
        drop(held);
        assert!(allocated() < holding);
    }
}
