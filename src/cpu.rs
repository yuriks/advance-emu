// Named constants for common registers
const LR: usize = 14;
const PC: usize = 15;

struct ArmCpu {
    regs: [u32; 16],
    cpsr: u32,
}

trait DecodeInstruction {
    fn decode_arm_instruction(instr: u32) -> Self;
}

//#[derive(DecodeInstruction)] TODO: Optimize with procedural macro later
#[derive(Debug, Eq, PartialEq)]
enum DecodedArmInstruction {
    DataProcessingImmediate {
        cond: u8,
        opcode: u8,
        s: bool,
        rn: u8,
        rd: u8,
        rotate: u8,
        imm: u8,
    },
    LoadStoreImmOffset {
        cond: u8,
        indexing_p: bool,
        imm_add: bool,
        byte: bool,
        indexing_w: bool,
        load: bool,
        rn: u8,
        rd: u8,
        imm: u16,
    },
    BranchImm {
        cond: u8,
        link: bool,
        offset: u32,
    },
    BranchAndExchangeReg {
        cond: u8,
        link: bool,
        rm: u8,
    },
    UndefinedInstruction,
    UnknownInstruction,
}

/// Tests instr against a bit pattern. Positions where format is '0' or '1' must have 0 or 1. Any
/// other character matches any bit, except for '_' which is skipped.
fn test(mut instr: u32, format: &'static [u8]) -> bool {
    assert_eq!(format.len(), 32 + 3);
    for c in format.iter().rev() {
        let bit = instr & 1;
        match c {
            b'0' if bit != 0 => return false,
            b'1' if bit != 1 => return false,
            b'_' => continue, // skip shifting instr
            _ => (),
        }
        instr >>= 1;
    }

    true
}

impl DecodeInstruction for DecodedArmInstruction {
    fn decode_arm_instruction(instr: u32) -> DecodedArmInstruction {
        use self::DecodedArmInstruction::*;

        // b"ccccxxxx_xxxxxxxx_xxxxxxxx_xxxxxxxx"
        let cond = bit!(instr[28:31]) as u8;

        // TEQ with S=0
        if test(instr, b"cccc0001_00101111_11111111_00l1mmmm") {
            return BranchAndExchangeReg {
                cond,
                link: bit!(instr[5]) != 0,
                rm: bit!(instr[0:3]) as u8,
            };
        }

        if test(instr, b"cccc001o_ooosnnnn_ddddrrrr_iiiiiiii") {
            return DataProcessingImmediate {
                cond,
                opcode: bit!(instr[21:24]) as u8,
                s: bit!(instr[20]) != 0,
                rn: bit!(instr[16:19]) as u8,
                rd: bit!(instr[12:15]) as u8,
                rotate: bit!(instr[8:11]) as u8,
                imm: bit!(instr[0:7]) as u8,
            };
        }

        if test(instr, b"cccc010p_ubwlnnnn_ddddiiii_iiiiiiii") {
            return LoadStoreImmOffset {
                cond,
                indexing_p: bit!(instr[24]) != 0,
                imm_add: bit!(instr[23]) != 0,
                byte: bit!(instr[22]) != 0,
                indexing_w: bit!(instr[21]) != 0,
                load: bit!(instr[20]) != 0,
                rn: bit!(instr[16:19]) as u8,
                rd: bit!(instr[12:15]) as u8,
                imm: bit!(instr[0:11]) as u16,
            }
        }

        if test(instr, b"cccc101l_iiiiiiii_iiiiiiii_iiiiiiii") {
            return BranchImm {
                cond,
                link: bit!(instr[24]) != 0,
                offset: bit!(instr[0:23]) as u32,
            };
        }

        UnknownInstruction
    }
}

mod tests {
    use super::*;

    #[test]
    fn decode_mov_imm() {
        let instr = 0xE3A00302; // mov r0, #134217728
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::DataProcessingImmediate {
            cond: 0b1110,
            opcode: 0b1101,
            s: false,
            rn: 0,
            rd: 0,
            rotate: 3,
            imm: 0x02,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn decode_cmp_imm() {
        let instr = 0xE35100EA; // cmp r1, #234
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::DataProcessingImmediate {
            cond: 0b1110,
            opcode: 0b1010,
            s: true,
            rn: 1,
            rd: 0,
            rotate: 0,
            imm: 234,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn decode_ldrb() {
        let instr = 0xE5D01003; // ldrb r1, [r0, #3]
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::LoadStoreImmOffset {
            cond: 0b1110,
            indexing_p: true,
            imm_add: true,
            byte: true,
            indexing_w: false,
            load: true,
            rn: 0,
            rd: 1,
            imm: 3,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn decode_str() {
        let instr = 0xE5800208; // str r0, [r0, #520]
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::LoadStoreImmOffset {
            cond: 0b1110,
            indexing_p: true,
            imm_add: true,
            byte: false,
            indexing_w: false,
            load: false,
            rn: 0,
            rd: 0,
            imm: 520,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn decode_b_imm() {
        let instr = 0xEA000006; // b $00000020
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::BranchImm {
            cond: 0b1110,
            link: false,
            offset: (0x20 - 8) / 4,
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn decode_bx_reg() {
        let instr = 0xE12FFF10; // bx r0
        let actual = DecodedArmInstruction::decode_arm_instruction(instr);
        let expected = DecodedArmInstruction::BranchAndExchangeReg {
            cond: 0b1110,
            link: false,
            rm: 0,
        };
        assert_eq!(actual, expected);
    }
}
