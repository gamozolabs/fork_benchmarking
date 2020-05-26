#![feature(llvm_asm)]

use std::fs::{File, OpenOptions};
use std::os::unix::io::IntoRawFd;
use std::sync::atomic::{AtomicU64, AtomicPtr, Ordering};
use std::collections::{BTreeSet, HashSet};
use libc::*;

pub mod threading;

/// Statistics for syncing between children in shared memory
#[derive(Default, Debug)]
struct Statistics {
    vm_cycles: AtomicU64,

    /// Number of "workers" currently "fuzzing"
    workers: AtomicU64,
}

/// Location where shared memory was mapped
static SHARED_MEMORY: AtomicPtr<Statistics> =
    AtomicPtr::new(core::ptr::null_mut());

/// Create shared memory to be used for communication of statistics between
/// children and the parent threads
unsafe fn create_shared_memory() {
    // Create a new file to use for the shared memory backing
    let fd = OpenOptions::new().create(true).read(true).write(true)
        .truncate(true).open("shared_memory").unwrap();
    fd.set_len(core::mem::size_of::<Statistics>() as u64).unwrap();

    // Map in the shared memory
    let ret = mmap(core::ptr::null_mut(), core::mem::size_of::<Statistics>(),
        PROT_READ | PROT_WRITE, MAP_SHARED, File::into_raw_fd(fd), 0);
    assert!(ret != MAP_FAILED);

    // Initialize the memory to default values
    core::ptr::write_volatile(ret as *mut Statistics, Statistics::default());

    // Store the address of the shared memory allocation
    SHARED_MEMORY.store(ret as *mut Statistics, Ordering::SeqCst);
}

/// Get access to the shared memory structure
/// Technically not safe cause it could be !Sync (eg. contains a `Cell`)
unsafe fn shared_memory() -> &'static Statistics {
    let sm = SHARED_MEMORY.load(Ordering::SeqCst);
    assert!(!sm.is_null());
    &*sm
}

/// Reset shared memory to the default values
unsafe fn reset_shared_memory() {
    let sm = SHARED_MEMORY.load(Ordering::SeqCst);
    assert!(!sm.is_null());
    *sm = Statistics::default();
}

fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn main() {
    /// Number of samples to have over the thread range (logscale)
    const THREAD_SAMPLES: usize = 32;

    /// Number of samples to have over the workload range (logscale)
    const WORKLOAD_SAMPLES: usize = 100;

    /// Maximum number of threads to test
    const MAX_THREADS: usize = 192;

    /// Maximum workload to sample to
    const MAX_WORKLOAD: usize = 1000000;

    // Create shared memory
    unsafe { create_shared_memory(); }

    // Get access to shared memory
    let shmem = unsafe { shared_memory() };

    // Map for children
    let mut children = HashSet::new();

    // Determine the scaling multipliers to hit the max values using the
    // number of samples requested
    let thrscale = (MAX_THREADS as f64 ).powf(1. / THREAD_SAMPLES as f64);
    let wlscale  = (MAX_WORKLOAD as f64).powf(1. / WORKLOAD_SAMPLES as f64);

    // Determine all the tests we should run. This will dedup any duplicate
    // tests
    let mut tests = BTreeSet::new();
    let mut threads = 1.0;
    while (threads as usize) < MAX_THREADS {
        // Capture the number of threads to use this test
        let num_threads = threads as u64;

        // Update the threads by the multiplier
        threads *= thrscale;
    
        let mut target_workload = 1.0;
        while (target_workload as usize) < MAX_WORKLOAD {
            // Capture the workload
            let workload = target_workload as u64;

            // Update the workload by the multiplier
            target_workload *= wlscale;

            // Log that we want to run a test with this number of threads
            // and the supplied workload
            tests.insert((num_threads, workload));
        }
    }

    // Run all the tests!
    for &(num_threads, workload) in tests.iter() {
        // No children should be running at this point
        assert!(children.len() == 0);

        // Reset statistics
        unsafe { reset_shared_memory(); }

        // Start a rdtsc-based timer too
        let start_cycles = rdtsc();

        // Create children while we're not at our target number of
        // children
        for thr_id in 0..num_threads {
            // Fork to make a child
            let child = unsafe { fork() };
            assert!(child != -1);

            if child == 0 {
                // We're the child

                // Pin to a specific processor
                threading::pin_to_logical_processor(thr_id as usize);
              
                // Wait for all worker threads to be started, this ensures
                // all threads start forking rnougly at the same time
                // (within the time that the `workers` variable gets
                // cache-coherencied across all cores. This will make sure
                // that any expensive jitter caused by forking in the
                // kernel will not be part of the benchmark. This also
                // ensures that the threads are all running at the same
                // time rather than straddled
                shmem.workers.fetch_add(1, Ordering::SeqCst);
                while shmem.workers.load(Ordering::SeqCst) !=
                    num_threads {}
                
                let timeout = rdtsc() + 1_000_000_000;

                while rdtsc() < timeout {
                    let subchild = unsafe { fork() };
                    assert!(subchild != 1);

                    if subchild == 0 {
                        let it = rdtsc();
                        unsafe {
                            llvm_asm!(r#"

                                test rcx, rcx
                                jz   3f

                                mov rax, rcx
                            2:
                            .rept 16
                                mov rdx, [rsp]
                            .endr

                                dec rax
                                jnz 2b

                            3:

                            "# :: "{rcx}"(workload) : "rax", "rdx" :
                            "intel", "volatile");
                        }
                        let elapsed = rdtsc() - it;

                        shmem.vm_cycles.fetch_add(elapsed,
                                                  Ordering::Relaxed);
                
                        // Done
                        unsafe { exit(0); }
                    } else {
                        // Wait for the subchild to exit
                        assert!(unsafe {
                            waitpid(subchild, core::ptr::null_mut(), 0)
                        } == subchild);
                    }
                }

                // We're done working
                shmem.workers.fetch_sub(1, Ordering::SeqCst);
                
                // Done entirely on this thread
                unsafe { exit(0); }
            } else {
                // Log the PID of the child we just spawned
                children.insert(child);
            }
        }

        // Wait for all children to exit
        children.retain(|&pid| {
            unsafe {
                waitpid(pid, core::ptr::null_mut(), 0) != pid
            }
        });

        // All children are done, log number of cycles
        let elapsed_cycles = rdtsc() - start_cycles;

        // Just make sure all workers are "done", this should never happen
        // unless we broke something
        assert!(shmem.workers.load(Ordering::SeqCst) == 0);

        print!("{:10} {:14} {:12.6}\n",
               num_threads,
               workload * (16 + 2),
               shmem.vm_cycles.load(Ordering::Relaxed) as f64 /
               (elapsed_cycles as f64 * num_threads as f64));
    }    
}

