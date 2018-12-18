use std::cell::Cell;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AccessWidth {
    Bit8,
    Bit16,
    Bit32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum OperationType {
    Read { is_instruction: bool },
    Write,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct MemoryRequest {
    pub address: u32,
    pub width: AccessWidth,
    pub op: OperationType,
    pub seq: bool,
}

pub struct Bus {
    /// Active memory request. Set only by the CPU/DMA.
    pub request: Cell<Option<MemoryRequest>>,
    /// True if a device is still processing a request and the read/write hasn't completed yet.
    pub busy: Cell<bool>,
    /// Overrides `busy` when being read by the CPU, so that DMA can take over the bus.
    pub dma_active: Cell<bool>,
    /// Last value read/written on the bus. For writes, it is assumed that the data is properly
    /// mirrored across all 32 bits no matter the access width.
    pub data: Cell<u32>,
}

impl Bus {
    #[inline]
    pub fn make_request(&self, request: MemoryRequest) {
        self.request.set(Some(request));
    }

    #[inline]
    pub fn should_cpu_wait(&self) -> bool {
        self.busy.get() || self.dma_active.get()
    }

    #[inline]
    pub fn should_dma_wait(&self) -> bool {
        self.busy.get()
    }
}

impl Default for Bus {
    fn default() -> Bus {
        Bus {
            request: None.into(),
            busy: false.into(),
            dma_active: false.into(),
            data: 0xFFFFFFFF.into(),
        }
    }
}