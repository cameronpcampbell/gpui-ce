//! Run with `cargo bench -p gpui-ce --features bench-support --bench reflection`.
//! Add `-- --allocations` or `-- --cold-metadata` for separate profiling runs.

use criterion::Criterion;
use gpui::reflection::benchmarks;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env,
    process::Command,
    time::Instant,
};

#[derive(Clone, Copy, Default)]
struct Allocations {
    count: usize,
    bytes: usize,
}

thread_local! {
    static ALLOCATIONS: Cell<Option<Allocations>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn record_allocation(bytes: usize) {
    let _recorded = ALLOCATIONS.try_with(|counter| {
        if let Some(mut allocations) = counter.get() {
            allocations.count += 1;
            allocations.bytes += bytes;
            counter.set(Some(allocations));
        }
    });
}

// Counters are enabled only by the allocation command, outside Criterion measurements.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());

        // SAFETY: Forward the caller's allocation contract to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());

        // SAFETY: Forward the caller's allocation contract to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The pointer and layout came from the system allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);

        // SAFETY: Forward the caller's reallocation contract to the system allocator.
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn profile(operation: &mut dyn FnMut()) -> Allocations {
    ALLOCATIONS.with(|counter| counter.set(Some(Allocations::default())));
    operation();

    ALLOCATIONS.with(|counter| counter.replace(None).unwrap())
}

fn configuration() {
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    let rustc = Command::new("rustc").arg("-V").output().unwrap();

    eprintln!(
        "revision={} dirty={}; {}",
        String::from_utf8_lossy(&revision.stdout).trim(),
        !status.stdout.is_empty(),
        String::from_utf8_lossy(&rustc.stdout).trim()
    );
    eprintln!(
        "target={}-{}; backend=none; bench-support=true; font-kit={}; wayland={}; x11={}; windows-manifest={}; debug_assertions={}",
        env::consts::ARCH,
        env::consts::OS,
        cfg!(feature = "font-kit"),
        cfg!(feature = "wayland"),
        cfg!(feature = "x11"),
        cfg!(feature = "windows-manifest"),
        cfg!(debug_assertions)
    );
}

fn cold_sample(kind: &str, allocations: bool) {
    // Prime the clock and optional counter without touching reflection metadata.
    let _clock = Instant::now();

    if allocations {
        let result = profile(&mut || benchmarks::cold_metadata(kind));

        println!("{} {}", result.count, result.bytes);

        return;
    }

    let start = Instant::now();
    benchmarks::cold_metadata(kind);

    println!("{}", start.elapsed().as_nanos());
}

fn cold_metadata(allocations: bool) {
    let executable = env::current_exe().unwrap();

    for kind in ["linked", "generic", "empty"] {
        let mut samples = Vec::new();
        let count = if allocations { 1 } else { 50 };

        for _sample in 0..count {
            let command = if allocations {
                "--cold-allocation-sample"
            } else {
                "--cold-sample"
            };
            let output = Command::new(&executable)
                .args([command, kind])
                .output()
                .unwrap();

            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );

            if allocations {
                println!(
                    "cold/{kind} allocations bytes: {}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );

                continue;
            }

            samples.push(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .trim()
                    .parse::<u128>()
                    .unwrap(),
            );
        }

        if samples.is_empty() {
            continue;
        }

        samples.sort_unstable();
        println!(
            "cold/{kind} n={} min={}ns p50={}ns p95={}ns max={}ns",
            samples.len(),
            samples[0],
            samples[25],
            samples[47],
            samples[49]
        );
    }
}

fn main() {
    let arguments: Vec<String> = env::args().collect();

    match arguments.get(1).map(String::as_str) {
        Some("--cold-sample" | "--cold-allocation-sample") => {
            cold_sample(&arguments[2], arguments[1] == "--cold-allocation-sample");

            return;
        }
        _argument => configuration(),
    }

    match arguments.get(1).map(String::as_str) {
        Some("--cold-metadata") => cold_metadata(false),
        Some("--allocations") => {
            benchmarks::profile_allocations(|name, operation| {
                let result = profile(operation);

                println!(
                    "warm/{name} allocations={} bytes={}",
                    result.count, result.bytes
                );
            });
            cold_metadata(true);
        }
        _argument => {
            let mut criterion = Criterion::default().configure_from_args();
            benchmarks::run(&mut criterion);
            criterion.final_summary();
        }
    }
}
