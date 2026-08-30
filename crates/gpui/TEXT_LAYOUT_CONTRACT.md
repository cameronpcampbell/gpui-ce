# Text layout behavior contract

GPUI keeps backend layout as the authority for shaping, line breaking, bidi order, and editing
geometry. The flattened runs, clusters, lines, and fragments in `text_system` are a temporary
painting view. Callers must not infer editing behavior from their count or grouping.

- Logical indices and ranges use UTF-8 byte offsets and end on character boundaries.
- Painting traverses visual lines, fragments, and clusters in visual order.
- Hit testing returns a byte index with upstream or downstream caret affinity.
- Visual left and right movement crosses bidi runs and soft-wrapped lines in display order.
- Selection geometry may contain more than one rectangle on a bidi line.
- Hard breaks, consecutive breaks, trailing empty lines, and atomic clusters survive wrapping.
- `line_clamp` limits the number of bounded visual lines. The last allowed line receives all
  remaining text and has no maximum advance. A clamp of zero still produces one line.
- Successful font registration advances the backend font generation. Layout caches must miss
  after that change so a layout that used fallback can resolve the newly registered face.

Text transforms, decoration colors, rasterization, document edits without a layout, and ellipsis
generation remain GPUI responsibilities.
