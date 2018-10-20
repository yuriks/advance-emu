macro_rules! bit {
    ($data:ident[$base:literal : $limit:literal]) => (bit!($data[$base; $limit - $base + 1]));
    ($data:ident[$bit:expr]) => (($data >> $bit) & 1);
    ($data:ident[$base:expr; $len:expr]) => (($data >> $base) & (1 << $len) - 1);
}
