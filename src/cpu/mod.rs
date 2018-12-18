mod decode;

use self::decode::DecodeInstruction;
use self::decode::DecodedArmInstruction;
use scheduler::GeneratorTask;
use scheduler::Task;
use system::AccessWidth;
use system::Bus;
use system::MemoryRequest;
use system::OperationType;

// Named constants for common registers
const LR: usize = 14;
const PC: usize = 15;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct Cpsr(u32);

macro_rules! flag_field {
    ($getter:ident, $setter:ident, $bit:expr) => {
        fn $getter(&self) -> bool {
            self.0 & (1 << $bit) != 0
        }

        fn $setter(&mut self, c: bool) {
            if c {
                self.0 |= (1 << $bit);
            } else {
                self.0 &= !(1 << $bit);
            }
        }
    };
}

impl Cpsr {
    flag_field!(negative, set_negative, 31);
    flag_field!(zero, set_zero, 30);
    flag_field!(carry, set_carry, 29);
    flag_field!(overflow, set_overflow, 29);
}

struct ArmCpu {
    regs: [u32; 16],
    cpsr: Cpsr,

    // Fetch stage
    f_out_instr: u32,
    sequential_fetch: bool,

    // Decode stage
    d_out_instr: u32,

    // Execute stage
    e_out_pc: u32,
    refill_steps: u32,
}

fn decode_immediate(imm: u8, rotate: u8, carry_in: bool) -> (u32, bool) {
    let result = (imm as u32).rotate_right(rotate as u32 * 2);
    let carry_out = if rotate == 0 {
        carry_in
    } else {
        bit!(result[31]) != 0
    };
    (result, carry_out)
}

fn has_signed_overflow(x: u32, y: u32, r: u32) -> bool {
    ((x ^ r) & (y ^ r)) & (1 << 31) != 0
}

fn add_with_carry(op1: u32, op2: u32, carry_in: bool) -> (u32, bool, bool) {
    let result = op1 as u64 + op2 as u64 + carry_in as u64;
    let carry = result & (1 << 32) != 0;
    (
        result as u32,
        carry,
        has_signed_overflow(op1, op2, result as u32),
    )
}

fn alu_operation(
    opcode: u8,
    op1: u32,
    op2: u32,
    shifter_carry: bool,
    mut cpsr: Cpsr,
) -> (u32, Cpsr) {
    let result = match opcode {
        // AND, TST
        0 | 8 => {
            cpsr.set_carry(shifter_carry);
            op1 & op2
        }
        // EOR, TEQ
        1 | 9 => {
            cpsr.set_carry(shifter_carry);
            op1 ^ op2
        }
        // SUB, CMP
        2 | 10 => {
            let (x, not_c) = op1.overflowing_sub(op2);
            cpsr.set_carry(!not_c);
            cpsr.set_overflow(has_signed_overflow(op1, op2, x));

            let (x2, c2, v2) = add_with_carry(op1, !op2, true);
            assert_eq!(x, x2);
            assert_eq!(cpsr.carry(), c2);
            assert_eq!(cpsr.overflow(), v2);

            x
        }
        // RSB
        3 => {
            let (x, not_c) = op2.overflowing_sub(op1);
            cpsr.set_carry(!not_c);
            cpsr.set_overflow(has_signed_overflow(op2, op1, x));

            let (x2, c2, v2) = add_with_carry(op2, !op1, true);
            assert_eq!(x, x2);
            assert_eq!(cpsr.carry(), c2);
            assert_eq!(cpsr.overflow(), v2);

            x
        }
        // ADD, CMN
        4 | 11 => {
            let (x, c) = op1.overflowing_add(op2);
            cpsr.set_carry(c);
            cpsr.set_overflow(has_signed_overflow(op1, op2, x));
            x
        }
        // ADC
        5 => {
            let (x, c, v) = add_with_carry(op1, op2, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // SBC
        6 => {
            let (x, c, v) = add_with_carry(op1, !op2, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // RSC
        7 => {
            let (x, c, v) = add_with_carry(op2, !op1, cpsr.carry());
            cpsr.set_carry(c);
            cpsr.set_overflow(v);
            x
        }
        // ORR
        12 => {
            cpsr.set_carry(shifter_carry);
            op1 | op2
        }
        // MOV
        13 => {
            cpsr.set_carry(shifter_carry);
            op2
        }
        // BIC
        14 => {
            cpsr.set_carry(shifter_carry);
            op1 & !op2
        }
        // MVN
        15 => {
            cpsr.set_carry(shifter_carry);
            !op2
        }
        _ => unreachable!(),
    };

    cpsr.set_negative(bit!(result[31]) != 0);
    cpsr.set_zero(result == 0);

    (result, cpsr)
}

impl ArmCpu {
    fn new() -> ArmCpu {
        ArmCpu {
            regs: [0; 16],
            cpsr: Cpsr(0),

            f_out_instr: 0xFFFFFFFF,
            sequential_fetch: false,

            d_out_instr: 0xFFFFFFFF,

            e_out_pc: 0,
            refill_steps: 2,
        }
    }

    fn step(&mut self, bus: &Bus) {
        if bus.should_cpu_wait() {
            return;
        }

        self.step_fetch_or_single_instruction(bus);
    }

    fn step_fetch_or_single_instruction(&mut self, bus: &Bus) {
        // Pre-read
        let d_in_instr = bus.data.get();
        let e_in_instr = self.d_out_instr;

        println!(
            "-[${:X}]-> F -[{:08X}]-> D -[{:08X}]-> E",
            self.regs[PC], d_in_instr, e_in_instr
        );

        // Fetch stage
        println!("Fetching from PC={:X}", self.regs[PC]);
        bus.make_request(MemoryRequest {
            address: self.regs[PC],
            width: AccessWidth::Bit32,
            op: OperationType::Read {
                is_instruction: true,
            },
            seq: self.sequential_fetch,
        });
        self.sequential_fetch = true;

        // Decode stage
        self.d_out_instr = d_in_instr;

        // Execute stage
        let mut new_pc = None;

        if self.refill_steps > 0 {
            // Maybe handle refill_steps with special states instead
            println!("Skipping execute");
            self.refill_steps -= 1;
        } else {
            println!("Executing {:08X}", e_in_instr);
            // TODO: Handle condition
            let decoded_instr = DecodedArmInstruction::decode_arm_instruction(e_in_instr);
            match decoded_instr {
                DecodedArmInstruction::DataProcessingImmediate {
                    cond,
                    opcode,
                    s,
                    rn,
                    rd,
                    rotate,
                    imm,
                } => {
                    let (imm_value, imm_carry) = decode_immediate(imm, rotate, self.cpsr.carry());
                    // TODO: Some stuff about SPSR
                    let (result, new_cpsr) = alu_operation(
                        opcode,
                        self.regs[rn as usize],
                        imm_value,
                        imm_carry,
                        self.cpsr,
                    );

                    // TODO: Handle flags update
                    // TODO: Handle PC writes

                    match opcode {
                        // TST, TEQ, CMP, CMN
                        8 | 9 | 10 | 11 => (),
                        _ => self.regs[rd as usize] = result,
                    }
                }
                DecodedArmInstruction::BranchImm { cond, link, offset } => {
                    if link {
                        self.regs[LR] = self.regs[PC].wrapping_sub(4);
                    }
                    // TODO: Handle faulting on bad address
                    new_pc = Some(self.regs[PC].wrapping_add((offset * 4) as u32));
                    println!("Branching to PC={:0X}", new_pc.unwrap());
                }
                instr => unimplemented!("Unimplemented instruction execute: {:?}", instr),
            }
        }

        if let Some(new_pc) = new_pc {
            self.sequential_fetch = false;
            self.refill_steps = 2;
            self.regs[PC] = new_pc;
        } else {
            self.regs[PC] = self.regs[PC].wrapping_add(4);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::pin::Pin;

    fn step(
        cpu: &mut ArmCpu,
        bus: &Bus,
        cycle_type: char,
        operation: char,
        bits: i32,
        address: u32,
        val: u32,
    ) {
        let seq = match cycle_type {
            'N' => false,
            'S' => true,
            x => panic!("Invalid cycle_type: {}", x),
        };

        let op = match operation {
            'R' => OperationType::Read {
                is_instruction: false,
            },
            'W' => OperationType::Write,
            'O' => OperationType::Read {
                is_instruction: true,
            },
            x => panic!("Invalid operation type: {}", x),
        };

        let width = match bits {
            8 => AccessWidth::Bit8,
            16 => AccessWidth::Bit16,
            32 => AccessWidth::Bit32,
            x => panic!("Invalid width: {}", x),
        };

        cpu.step(&bus);
        assert_eq!(
            bus.request.get(),
            Some(MemoryRequest {
                address,
                width,
                op,
                seq,
            })
        );
        bus.data.set(val);
    }

    fn step_i(cpu: &mut ArmCpu, bus: &Bus, cycle_type: char) {
        match cycle_type {
            'I' => (),
            x => panic!("Invalid cycle_type: {}", x),
        };

        cpu.step(&bus);
        assert_eq!(bus.request.get(), None);
    }

    #[test]
    fn test_mov() {
        let bus = Default::default();
        let mut cpu = ArmCpu::new();

        // mov r0, #0x0800'0000
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000000, 0xE3A00302);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000004, 0xFFFFFFFF);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000008, 0xFFFFFFFF);
        assert_eq!(cpu.regs[0], 0x0800_0000);
    }

    #[test]
    fn test_branch() {
        let bus = Default::default();
        let mut cpu = ArmCpu::new();

        // b loc_0020
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000000, 0xEA000006);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000004, 0xFFFFFFFF);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000008, 0xFFFFFFFF);
        // mov r0, #0x0800'0000
        step(&mut cpu, &bus, 'N', 'O', 32, 0x00000020, 0xE3A00302);
        step(&mut cpu, &bus, 'S', 'O', 32, 0x00000024, 0xFFFFFFFF);
    }
}
