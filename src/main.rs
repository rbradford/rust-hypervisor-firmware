// Copyright © 2019 Intel Corporation
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![feature(asm_const)]
#![feature(alloc_error_handler)]
#![feature(slice_take)]
#![feature(stdsimd)]
#![feature(stmt_expr_attributes)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(unused_imports, dead_code))]
#![cfg_attr(not(feature = "log-serial"), allow(unused_variables, unused_imports))]

use core::{arch::asm, panic::PanicInfo};

#[cfg(target_arch = "x86_64")]
use x86_64::instructions::hlt;

#[macro_use]
mod serial;

#[macro_use]
mod common;

mod arch;
mod block;
mod boot;
mod bootinfo;
mod bzimage;
#[cfg(target_arch = "x86_64")]
mod cmos;
mod coreboot;
mod delay;
mod efi;
mod fat;
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
mod fdt;
#[cfg(all(test, feature = "integration_tests"))]
mod integration;
mod layout;
mod loader;
mod mem;
mod part;
mod pci;
mod pe;
#[cfg(target_arch = "x86_64")]
mod pvh;
mod rtc;
#[cfg(target_arch = "aarch64")]
mod rtc_pl031;
#[cfg(target_arch = "riscv64")]
mod uart_mmio;
#[cfg(target_arch = "aarch64")]
mod uart_pl011;
mod virtio;

#[cfg(all(not(test), feature = "log-panic"))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log!("PANIC: {}", info);
    loop {
        #[cfg(target_arch = "x86_64")]
        hlt()
    }
}

#[cfg(all(not(test), not(feature = "log-panic")))]
#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

const VIRTIO_PCI_VENDOR_ID: u16 = 0x1af4;
const VIRTIO_PCI_BLOCK_DEVICE_ID: u16 = 0x1042;
fn boot_from_device(device: &mut block::VirtioBlockDevice, info: &dyn bootinfo::Info) -> bool {
    if let Err(err) = device.init() {
        log!("Error configuring block device: {:?}", err);
        return false;
    }
    log!(
        "Virtio block device configured. Capacity: {} sectors",
        device.get_capacity()
    );

    let (start, end) = match part::find_efi_partition(device) {
        Ok(p) => p,
        Err(err) => {
            log!("Failed to find EFI partition: {:?}", err);
            return false;
        }
    };
    log!("Found EFI partition");

    let mut f = fat::Filesystem::new(device, start, end);
    if let Err(err) = f.init() {
        log!("Failed to create filesystem: {:?}", err);
        return false;
    }
    log!("Filesystem ready");

    if false {
        #[cfg(target_arch = "riscv64")]
        {
            let mut file = f.open("/Image").unwrap();
            let mut l = pe::Loader::new(&mut file);
            log!("File opened");

            let load_addr = 0x8020_0000 + 0x20_0000;
            let (entry_addr, load_addr, size) = l.load(load_addr).unwrap();
            log!(
                "Loading kernel: load_addr = 0x{:x} entry_addr = 0x{:x} size = {}",
                load_addr,
                entry_addr,
                size
            );
            let hart = 0;
            let fdt_address = info.fdt_address().unwrap();
            log!(
                "Booting directly into kernel. HART: {} FDT address: 0x{:x}",
                hart,
                fdt_address
            );

            let ptr = load_addr;
            let code: extern "C" fn(u64, u64) = unsafe { core::mem::transmute(ptr) };
            (code)(hart, fdt_address);

            log!("Jumped to kernel");

            return true;
        }

        match loader::load_default_entry(&f, info) {
            Ok(mut kernel) => {
                log!("Jumping to kernel");
                kernel.boot();
                return true;
            }
            Err(err) => log!("Error loading default entry: {:?}", err),
        }
    }

    {
        log!("Using EFI boot.");
        #[cfg(target_arch = "aarch64")]
        let efi_boot_path = "/EFI/BOOT/BOOTAA64.EFI";
        #[cfg(target_arch = "x86_64")]
        let efi_boot_path = "/EFI/BOOT/BOOTX64 EFI";
        #[cfg(target_arch = "riscv64")]
        let efi_boot_path = "/EFI/BOOT/BOOTRV64.EFI";

        let mut file = match f.open(efi_boot_path) {
            Ok(file) => file,
            Err(err) => {
                log!("Failed to load default EFI binary: {:?}", err);
                return false;
            }
        };
        log!("Found bootloader: {}", efi_boot_path);

        let mut l = pe::Loader::new(&mut file);
        #[cfg(target_arch = "aarch64")]
        let load_addr = arch::aarch64::layout::map::dram::KERNEL_START as u64;
        #[cfg(target_arch = "x86_64")]
        let load_addr = 0x20_0000;
        #[cfg(target_arch = "riscv64")]
        let load_addr = 0x8020_0000 + 0x20_0000;

        let (entry_addr, load_addr, size) = match l.load(load_addr) {
            Ok(load_info) => load_info,
            Err(err) => {
                log!("Error loading executable: {:?}", err);
                return false;
            }
        };

        log!(
            "EFI Executable loaded: entry_addr = 0x{:x}, load_addr = 0x{:x}",
            entry_addr,
            load_addr
        );
        efi::efi_exec(entry_addr, load_addr, size, info, &f, device);
    }
    true
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn rust64_start(#[cfg(not(feature = "coreboot"))] pvh_info: &pvh::StartInfo) -> ! {
    serial::PORT.borrow_mut().init();

    arch::x86_64::sse::enable_sse();
    arch::x86_64::paging::setup();

    #[cfg(feature = "coreboot")]
    let info = &coreboot::StartInfo::default();

    #[cfg(not(feature = "coreboot"))]
    let info = pvh_info;

    main(info)
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub extern "C" fn rust64_start(x0: *const u8) -> ! {
    serial::PORT.borrow_mut().init();

    arch::aarch64::simd::setup_simd();
    arch::aarch64::paging::setup();

    let info = fdt::StartInfo::new(x0);

    if let Some((base, length)) = info.find_compatible_region(&["pci-host-ecam-generic"]) {
        pci::init(base as u64, length as u64);
    }

    main(&info)
}

#[cfg(target_arch = "riscv64")]
#[no_mangle]
pub extern "C" fn rust64_start(a0: u64, a1: *const u8) -> ! {
    serial::PORT.borrow_mut().init();

    log!("Starting on RV64 0x{:x} 0x{:x} ", a0, a1 as u64);

    let info = fdt::StartInfo::new(a1);

    let mem = info.fdt.memory();
    for region in mem.regions() {
        log!(
            "Memory region {}MiB@0x{:x}",
            region.size.unwrap_or(0) / 1024 / 1024,
            region.starting_address as u64
        );
    }

    if let Some((base, length)) = info.find_compatible_region(&["pci-host-ecam-generic"]) {
        pci::init(base as u64, length as u64);
    }
    main(&info);

    log!("Shutting down");
    const RESET_BASE: u64 = 0x10_0000;
    // Safety: on this platform, a write of 0x5555 to 0x100000 will trigger the platform to
    // poweroff, which is defined behavior.
    unsafe {
        core::ptr::write_volatile(RESET_BASE as *mut u32, 0x5555);
    }
    loop {}
}

fn main(info: &dyn bootinfo::Info) -> ! {
    log!("\nBooting with {}", info.name());

    pci::print_bus();

    let mut next_address = 0x40000000;
    log!("Starting PCI address BARs at: 0x{:x}", next_address);

    pci::with_devices(
        VIRTIO_PCI_VENDOR_ID,
        VIRTIO_PCI_BLOCK_DEVICE_ID,
        |mut pci_device| {
            pci_device.init();
            next_address = pci_device.allocate_bars(next_address);
            pci_device.init();

            let mut pci_transport = pci::VirtioPciTransport::new(pci_device);
            let mut device = block::VirtioBlockDevice::new(&mut pci_transport);
            boot_from_device(&mut device, info)
        },
    );

    panic!("Unable to boot from any virtio-blk device")
}
