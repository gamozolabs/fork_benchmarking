use std;

#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
#[allow(non_snake_case)]
struct PROCESSOR_NUMBER {
    Group:    u16,
    Number:   u8,
    Reserved: u8,
}

#[derive(Clone, Copy, Default, Debug)]
#[repr(C)]
#[allow(non_snake_case)]
struct GROUP_AFFINITY {
    Mask:     u64,
    Group:    u16,
    Reserved: [u16; 3],
}


#[cfg(target_os="windows")]
extern {
    fn GetNumaProcessorNodeEx(Processor: *const PROCESSOR_NUMBER,
                              NodeNumber: *mut u16) -> bool;

    fn GetCurrentThread() -> usize;

    fn SetThreadGroupAffinity(
        hThread: usize, GroupAffinity: GROUP_AFFINITY,
        PreviousGroupAffinity: *mut GROUP_AFFINITY) -> bool;
}

#[derive(Clone, Copy, Default, Debug)]
pub struct NumaInfo {
    procnum: PROCESSOR_NUMBER,
    numa_id: u16,
}

/// Pin the current thread to a specific logical processor
#[cfg(target_os="windows")]
pub fn pin_to_logical_processor(numainfo: NumaInfo)
{
    let group = GROUP_AFFINITY {
        Mask:     1u64 << numainfo.procnum.Number,
        Group:    numainfo.procnum.Group,
        Reserved: [0; 3],
    };

    unsafe {
        assert!(SetThreadGroupAffinity(GetCurrentThread(),
                                       group, std::ptr::null_mut()));
    }
}

#[cfg(target_os="linux")]
extern {
    fn sched_setaffinity(pid: usize, cpusetsize: usize,
        mask: *mut usize) -> i32;

    fn syscall(id: usize) -> i32;
}

#[cfg(target_os="linux")]
pub fn pin_to_logical_processor(core_id: usize) {
    unsafe {
        let mut bitmask = [0usize; 1024 / (std::mem::size_of::<usize>() * 8)];

        let usize_idx = core_id / (std::mem::size_of::<usize>() * 8);
        let bit_idx   = core_id % (std::mem::size_of::<usize>() * 8);

        // Set the affinity
        bitmask[usize_idx] |= 1 << bit_idx;

        let tid = syscall(186);
        assert!(tid > 0);

        assert!(sched_setaffinity(tid as usize,
            std::mem::size_of_val(&bitmask), bitmask.as_mut_ptr()) == 0,
            "Failed to pin to core");
    }
}

#[cfg(target_os="linux")]
pub fn get_logical_processors() -> Vec<NumaInfo> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo")
        .expect("Failed to read CPU info");

    let mut ret = Vec::new();
    for line in cpuinfo.lines() {
        if line.starts_with("processor") {
            ret.push(NumaInfo::default());
        }
    }

    ret
}

/// Get a list of all logical processors on the system
#[cfg(target_os="windows")]
pub fn get_logical_processors() -> Vec<NumaInfo>
{
    let mut ret = Vec::new();

    /* Support up to 64 groups, each group contains up to 64 logical
     * processors
     */
    for group in 0..64 {
        for number in 0..64 {
            let procnum = PROCESSOR_NUMBER {
                Group:    group,
                Number:   number,
                Reserved: 0,
            };

            if let Some(numa_id) = get_numa_node_id(procnum) {
                let ent = NumaInfo {
                    procnum,
                    numa_id,
                };

                ret.push(ent);
            }
        }
    }

    /* Should have found at least one processor */
    assert!(ret.len() >= 1);

    print!("{} logical processors detected\n", ret.len());

    ret
}

#[cfg(target_os="windows")]
fn get_numa_node_id(procnum: PROCESSOR_NUMBER) -> Option<u16>
{
    let mut ret = 0u16;

    unsafe {
        if GetNumaProcessorNodeEx(&procnum, &mut ret) == false {
            return None;
        }

        /* If the specified processor does not exist, this parameter
         * is set to MAXUSHORT */
        if ret == std::u16::MAX {
            None
        } else {
            Some(ret)
        }
    }
}
