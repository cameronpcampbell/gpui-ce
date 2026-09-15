# Future work

## Delegate inline vertical alignment to Parley

GPUI currently recalculates line height, baselines, and inline-box Y positions to support `baseline`, `middle`, `top`, and `bottom`. Parley 0.11.1 only aligns a box's bottom edge to the text baseline.

Upstream `main` has added `InlineBox::baseline`, while [full vertical alignment remains in draft](https://github.com/linebender/parley/pull/579). Once Parley releases compatible support, pass box alignment data to Parley and remove `align_inline_boxes` and its per-line metric bookkeeping after checking layout parity.

## Use out-of-flow boxes

GPUI sends every Parley box as `InlineBoxKind::InFlow` and removes absolute children from the inline document. Evaluate `InlineBoxKind::OutOfFlow` for absolute inline children so Parley provides their static inline position while Taffy retains sizing and final inset placement.
