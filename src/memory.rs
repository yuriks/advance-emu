use byteorder::ByteOrder;
use byteorder::LE;
use scheduler::GeneratorTask;
use scheduler::Task;
use std::cell::Cell;
use std::rc::Rc;
use system::AccessWidth;
use system::Bus;
use system::OperationType;

/// Loose bits of memory not stored in other units
struct Memory {
    bios: Box<[u8; 16 * 1024]>,
    bios_unlocked: bool,
    last_bios_read: u32,

    ewram: Box<Cell<[u8; 256 * 1024]>>,
    iwram: Box<Cell<[u8; 32 * 1024]>>,

    palettes: Cell<[u16; 512]>,
    vram: Box<Cell<[u8; 96 * 1024]>>,
    oam: Cell<[u16; 128 * 4]>,

    cart_rom: Box<[u8]>,
    cart_sram: Box<Cell<[u8]>>,
}

#[inline(always)]
fn concat16(msb: u16, lsb: u16) -> u32 {
    (msb as u32) << 16 | lsb as u32
}

#[inline(always)]
fn mirror_16to32(x: u16) -> u32 {
    concat16(x, x)
}

#[inline(always)]
fn mirror_8to32(x: u8) -> u32 {
    let x = x as u32;
    x << 24 | x << 16 | x << 8 | x
}

fn do_iwram_rw32(
    data: &Cell<u32>,
    memory: &mut [u8],
    offset: u32,
    op: OperationType,
    width: AccessWidth,
) {
    match op {
        OperationType::Read { .. } => {
            // Read new value, then merge with previous one to simulate bus capacitance
            let read = LE::read_u32(&memory[(offset & !0b11) as usize..]);
            let mask = match width {
                AccessWidth::Bit8 => 0xFF << ((offset & 0b11) * 8),
                AccessWidth::Bit16 => 0xFFFF << ((offset & 0b10) * 8),
                AccessWidth::Bit32 => 0xFFFFFFFF,
            };
            data.set((data.get() & !mask) | (read & mask));
        }
        OperationType::Write => match width {
            AccessWidth::Bit8 => {
                memory[offset as usize] = data.get() as u8;
            }
            AccessWidth::Bit16 => {
                LE::write_u16(&mut memory[offset as usize..], data.get() as u16);
            }
            AccessWidth::Bit32 => {
                LE::write_u32(&mut memory[offset as usize..], data.get());
            }
        },
    }
}

fn do_ewram_rw16(
    data: &mut u16,
    memory: &mut [u8],
    offset: u32,
    op: OperationType,
    width: AccessWidth,
) {
    match op {
        OperationType::Read { .. } => {
            *data = LE::read_u16(&memory[(offset & !0b1) as usize..]);
        }
        OperationType::Write => {
            do_write16(memory, offset as usize, *data, width);
        }
    }
}

fn do_read_write16(data: &Cell<u32>, memory: &mut [u8], offset: u32, op: OperationType) {
    match op {
        OperationType::Read { .. } => {
            data.set(LE::read_u32(&memory[(offset & !0b11) as usize..]));
        }
        OperationType::Write => {
            LE::write_u16(&mut memory[offset as usize..], data.get() as u16);
        }
    }
}

fn do_write16(memory: &mut [u8], offset: usize, data: u16, width: AccessWidth) {
    match width {
        AccessWidth::Bit8 => memory[offset] = data as u8,
        AccessWidth::Bit16 | AccessWidth::Bit32 => {
            LE::write_u16(&mut memory[offset & !0b1..], data)
        }
    }
}

impl Memory {
    fn run_task(&mut self, bus: Rc<Bus>) -> impl Task<Return = ()> {
        GeneratorTask::new(move || {
            loop {
                if let Some(request) = bus.request.get() {
                    let address = request.address;

                    // Handle BIOS locking
                    if let OperationType::Read {
                        is_instruction: true,
                    } = request.op
                    {
                        // TODO: Need to confirm range for this check
                        self.bios_unlocked = address < 0x4000;
                    }

                    match bit!(address[24:31]) {
                        // BIOS
                        0x0 => {
                            if self.bios_unlocked {
                                let offset = request.address & 0x3FFC;
                                self.last_bios_read = LE::read_u32(&self.bios[offset as usize..]);
                            }
                            bus.data.set(self.last_bios_read);
                        }
                        // TODO: 0x1 Unused, or BIOS?
                        // EWRAM
                        0x2 => {
                            bus.busy.set(true);
                            wait_cycles!(2);

                            let offset = request.address & 0x3FFFF;
                            let mut low_latch = bus.data.get() as u16;
                            let mut high_latch = (bus.data.get() >> 16) as u16;

                            do_ewram_rw16(
                                &mut low_latch,
                                self.ewram.get_mut(),
                                offset,
                                request.op,
                                request.width,
                            );
                            bus.data.set(mirror_16to32(low_latch));

                            if request.width == AccessWidth::Bit32 {
                                wait_cycles!(1 + 2);

                                // TODO: Is it XOR or OR? Even if it's not XOR, might be able to
                                // save some work by moving CPU-side rotation to here instead? No,
                                // that affects the open-bus behavior.
                                do_ewram_rw16(
                                    &mut high_latch,
                                    self.ewram.get_mut(),
                                    offset ^ 0b10,
                                    request.op,
                                    request.width,
                                );
                                bus.data.set(concat16(high_latch, low_latch));
                            }

                            bus.busy.set(false);
                        }
                        // IWRAM
                        0x3 => {
                            let offset = request.address & 0x7FFF;
                            do_iwram_rw32(
                                &bus.data,
                                self.iwram.get_mut(),
                                offset,
                                request.op,
                                request.width,
                            );
                        }
                        // I/O registers
                        0x4 => {}
                        // Palette RAM
                        0x5 => {}
                        // VRAM
                        0x6 => {}
                        // OAM
                        0x7 => {}
                        // Cart ROM mirrors
                        0x8..=0xD => {}
                        // Cart SRAM
                        0xE => {}
                        // TODO: 0xF Unused, or Cart SRAM?
                        _ => {}
                    }
                }
                wait_cycles!(1);
            }
        })
    }
}
