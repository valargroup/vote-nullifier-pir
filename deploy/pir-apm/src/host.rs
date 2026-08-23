use std::path::{Path, PathBuf};

use sysinfo::{Disks, System};

#[derive(Clone, Debug, Default)]
pub struct HostHealth {
    pub load_one: f64,
    pub load_five: f64,
    pub load_fifteen: f64,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_available_bytes: u64,
    pub disk_used_ratio: f64,
    pub data_dir: PathBuf,
}

pub fn collect(data_dir: &Path) -> HostHealth {
    let mut system = System::new();
    system.refresh_memory();
    let load = System::load_average();
    let disks = Disks::new_with_refreshed_list();
    let disk = disks
        .iter()
        .filter(|disk| data_dir.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count());
    let (disk_total_bytes, disk_available_bytes) = disk
        .map(|disk| (disk.total_space(), disk.available_space()))
        .unwrap_or((0, 0));
    let disk_used_ratio = if disk_total_bytes > 0 {
        1.0 - disk_available_bytes as f64 / disk_total_bytes as f64
    } else {
        0.0
    };

    HostHealth {
        load_one: load.one,
        load_five: load.five,
        load_fifteen: load.fifteen,
        total_memory_bytes: system.total_memory(),
        available_memory_bytes: system.available_memory(),
        disk_total_bytes,
        disk_available_bytes,
        disk_used_ratio,
        data_dir: data_dir.to_path_buf(),
    }
}
