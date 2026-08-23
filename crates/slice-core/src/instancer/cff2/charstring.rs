//! Rewriting CFF2 charstrings so their blends describe the new design space.
//!
//! The alternative to this module is to draw every glyph through skrifa at the target
//! location and emit fresh charstrings from the resulting path. That is less code and it
//! is wrong: it discards every `hstem`, `vstem`, `hintmask` and `cntrmask` the designer
//! put there, which is exactly the data that keeps a face legible at text sizes. The
//! interpreter here touches only the numbers a `blend` produces and copies every other
//! byte of the program through, so hinting survives instancing intact.
//!
//! # Subroutines are inlined
//!
//! A subroutine's `blend` operators are resolved against whichever `vsindex` was in
//! effect when it was called, so the same subroutine called from two glyphs with
//! different `vsindex` values does not have one rewriting. Inlining sidesteps that
//! entirely, at the cost of the size the subroutines were saving. fontTools does the
//! same thing for the same reason — `instantiateCFF2` opens with `cff.desubroutinize()`.

use super::num::{read_charstring_number, write_charstring_number, MAX_DEPTH};
use crate::instancer::glyphs::ot_round;
use crate::instancer::regions::RegionRemap;
use crate::SliceError;

/// Type 2 operators this rewriter treats specially. Everything else is copied through.
mod op {
    pub const HSTEM: u8 = 1;
    pub const VSTEM: u8 = 3;
    pub const CALLSUBR: u8 = 10;
    pub const RETURN: u8 = 11;
    pub const ESCAPE: u8 = 12;
    pub const ENDCHAR: u8 = 14;
    pub const VSINDEX: u8 = 15;
    pub const BLEND: u8 = 16;
    pub const HSTEMHM: u8 = 18;
    pub const HINTMASK: u8 = 19;
    pub const CNTRMASK: u8 = 20;
    pub const VSTEMHM: u8 = 23;
    pub const CALLGSUBR: u8 = 29;
}

/// Everything the rewriter needs that is not in the charstring itself.
pub struct Context<'a> {
    /// Global subroutines, as stored in the CFF2 table.
    pub global_subrs: &'a [&'a [u8]],
    /// Local subroutines for the font DICT this glyph belongs to.
    pub local_subrs: &'a [&'a [u8]],
    /// One region remap per ItemVariationData, indexed the way `vsindex` indexes them.
    pub remaps: &'a [RegionRemap],
    /// The `vsindex` the glyph starts with, which its Private DICT may have set.
    pub initial_vsindex: u16,
}

/// One value waiting on the operand stack.
///
/// The distinction matters because a blended value occupies one stack slot but several
/// operands in the byte stream, so the logical stack and the encoded form have to be
/// tracked apart from each other.
#[derive(Clone, Debug)]
enum Pending {
    Plain(f64),
    Blended { base: f64, deltas: Vec<f64> },
}

impl Pending {
    /// The value the interpreter sees, which is the value at the new default location.
    fn value(&self) -> f64 {
        match self {
            Pending::Plain(v) => *v,
            Pending::Blended { base, .. } => *base,
        }
    }
}

struct Rewriter<'a> {
    context: &'a Context<'a>,
    out: Vec<u8>,
    pending: Vec<Pending>,
    stem_count: usize,
    vsindex: u16,
    /// The `vsindex` the emitted blends were written against, once one has been emitted.
    ///
    /// CFF2 allows one `vsindex` per charstring, so every blend in the output has to
    /// agree on the ItemVariationData it indexes.
    emitted_vsindex: Option<u16>,
    finished: bool,
}

fn malformed(what: &str) -> SliceError {
    SliceError::Read(format!("a CFF2 charstring is malformed: {what}"))
}

/// Rewrite one charstring, resolving or re-tenting every blend it contains.
///
/// When the remaps leave no regions — every axis pinned — the result contains no `blend`
/// and no `vsindex`, which is what a static instance needs, because a `blend` in a font
/// with no ItemVariationStore has nothing to blend against.
pub fn rewrite(charstring: &[u8], context: &Context) -> Result<Vec<u8>, SliceError> {
    let mut rewriter = Rewriter {
        context,
        out: Vec::with_capacity(charstring.len()),
        pending: Vec::new(),
        stem_count: 0,
        vsindex: context.initial_vsindex,
        emitted_vsindex: None,
        finished: false,
    };
    rewriter.run(charstring, 0)?;
    // Trailing operands with no operator to consume them are legal in neither format,
    // but a charstring that ends mid-expression should not silently lose them.
    rewriter.flush();

    let mut out = rewriter.out;
    // `vsindex` has to come before the first blend, and after inlining the place it
    // originally sat may be inside a subroutine body. The head of the charstring is
    // always before the first blend, so that is where it goes.
    if let Some(index) = rewriter.emitted_vsindex {
        if index != context.initial_vsindex {
            let mut head = Vec::with_capacity(out.len() + 3);
            write_charstring_number(f64::from(index), &mut head);
            head.push(op::VSINDEX);
            head.append(&mut out);
            out = head;
        }
    }
    Ok(out)
}

impl Rewriter<'_> {
    fn run(&mut self, data: &[u8], depth: usize) -> Result<(), SliceError> {
        if depth > MAX_DEPTH {
            return Err(malformed("subroutine calls nested too deeply"));
        }

        let mut pos = 0usize;
        while pos < data.len() {
            if self.finished {
                return Ok(());
            }
            let b0 = data[pos];
            if b0 >= 32 || b0 == 28 {
                let (value, len) = read_charstring_number(data, pos)?;
                self.pending.push(Pending::Plain(value));
                pos += len;
                continue;
            }
            pos += 1;

            match b0 {
                op::HSTEM | op::VSTEM | op::HSTEMHM | op::VSTEMHM => {
                    self.stem_count += self.pending.len() / 2;
                    self.flush();
                    self.out.push(b0);
                }
                op::HINTMASK | op::CNTRMASK => {
                    // Operands still on the stack at a mask are an implicit `vstem`.
                    self.stem_count += self.pending.len() / 2;
                    self.flush();
                    self.out.push(b0);
                    let mask_len = self.stem_count.div_ceil(8).max(1);
                    let end = pos + mask_len;
                    let mask = data
                        .get(pos..end)
                        .ok_or_else(|| malformed("a hint mask ran past the end"))?;
                    self.out.extend_from_slice(mask);
                    pos = end;
                }
                op::CALLSUBR => self.call(depth, false)?,
                op::CALLGSUBR => self.call(depth, true)?,
                op::RETURN => return Ok(()),
                op::ENDCHAR => {
                    // CFF2 has neither operator, but a font that carries one means the
                    // program to stop here, and honouring that is safer than reading
                    // whatever follows as more charstring.
                    self.finished = true;
                    return Ok(());
                }
                op::VSINDEX => {
                    let value = self
                        .pending
                        .pop()
                        .ok_or_else(|| malformed("vsindex with no operand"))?
                        .value();
                    if !(0.0..=65535.0).contains(&value) {
                        return Err(malformed("vsindex is not a variation data index"));
                    }
                    self.vsindex = value as u16;
                }
                op::BLEND => self.blend()?,
                op::ESCAPE => {
                    let b1 = *data
                        .get(pos)
                        .ok_or_else(|| malformed("a two-byte operator was cut short"))?;
                    pos += 1;
                    self.flush();
                    self.out.push(op::ESCAPE);
                    self.out.push(b1);
                }
                _ => {
                    self.flush();
                    self.out.push(b0);
                }
            }
        }
        Ok(())
    }

    /// Inline the subroutine the top of the stack names.
    fn call(&mut self, depth: usize, global: bool) -> Result<(), SliceError> {
        let subrs = if global {
            self.context.global_subrs
        } else {
            self.context.local_subrs
        };
        let operand = self
            .pending
            .pop()
            .ok_or_else(|| malformed("a subroutine call with no index"))?;
        let Pending::Plain(index) = operand else {
            return Err(malformed("a subroutine index cannot be a blended value"));
        };
        let bias = subr_bias(subrs.len());
        let resolved = index as i64 + i64::from(bias);
        let body = usize::try_from(resolved)
            .ok()
            .and_then(|i| subrs.get(i))
            .ok_or_else(|| malformed(&format!("subroutine {resolved} does not exist")))?;
        self.run(body, depth + 1)
    }

    /// Resolve one `blend`, replacing its operands with values for the new design space.
    fn blend(&mut self) -> Result<(), SliceError> {
        let count = self
            .pending
            .pop()
            .ok_or_else(|| malformed("blend with no operand count"))?
            .value();
        if !(1.0..=65535.0).contains(&count) || count.fract() != 0.0 {
            return Err(malformed(&format!("{count} is not a blend operand count")));
        }
        let count = count as usize;

        let remap = self
            .context
            .remaps
            .get(self.vsindex as usize)
            .ok_or_else(|| malformed(&format!("vsindex {} has no variation data", self.vsindex)))?;
        let old_regions = remap.old_region_count();

        let needed = count * (old_regions + 1);
        if self.pending.len() < needed {
            return Err(malformed("blend has fewer operands than its count claims"));
        }
        let operands: Vec<f64> = self
            .pending
            .split_off(self.pending.len() - needed)
            .into_iter()
            .map(|item| match item {
                // fontTools raises NotImplementedError on nested blends and no shipping
                // font has one; treating it as malformed keeps the same promise, that
                // the output is either right or refused.
                Pending::Plain(v) => Ok(v),
                Pending::Blended { .. } => Err(malformed("blends cannot nest")),
            })
            .collect::<Result<_, _>>()?;

        let (bases, deltas) = operands.split_at(count);
        for (index, base) in bases.iter().enumerate() {
            let row = &deltas[index * old_regions..(index + 1) * old_regions];
            let (gain, new_deltas) = remap.apply(row);
            // fontTools adds the gain to the base *rounded*, and rounds the surviving
            // deltas with `otRound`. The two roundings differ — Python's `round` breaks
            // ties to even, `otRound` breaks them upwards — and both are copied here so
            // that a value landing exactly on a half agrees with the reference the
            // conformance corpus compares against.
            let base = base + gain.round_ties_even();
            let new_deltas: Vec<f64> = new_deltas.iter().map(|d| f64::from(ot_round(*d))).collect();
            if new_deltas.iter().all(|d| *d == 0.0) {
                // A value that no longer moves is not worth a blend, and writing one
                // would force a `vsindex` into a charstring that has nothing to vary.
                self.pending.push(Pending::Plain(base));
            } else {
                self.record_blend_vsindex()?;
                self.pending.push(Pending::Blended {
                    base,
                    deltas: new_deltas,
                });
            }
        }
        Ok(())
    }

    fn record_blend_vsindex(&mut self) -> Result<(), SliceError> {
        match self.emitted_vsindex {
            None => {
                self.emitted_vsindex = Some(self.vsindex);
                Ok(())
            }
            Some(existing) if existing == self.vsindex => Ok(()),
            Some(existing) => Err(SliceError::Unsupported(format!(
                "a glyph in this font blends against two different variation data \
                 subtables ({existing} and {}), which a CFF2 charstring cannot express.",
                self.vsindex
            ))),
        }
    }

    /// Write out the operand stack, grouping the blended values into `blend` operators.
    fn flush(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        let mut index = 0usize;
        while index < pending.len() {
            match &pending[index] {
                Pending::Plain(value) => {
                    write_charstring_number(*value, &mut self.out);
                    index += 1;
                }
                Pending::Blended { .. } => {
                    // One `blend` covers as many consecutive blended values as there
                    // are: the operator takes a count, and the values it produces stay
                    // on the stack in order.
                    let start = index;
                    while matches!(pending.get(index), Some(Pending::Blended { .. })) {
                        index += 1;
                    }
                    let run = &pending[start..index];
                    for item in run {
                        if let Pending::Blended { base, .. } = item {
                            write_charstring_number(*base, &mut self.out);
                        }
                    }
                    for item in run {
                        if let Pending::Blended { deltas, .. } = item {
                            for delta in deltas {
                                write_charstring_number(*delta, &mut self.out);
                            }
                        }
                    }
                    write_charstring_number(run.len() as f64, &mut self.out);
                    self.out.push(op::BLEND);
                }
            }
        }
    }
}

/// The bias added to a subroutine index, which depends on how many there are.
pub fn subr_bias(count: usize) -> i32 {
    if count < 1240 {
        107
    } else if count < 33900 {
        1131
    } else {
        32768
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instancer::regions::{pinned_remap, Region, RegionRemap};

    fn region(tent: (f64, f64, f64)) -> Region {
        [(0usize, tent)].into_iter().collect()
    }

    /// Decode a charstring back into numbers and operator names, for readable assertions.
    fn disassemble(data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut pos = 0;
        let mut stems = 0usize;
        let mut operands = 0usize;
        while pos < data.len() {
            let b0 = data[pos];
            if b0 >= 32 || b0 == 28 {
                let (value, len) = read_charstring_number(data, pos).unwrap();
                out.push(format!("{value}"));
                operands += 1;
                pos += len;
                continue;
            }
            pos += 1;
            match b0 {
                op::ESCAPE => {
                    out.push(format!("esc{}", data[pos]));
                    pos += 1;
                }
                op::HINTMASK | op::CNTRMASK => {
                    stems += operands / 2;
                    out.push(
                        if b0 == op::HINTMASK {
                            "hintmask"
                        } else {
                            "cntrmask"
                        }
                        .into(),
                    );
                    let len = stems.div_ceil(8).max(1);
                    out.push(format!("mask{:02x?}", &data[pos..pos + len]));
                    pos += len;
                }
                op::HSTEM | op::VSTEM | op::HSTEMHM | op::VSTEMHM => {
                    stems += operands / 2;
                    out.push(format!("op{b0}"));
                }
                _ => out.push(format!("op{b0}")),
            }
            operands = 0;
        }
        out
    }

    #[test]
    fn a_charstring_with_no_blends_comes_through_byte_for_byte() {
        // `.notdef` from the `cff2-vf` fixture: no blends, so nothing should move.
        let source = hex("bd16f888f950fc8806d14515f7fcfcc4fbfc06");
        let remaps = vec![pinned_remap(&[region((0.0, 1.0, 1.0))], &[0.6])];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        assert_eq!(rewrite(&source, &context).unwrap(), source);
    }

    #[test]
    fn pinning_resolves_a_blend_into_a_plain_number() {
        // The `I` glyph from `cff2-vf`: 90 with a delta of 130 on a single region. At
        // wght=700 the region's scalar is 0.6, so the stem is 90 + 78 = 168 -- the value
        // fontTools 4.62.1 writes for the same job.
        let source = hex("e516e5f7168c10f95031fb168c1006");
        let remaps = vec![pinned_remap(&[region((0.0, 1.0, 1.0))], &[0.6])];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(
            disassemble(&out),
            ["90", "op22", "168", "700", "-168", "op6"]
        );
    }

    #[test]
    fn a_multi_value_blend_resolves_every_value() {
        // `period`: three values blended at once, `90 90 -90 130 130 -130 3 blend`.
        let source = hex("e516e5e531f716f716fb168e1006");
        let remaps = vec![pinned_remap(&[region((0.0, 1.0, 1.0))], &[0.6])];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(
            disassemble(&out),
            ["90", "op22", "168", "168", "-168", "op6"]
        );
    }

    /// A remap that keeps one region and scales its deltas, as narrowing an axis does.
    fn scaling_remap(scale: f64) -> RegionRemap {
        RegionRemap {
            regions: vec![vec![(0.0, 1.0, 1.0)]],
            gain: vec![0.0],
            coeff: vec![vec![scale]],
        }
    }

    #[test]
    fn narrowing_an_axis_keeps_the_blend_and_scales_its_deltas() {
        let source = hex("e516e5f7168c10f95031fb168c1006");
        let remaps = vec![scaling_remap(0.6)];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(
            disassemble(&out),
            ["90", "op22", "90", "78", "1", "op16", "700", "-90", "-78", "1", "op16", "op6"]
        );
    }

    #[test]
    fn a_blend_whose_deltas_all_vanish_becomes_a_plain_number() {
        // Scaling by zero leaves nothing varying, and a blend that moves nothing would
        // drag a `vsindex` into a charstring that has no reason for one.
        let source = hex("e516e5f7168c10f95031fb168c1006");
        let remaps = vec![scaling_remap(0.0)];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(disassemble(&out), ["90", "op22", "90", "700", "-90", "op6"]);
    }

    #[test]
    fn a_subroutine_is_inlined_with_its_blends_resolved() {
        // The `I` program split in two: the caller moves, the subroutine draws.
        let subr = hex("e5f7168c10f95031fb168c1006");
        let subrs: Vec<&[u8]> = vec![&subr];
        // 107 is the bias for a small subroutine index, so operand -107 selects entry 0.
        let source = hex("e516200a");
        let remaps = vec![pinned_remap(&[region((0.0, 1.0, 1.0))], &[0.6])];
        let context = Context {
            global_subrs: &[],
            local_subrs: &subrs,
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(
            disassemble(&out),
            ["90", "op22", "168", "700", "-168", "op6"]
        );
    }

    #[test]
    fn hint_operators_and_their_masks_survive() {
        // 100 20 hstemhm, 60 30 hintmask <1 byte>, 0 hmoveto.
        let mut source = Vec::new();
        write_charstring_number(100.0, &mut source);
        write_charstring_number(20.0, &mut source);
        source.push(op::HSTEMHM);
        write_charstring_number(60.0, &mut source);
        write_charstring_number(30.0, &mut source);
        source.push(op::HINTMASK);
        source.push(0b1100_0000);
        write_charstring_number(0.0, &mut source);
        source.push(22);

        let remaps = vec![RegionRemap::default()];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        assert_eq!(rewrite(&source, &context).unwrap(), source);
    }

    #[test]
    fn vsindex_is_hoisted_to_the_head_when_the_blends_need_it() {
        // vsindex 1, then a blend against variation data 1.
        let mut source = Vec::new();
        write_charstring_number(1.0, &mut source);
        source.push(op::VSINDEX);
        write_charstring_number(90.0, &mut source);
        write_charstring_number(130.0, &mut source);
        write_charstring_number(1.0, &mut source);
        source.push(op::BLEND);
        source.push(22);

        let remaps = vec![RegionRemap::default(), scaling_remap(0.5)];
        let context = Context {
            global_subrs: &[],
            local_subrs: &[],
            remaps: &remaps,
            initial_vsindex: 0,
        };
        let out = rewrite(&source, &context).unwrap();
        assert_eq!(
            disassemble(&out),
            ["1", "op15", "90", "65", "1", "op16", "op22"]
        );
    }

    #[test]
    fn malformed_charstrings_are_errors_rather_than_panics() {
        let remaps = vec![pinned_remap(&[region((0.0, 1.0, 1.0))], &[0.6])];
        let context = |subrs: &'static [&'static [u8]]| Context {
            global_subrs: subrs,
            local_subrs: subrs,
            remaps: &remaps,
            initial_vsindex: 0,
        };

        // A blend claiming more operands than the stack holds.
        let mut blend_underflow = Vec::new();
        write_charstring_number(5.0, &mut blend_underflow);
        blend_underflow.push(op::BLEND);
        assert!(rewrite(&blend_underflow, &context(&[])).is_err());

        // A blend with no count at all.
        assert!(rewrite(&[op::BLEND], &context(&[])).is_err());

        // A call to a subroutine that does not exist.
        let mut missing_subr = Vec::new();
        write_charstring_number(0.0, &mut missing_subr);
        missing_subr.push(op::CALLSUBR);
        assert!(rewrite(&missing_subr, &context(&[])).is_err());

        // A subroutine that calls itself: this must terminate.
        static SELF_CALL: &[u8] = &[139 - 107, op::CALLSUBR];
        static SELF_CALLING: &[&[u8]] = &[SELF_CALL];
        assert!(rewrite(SELF_CALL, &context(SELF_CALLING)).is_err());

        // A hint mask running off the end of the program.
        assert!(rewrite(&[op::HINTMASK], &context(&[])).is_err());

        // A truncated operand.
        assert!(rewrite(&[255, 0, 0], &context(&[])).is_err());
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
}
