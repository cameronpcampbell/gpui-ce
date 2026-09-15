# Future work

Verified against Parley 0.11.1 and upstream `main` on 2026-09-15.

## Vertical alignment

GPUI handles `baseline`, `middle`, `top`, `bottom`, and their line metrics itself.

- Parley 0.11.1 supports bottom-to-baseline alignment only.
- [PR #639](https://github.com/linebender/parley/pull/639) added custom box baselines and matching line-height calculation to `main`, but not the other modes.
- Draft [PR #579](https://github.com/linebender/parley/pull/579) implements and tests all four modes, but is not merge-ready and uses an older `InlineBox` API.

Keep `align_inline_boxes` until equivalent support is released and passes GPUI's inline-layout tests.

## Out-of-flow boxes

[`InlineBoxKind::OutOfFlow`](https://docs.rs/parley/0.11.1/parley/enum.InlineBoxKind.html#variant.OutOfFlow) already exists in 0.11.1. It can provide an absolute inline child's static position without affecting text flow. Taffy must still handle sizing, insets, and final placement.
