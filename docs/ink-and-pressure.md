# Ink thickness, pressure, and edge treatment

This is the Nocturne product behavior for pressure-sensitive drawing. The detailed engineering
contract is maintained by the [`nc-uiux-drawing` skill](../SKILLS/nc-uiux-drawing/SKILL.md), in
[Ink scaling and edge treatment](../SKILLS/nc-uiux-drawing/references/ink-scaling-and-edge-treatment.md).
The skill is the owner of this contract: implementation changes must update it first, and this
document must remain consistent with it.

## Thickness

The thickness chooser uses roughly geometric spacing, from fine writing to broad marks:

```text
0.4, 0.7, 1, 1.4, 2, 2.8, 3.5, 4.5, 6, 9, 14, 20, 32, 50, 80 pt
```

Tablet Studio offers separate eraser sizes from 2 to 80 points.

## Pressure

The selected thickness is the baseline full-pressure width at neutral sensitivity. Above neutral,
the sensitivity control also expands the available full-pressure range to 4x at 100, matching
the pressure canvas's 12 px diagnostic maximum against a 3 px pen baseline. Sensitivity changes
the response curve and, above neutral, that maximum width:

- low sensitivity requires more pressure before the stroke becomes broad;
- neutral sensitivity follows the device reading approximately;
- high sensitivity makes a lighter hand become broad more quickly;
- sensitivity 100 must reach the expanded 4x width at maximum pressure.

The intended model is a bounded power curve. It must produce a monotonic, finite response and a
clearly visible difference between light, medium, and hard pressure. Highlighters may remain flat
because their broad translucent mark is intentional.

Pressure readings must be inspected before tuning the curve. If a tablet driver compresses the
range delivered to Nocturne, a curve alone cannot restore the missing physical distinction.

## Ink edges

Ink should not be rendered with a generic blur. Thin pen lines should remain crisp and
antialiased. Pencil uses graphite grain in page coordinates, and brush edges use a softer fringe
around a solid core. Fountain pen has a crisp pressure-shaped edge. Highlighter lays down a
translucent, pressure-flat mark. The fringe uses a fixed physical width.

The editable ink data must preserve the original points, pressure readings, tool, selected width,
and sensitivity. Edge treatment is a rendering concern and must not destroy that information.

Round endpoints must not darken translucent ink by painting a cap over its own ribbon.
Separate highlighter strokes still composite normally where they overlap.

## Acceptance checks

Synthetic tests must cover pressures `0`, `0.1`, `0.25`, `0.5`, `0.75`, and `1.0` at minimum,
neutral, and maximum sensitivity. Verify monotonic widths, full-pressure maximums, visible
light-to-hard separation, finite bounded values, and behavior in both light and dark themes.
Repeat with a real pressure pen and a mouse before claiming pressure behavior is complete.
