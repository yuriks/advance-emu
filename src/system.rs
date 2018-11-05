use std::cell::Cell;

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum AccessWidth {
    Bit8,
    Bit16,
    Bit32,
}

#[derive(Copy, Clone)]
pub enum OperationType {
    Read { is_instruction: bool },
    Write,
}

#[derive(Copy, Clone)]
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
    fn make_request(&self, request: MemoryRequest) {
        assert!(self.request.get().is_none());
        self.request.set(Some(request));
    }

    #[inline]
    fn should_cpu_wait(&self) -> bool {
        self.busy.get() || self.dma_active.get()
    }

    #[inline]
    fn should_dma_wait(&self) -> bool {
        self.busy.get()
    }
}
