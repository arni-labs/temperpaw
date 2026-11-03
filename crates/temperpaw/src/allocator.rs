//! jemalloc's built-in options (its `malloc_conf` C string).
//!
//! By default freed pages are purged only as the same arena keeps allocating,
//! so a burst's high-water stays resident. Here dirty pages decay within a
//! second and muzzy pages at once. On Linux a background thread does the
//! purging; jemalloc supports those threads only there (on macOS asking for
//! one leaves the allocator half-initialized). `_RJEM_MALLOC_CONF` in the
//! environment is read after this and still overrides it.
//!
//! Also on Linux, jemalloc's memory is kept out of transparent huge pages.
//! Railway's hosts run THP "always" with khugepaged's max_ptes_none at 511, so
//! khugepaged fills a 2 MB range jemalloc has mostly purged back into one huge
//! page: resident memory grows while the program holds nothing more, and
//! jemalloc's own counts never see it.

/// A pointer to jemalloc's options string, laid out as the `const char *` it reads.
#[repr(transparent)]
pub struct JemallocConf(*const std::ffi::c_char);

// SAFETY: the pointer is to a 'static C string literal that is never written.
unsafe impl Sync for JemallocConf {}

/// Read by jemalloc at startup under its prefixed name.
#[cfg(target_os = "linux")]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: JemallocConf =
    JemallocConf(c"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0,thp:never".as_ptr());

/// Read by jemalloc at startup under its prefixed name.
#[cfg(not(target_os = "linux"))]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: JemallocConf =
    JemallocConf(c"dirty_decay_ms:1000,muzzy_decay_ms:0".as_ptr());

/// jemalloc's own byte counts, recorded once a minute beside the process's
/// resident memory. `allocated` counts live allocations and `active` the pages
/// holding them; `resident` includes jemalloc's bookkeeping, and `retained`
/// counts address space kept mapped, not resident memory. When resident memory
/// rises, `allocated` rising with it means the program holds more.
pub fn spawn_allocator_metrics_loop() {
    use tikv_jemalloc_ctl::{epoch, stats};

    type Read = fn() -> tikv_jemalloc_ctl::Result<usize>;
    let meter = opentelemetry::global::meter("temperpaw.allocator");
    let counts: [(&str, &str, Read); 5] = [
        (
            "jemalloc_allocated_bytes",
            "Bytes the program holds.",
            stats::allocated::read,
        ),
        (
            "jemalloc_active_bytes",
            "Bytes in pages holding the program's allocations.",
            stats::active::read,
        ),
        (
            "jemalloc_resident_bytes",
            "Bytes jemalloc keeps resident, its metadata included.",
            stats::resident::read,
        ),
        (
            "jemalloc_retained_bytes",
            "Address space jemalloc keeps mapped, not resident.",
            stats::retained::read,
        ),
        (
            "jemalloc_metadata_bytes",
            "Bytes of jemalloc's own bookkeeping.",
            stats::metadata::read,
        ),
    ];
    let counts = counts.map(|(name, description, read)| {
        let gauge = meter
            .u64_gauge(name)
            .with_unit("By")
            .with_description(description)
            .build();
        (name, gauge, read)
    });
    tokio::spawn(async move {
        // determinism-ok: background task for process health metrics
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            // The counts are a snapshot that only moves when the epoch does.
            if let Err(error) = epoch::advance() {
                tracing::warn!(%error, "jemalloc counts not refreshed");
                continue;
            }
            for (name, gauge, read) in &counts {
                match read() {
                    Ok(bytes) => gauge.record(bytes as u64, &[]),
                    Err(error) => tracing::warn!(gauge = name, %error, "jemalloc count not read"),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use tikv_jemalloc_ctl::{epoch, stats, thread};

    #[test]
    fn jemalloc_counts_what_the_program_holds() {
        // This thread's own totals, so tests running alongside cannot move them.
        let allocated = thread::allocatedp::read().expect("jemalloc keeps stats");
        let deallocated = thread::deallocatedp::read().expect("jemalloc keeps stats");
        let (allocated_before, deallocated_before) = (allocated.get(), deallocated.get());
        let held = vec![1_u8; 64 * 1024 * 1024];
        assert!(allocated.get() - allocated_before >= held.len() as u64);
        let len = held.len() as u64;
        drop(held);
        assert!(deallocated.get() - deallocated_before >= len);

        // And the process-wide counts the gauges read are there.
        epoch::advance().expect("epoch advances");
        assert!(stats::allocated::read().expect("jemalloc keeps stats") > 0);
    }
}
