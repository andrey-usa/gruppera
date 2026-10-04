// gruppera — high-performance station-temperature aggregator.
//
// Design notes: input is memory-mapped and split into 2MB chunks claimed
// via an atomic cursor. Each thread runs three interleaved row parsers to
// hide hash-table latency. Station names are hashed from their raw bytes;
// temperatures are parsed branchlessly with SWAR bit tricks into integer
// tenths, accumulated as integers, and formatted only at output.
//
// The techniques used here (mmap chunking, SWAR parsing, open-addressing
// tables, fixed-point accumulation) are standard practice in high-
// performance text aggregation, as demonstrated by the 1BRC challenge
// community, whose public write-ups informed this design.

use std::sync::atomic::{AtomicUsize, Ordering};

const CHUNK_BYTES: usize = 1 << 21;
const TABLE_BITS: usize = 17;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const TABLE_MASK: usize = TABLE_SIZE - 1;

/// Rows starting within TAIL_MARGIN bytes of EOF use the scalar parser.
/// The vectorized path issues 8-byte loads up to ~108 bytes past a row's
/// start (slow-path name fold + temperature word); with the official
/// generator's max 100-byte station names, 128 bytes of margin keeps every
/// load inside the mapping. This replaces UB (page-rounding luck) with a
/// provably in-bounds tail.
const TAIL_MARGIN: usize = 128;

/// One hash-table entry: exactly one 64-byte cache line (AoS layout, the
/// 1BRC winners' slot design). Field order matters — keep the 8-byte fields
/// first so no alignment padding pushes the struct to 128 bytes.
#[repr(C, align(64))]
struct Entry {
    hash: u64,
    fp0: u64, // masked first 8 name bytes (full name when len <= 16)
    fp1: u64, // masked next 8 name bytes
    name: *const u8, // points into the mmap; valid until unmap
    total: i64, // sum, in tenths
    lo: i32,   // min, in tenths
    hi: i32,   // max, in tenths
    n: u32,    // count
    name_len: u32,
    used: bool,
}

// Compile-time guard: the slot must stay a single cache line.
const _: () = assert!(std::mem::size_of::<Entry>() == 64);

impl Entry {
    fn vacant() -> Self {
        Entry {
            used: false,
            hash: 0,
            fp0: 0,
            fp1: 0,
            name: std::ptr::null(),
            name_len: 0,
            lo: 999,
            hi: -999,
            total: 0,
            n: 0,
        }
    }
}

/// Byte masks for isolating the low N bytes of a u64.
const LOW_MASK: [u64; 9] = [
    0x0000_0000_0000_00FF,
    0x0000_0000_0000_FFFF,
    0x0000_0000_00FF_FFFF,
    0x0000_0000_FFFF_FFFF,
    0x0000_00FF_FFFF_FFFF,
    0x0000_FFFF_FFFF_FFFF,
    0x00FF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
];

/// Cursor over the mapped input with unchecked 8-byte reads.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    stop: usize,
}

impl<'a> Cursor<'a> {
    #[inline(always)]
    fn live(&self) -> bool {
        self.at < self.stop
    }

    #[inline(always)]
    unsafe fn u64_at(&self, at: usize) -> u64 {
        (self.bytes.as_ptr().add(at) as *const u64).read_unaligned()
    }

    #[inline(always)]
    unsafe fn u64_here(&self) -> u64 {
        self.u64_at(self.at)
    }
}

/// SWAR test for the ';' byte in each lane of a u64.
#[inline(always)]
fn semi_mask(word: u64) -> u64 {
    let x = word ^ 0x3B3B_3B3B_3B3B_3B3B;
    x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080
}

/// SWAR test for '\n', used to align chunk edges.
#[inline(always)]
fn newline_mask(word: u64) -> u64 {
    let x = word ^ 0x0A0A_0A0A_0A0A_0A0A;
    x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080
}

/// Branchless parse of "[−]d[d].d" at the cursor (which sits on ';').
/// Returns tenths of a degree. Advances past "t.t\n".
#[inline(always)]
unsafe fn parse_tenths(c: &mut Cursor) -> i32 {
    // The 8 bytes starting one past ';' always hold the whole temperature:
    // layouts are "d.d", "dd.d", "-d.d", "-dd.d" (3..5 bytes).
    let w = c.u64_at(c.at + 1);
    // '.' (0x2E) is the only byte in 0..3 with bit 4 clear among digits.
    let dot_bit = ((!w) & 0x1010_1000).trailing_zeros();
    let shift = 28 - dot_bit;
    let neg_mask = (((!w) << 59) as i64 >> 63) as u64; // all-ones if '-'
    let no_sign = w & !(neg_mask & 0xFF);
    // Pack the three digit nibbles, then one multiply extracts 100*X+10*Y+Z.
    let nibbles = ((no_sign << shift) & 0x0F00_0F0F_00) as u128;
    let mag = (((nibbles * 0x640A_0001) >> 32) & 0x3FF) as i64;
    let signed = (mag ^ (neg_mask as i64)).wrapping_sub(neg_mask as i64);
    c.at += ((dot_bit >> 3) + 4) as usize;
    signed as i32
}

#[inline(always)]
fn index_of(hash: u64) -> usize {
    let h = hash ^ (hash >> 33) ^ (hash >> 15);
    (h as usize) & TABLE_MASK
}

/// Compare two byte strings via u64 chunks plus a masked tail.
#[inline(always)]
unsafe fn bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    let mut i = 0;
    while i + 8 <= len {
        if (a.add(i) as *const u64).read_unaligned() != (b.add(i) as *const u64).read_unaligned() {
            return false;
        }
        i += 8;
    }
    if i < len {
        let tail = len - i;
        // Loop exited with i+8 > len and i < len: tail in [1,7].
        let mask = *LOW_MASK.get_unchecked(tail);
        let am = (a.add(i) as *const u64).read_unaligned() & mask;
        let bm = (b.add(i) as *const u64).read_unaligned() & mask;
        if am != bm {
            return false;
        }
    }
    true
}

/// Locate the entry for the station name starting at the cursor.
/// The cursor is advanced past "name;". Returns the table index.
#[inline(always)]
unsafe fn lookup(c: &mut Cursor, table: &mut [Entry]) -> usize {
    let start = c.at;
    let w0 = c.u64_here();
    let m0 = semi_mask(w0);
    let w1 = c.u64_at(c.at + 8);
    let m1 = semi_mask(w1);

    // Fast path: ';' within the first 16 bytes. Mask the name words so the
    // cached pair fully identifies the name; hash is their xor.
    if (m0 | m1) != 0 {
        let n0 = (m0.trailing_zeros() >> 3) as usize;
        let second_active = if n0 == 8 { !0u64 } else { 0u64 };
        let n1 = (m1.trailing_zeros() >> 3) as usize;
        // n0, n1 in [0,8] by construction (trailing_zeros of u64, >> 3);
        // LOW_MASK has 9 entries, so unchecked is safe.
        let a = w0 & *LOW_MASK.get_unchecked(n0);
        let b = second_active & w1 & *LOW_MASK.get_unchecked(n1);
        let h = a ^ b;
        let len = n0 + ((n1 as u64 & second_active) as usize);
        c.at += len; // leave cursor ON ';' for parse_tenths

        let mut idx = index_of(h);
        {
            // idx < TABLE_SIZE == table.len() by index_of's mask.
            let e = &*table.get_unchecked(idx);
            // For len <= 16 the masked pair IS the whole name.
            if e.used && e.fp0 == a && e.fp1 == b && e.name_len as usize == len {
                return idx;
            }
        }
        idx = insert_probe(table, c.bytes.as_ptr(), start, len, h, idx, a, b);
        return idx;
    }

    // Slow path: name longer than 16 bytes. Fold 8-byte chunks into the
    // hash until the ';' chunk, then mask its tail.
    let mut h = w0 ^ w1;
    c.at += 16;
    let len: usize;
    loop {
        let w = c.u64_here();
        let m = semi_mask(w);
        if m != 0 {
            let tz = m.trailing_zeros();
            h ^= w << (63 - tz);
            c.at += (tz >> 3) as usize;
            break;
        }
        h ^= w;
        c.at += 8;
    }
    len = c.at - start;
    // Leave cursor ON ';' for parse_tenths.
    let total = len + 1;
    let (a, b) = masked_pair(c, start, total);
    let mut idx = index_of(h);
    idx = insert_probe(table, c.bytes.as_ptr(), start, len, h, idx, a, b);
    idx
}

/// Fetch the masked 16-byte name fingerprint used for the fast compare.
#[inline(always)]
unsafe fn masked_pair(c: &Cursor, start: usize, total: usize) -> (u64, u64) {
    let mut a = c.u64_at(start);
    let mut b = c.u64_at(start + 8);
    if total <= 8 {
        // total in [1,8]: total-1 in [0,7].
        a &= *LOW_MASK.get_unchecked(total - 1);
        b = 0;
    } else if total < 16 {
        // total in [9,15]: total-9 in [0,6].
        b &= *LOW_MASK.get_unchecked(total - 9);
    }
    (a, b)
}

/// Linear probe (step 31) with full byte comparison on fingerprint hit.
unsafe fn insert_probe(
    table: &mut [Entry],
    base: *const u8,
    start: usize,
    len: usize,
    h: u64,
    mut idx: usize,
    a: u64,
    b: u64,
) -> usize {
    loop {
        // idx stays < TABLE_SIZE: index_of masks, step 31 preserves the mask.
        let e = &*table.get_unchecked(idx);
        if !e.used {
            let s = &mut *table.get_unchecked_mut(idx);
            s.used = true;
            s.hash = h;
            s.fp0 = a;
            s.fp1 = b;
            s.name = base.add(start);
            s.name_len = len as u32;
            s.lo = 999;
            s.hi = -999;
            return idx;
        }
        if e.hash == h && e.name_len as usize == len && bytes_eq(e.name, base.add(start), len) {
            return idx;
        }
        idx = (idx + 31) & TABLE_MASK;
    }
}

#[inline(always)]
unsafe fn accumulate(table: &mut [Entry], idx: usize, v: i32) {
    // idx < TABLE_SIZE == table.len(): produced by index_of's mask.
    let e = &mut *table.get_unchecked_mut(idx);
    // Branchy min/max: taken ~log(n) times per key, predictor learns it.
    if v < e.lo {
        e.lo = v;
    }
    if v > e.hi {
        e.hi = v;
    }
    e.total += v as i64;
    e.n += 1;
}

/// Scalar fallback for rows starting within TAIL_MARGIN bytes of EOF.
/// Every read is provably inside the mapping: the row itself is whole
/// (chunks are line-aligned), byte scans stay within the row, fingerprints
/// are built byte-wise, and the only word loads are explicitly bounded
/// (`wa + 8 <= semi < file_end`). Produces bit-identical table updates to
/// the vectorized path: same fingerprints, same hash fold, same accumulation.
#[inline(never)]
unsafe fn parse_row_scalar(c: &mut Cursor, table: &mut [Entry]) {
    let p = c.bytes.as_ptr();
    let start = c.at;
    // Byte-at-a-time ';' scan; the row is whole, so this terminates in-bounds.
    let mut semi = start;
    while *p.add(semi) != b';' {
        semi += 1;
    }
    let len = semi - start;
    // Fingerprint: first min(len+1, 16) bytes as LE u64s. The +1 covers ';',
    // exactly matching the fast path's LOW_MASK[len] masking for len <= 15
    // and the slow path's raw first-16-bytes for len > 15. Byte-built so no
    // load can cross EOF, even for the final short row.
    let fp_bytes = (len + 1).min(16);
    let mut a: u64 = 0;
    let mut b: u64 = 0;
    let mut i = 0;
    while i < fp_bytes && i < 8 {
        a |= (*p.add(start + i) as u64) << (8 * i);
        i += 1;
    }
    while i < fp_bytes {
        b |= (*p.add(start + i) as u64) << (8 * (i - 8));
        i += 1;
    }
    let h = if len <= 15 {
        a ^ b
    } else {
        // Same fold as the lookup() slow path (a ^ b == w0 ^ w1): whole
        // words strictly before the ';' word, then the partial word [wa, semi]
        // with ';' itself folded in, exactly as the slow path's shift does.
        let mut h = a ^ b;
        let mut wa = start + 16;
        while wa + 8 <= semi {
            h ^= c.u64_at(wa);
            wa += 8;
        }
        let mut w: u64 = 0;
        let mut k = 0;
        while wa + k <= semi {
            w |= (*p.add(wa + k) as u64) << (8 * k);
            k += 1;
        }
        let tz = (semi - wa) * 8 + 7; // bit index of ';', as in semi_mask
        h ^= w << (63 - tz);
        h
    };
    let idx = insert_probe(table, p, start, len, h, index_of(h), a, b);
    // Scalar temperature parse: [-]d[d].d\n
    let mut at = semi + 1;
    let neg = *p.add(at) == b'-';
    at += neg as usize;
    let mut v: i32 = 0;
    while *p.add(at) != b'.' {
        v = v * 10 + (*p.add(at) - b'0') as i32;
        at += 1;
    }
    at += 1; // past '.'
    v = v * 10 + (*p.add(at) - b'0') as i32;
    c.at = at + 2; // past digit and '\n'
    accumulate(table, idx, if neg { -v } else { v });
}

/// Advance to the next '\n' at or after `at` (SWAR scan, scalar tail).
unsafe fn align_newline(bytes: &[u8], mut at: usize, end: usize) -> usize {
    let p = bytes.as_ptr();
    while at + 8 <= end {
        let w = (p.add(at) as *const u64).read_unaligned();
        let m = newline_mask(w);
        if m != 0 {
            return at + ((m.trailing_zeros() >> 3) as usize);
        }
        at += 8;
    }
    while at < end && *p.add(at) != b'\n' {
        at += 1;
    }
    at.min(end.saturating_sub(1))
}

fn worker(
    bytes: &[u8],
    cursor: &AtomicUsize,
    scan_end: usize,
    out: &mut Vec<(Vec<u8>, i32, i64, i32, u32)>,
) {
    let mut table: Vec<Entry> = (0..TABLE_SIZE).map(|_| Entry::vacant()).collect();
    // scan_end is line-aligned and at least TAIL_MARGIN bytes before EOF
    // (or 0), so every row the workers touch is fast-path safe with zero
    // per-row bounds checks; the tail is parsed scalarly by the main thread.
    unsafe {
        loop {
            let claimed = cursor.fetch_add(CHUNK_BYTES, Ordering::Relaxed);
            if claimed >= scan_end {
                break;
            }
            let seg_end = align_newline(bytes, (claimed + CHUNK_BYTES).min(scan_end - 1), scan_end);
            let seg_start = if claimed == 0 {
                0
            } else {
                align_newline(bytes, claimed, scan_end) + 1
            };
            if seg_start >= seg_end {
                continue;
            }
            // Three interleaved cursors split the chunk for ILP.
            let third = (seg_end - seg_start) / 3;
            let b1 = align_newline(bytes, seg_start + third, scan_end);
            let b2 = align_newline(bytes, seg_start + third + third, scan_end);
            let mut c1 = Cursor { bytes, at: seg_start, stop: b1 };
            let mut c2 = Cursor { bytes, at: b1 + 1, stop: b2 };
            let mut c3 = Cursor { bytes, at: b2 + 1, stop: seg_end };

            while c1.live() && c2.live() && c3.live() {
                // The three lookups are independent; the CPU overlaps their
                // table probes (ILP hides the load latency).
                let i1 = lookup(&mut c1, &mut table);
                let i2 = lookup(&mut c2, &mut table);
                let i3 = lookup(&mut c3, &mut table);
                let v1 = parse_tenths(&mut c1);
                let v2 = parse_tenths(&mut c2);
                let v3 = parse_tenths(&mut c3);
                accumulate(&mut table, i1, v1);
                accumulate(&mut table, i2, v2);
                accumulate(&mut table, i3, v3);
            }
            for c in [&mut c1, &mut c2, &mut c3] {
                while c.live() {
                    let i = lookup(c, &mut table);
                    let v = parse_tenths(c);
                    accumulate(&mut table, i, v);
                }
            }
        }
        collect_table(&table, out);
    }
}

/// Move a per-thread table's results into the merge input.
fn collect_table(table: &[Entry], out: &mut Vec<(Vec<u8>, i32, i64, i32, u32)>) {
    unsafe {
        for e in table.iter() {
            if e.used {
                let name = std::slice::from_raw_parts(e.name, e.name_len as usize).to_vec();
                out.push((name, e.lo, e.total, e.hi, e.n));
            }
        }
    }
}

/// Java-compatible rounding: Math.round semantics (half up, i.e. toward
/// +infinity), then one decimal. Rust's f64::round is half-away-from-zero
/// and disagrees with the 1BRC reference on negative .5 means.
fn round1(v: f64) -> f64 {
    ((v * 10.0 + 0.5).floor() as i64) as f64 / 10.0
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "measurements.txt".to_string());
    let file = std::fs::File::open(&path).expect("open input");
    let mmap = unsafe { memmap2::Mmap::map(&file).expect("mmap") };
    // Streaming one-pass access: tell the kernel to readahead aggressively
    // and drop pages behind us.
    unsafe {
        let _ = libc::madvise(
            mmap.as_ptr() as *mut libc::c_void,
            mmap.len(),
            libc::MADV_SEQUENTIAL,
        );
    }
    let bytes: &[u8] = &mmap;
    let file_end = bytes.len();

    // Scalar tail: rows starting within TAIL_MARGIN bytes of EOF are parsed
    // by the main thread with parse_row_scalar, so the workers' 8-byte loads
    // provably never cross the mapping end. tail_start is the last line start
    // at or before file_end - TAIL_MARGIN (0 for a sub-128-byte file).
    let tail_start = if file_end > TAIL_MARGIN {
        let limit = file_end - TAIL_MARGIN;
        let mut j = limit - 1;
        while j > 0 && bytes[j] != b'\n' {
            j -= 1;
        }
        if bytes[j] == b'\n' { j + 1 } else { 0 }
    } else {
        0
    };

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let cursor = AtomicUsize::new(0);
    let mut per_thread: Vec<Vec<(Vec<u8>, i32, i64, i32, u32)>> = Vec::new();
    std::thread::scope(|s| {
        let mut hs = Vec::new();
        for _ in 0..threads {
            hs.push(s.spawn(|| {
                let mut v = Vec::new();
                worker(bytes, &cursor, tail_start, &mut v);
                v
            }));
        }
        for h in hs {
            per_thread.push(h.join().unwrap());
        }
    });
    if tail_start < file_end {
        let mut tail_table: Vec<Entry> = (0..TABLE_SIZE).map(|_| Entry::vacant()).collect();
        let mut c = Cursor { bytes, at: tail_start, stop: file_end };
        unsafe {
            while c.live() {
                parse_row_scalar(&mut c, &mut tail_table);
            }
        }
        let mut v = Vec::new();
        collect_table(&tail_table, &mut v);
        per_thread.push(v);
    }

    let mut merged: std::collections::BTreeMap<Vec<u8>, (i32, i64, i32, u32)> =
        std::collections::BTreeMap::new();
    for tv in &per_thread {
        for (name, lo, total, hi, n) in tv {
            merged
                .entry(name.clone())
                .and_modify(|e| {
                    if *lo < e.0 {
                        e.0 = *lo;
                    }
                    if *hi > e.2 {
                        e.2 = *hi;
                    }
                    e.1 += *total;
                    e.3 += *n;
                })
                .or_insert((*lo, *total, *hi, *n));
        }
    }

    let mut s = String::from("{");
    for (i, (name, (lo, total, hi, n))) in merged.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!(
            "{}={:.1}/{:.1}/{:.1}",
            String::from_utf8_lossy(name),
            round1(*lo as f64 / 10.0),
            round1(*total as f64 / 10.0 / *n as f64),
            round1(*hi as f64 / 10.0)
        ));
    }
    s.push('}');
    println!("{s}");
    std::mem::drop(mmap); // keep the mapping alive while name pointers are in use
}
