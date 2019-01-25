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

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(unused_imports))]

use core::panic::PanicInfo;

use cpuio::Port;

mod block;
mod fat;
mod mem;
mod part;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
/// Output the message provided in `message` over the serial port
fn serial_message(message: &str) {
    let mut serial: Port<u8> = unsafe { Port::new(0x3f8) };
    for c in message.chars() {
        serial.write(c as u8);
    }
}

#[cfg(not(test))]
/// Reset the VM via the keyboard controller
fn i8042_reset() -> ! {
    loop {
        let mut good: u8 = 0x02;
        let mut i8042_command: Port<u8> = unsafe { Port::new(0x64) };
        while good & 0x02 > 0 {
            good = i8042_command.read();
        }
        i8042_command.write(0xFE);
    }
}

#[cfg(not(test))]
/// Setup page tables to provide an identity mapping over the full 4GiB range
fn setup_pagetables() {
    let pte = mem::MemoryRegion::new(0xb000, 2048 * 8);
    for i in 0..2048 {
        pte.io_write_u64(i * 8, (i << 21) + 0x83u64)
    }

    let pde = mem::MemoryRegion::new(0xa000, 4096);
    for i in 0..4 {
        pde.io_write_u64(i * 8, (0xb000u64 + (0x1000u64 * i)) | 0x03);
    }

    serial_message("Page tables setup\n");
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    serial_message("Starting..\n");

    setup_pagetables();

    let mut device = block::VirtioMMIOBlockDevice::new(0xd0000000u64);
    match device.init() {
        Err(_) => serial_message("Error configuring block device\n"),
        Ok(_) => serial_message("Virtio block device configured\n"),
    }

    match part::find_efi_partition(&mut device) {
        Ok((start, end)) => {
            serial_message("Found EFI partition\n");
            let mut f = fat::Filesystem::new(&mut device, start);
            match f.init() {
                Ok(()) => {
                    serial_message("Filesystem ready\n");
                    let (ftype, cluster) = f.directory_find_at_root("EFI").unwrap();
                    let (ftype, cluster) = f.directory_find_at_cluster(cluster, "BOOT").unwrap();
                    let (ftype, cluster) =
                        f.directory_find_at_cluster(cluster, "BOOTX64 EFI").unwrap();
                    if cluster > 0 {
                        serial_message("Found bootloader (BOOTX64.EFI)\n")
                    }
                }
                Err(e) => serial_message("Failed to create filesystem\n"),
            }
        }
        Err(e) => serial_message("Failed to find EFI partition\n"),
    }

    i8042_reset()
}
