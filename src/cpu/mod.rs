mod decode;

// Named constants for common registers
const LR: usize = 14;
const PC: usize = 15;

struct ArmCpu {
    regs: [u32; 16],
    cpsr: u32,
}
