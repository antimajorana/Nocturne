//! Pure-Rust 2D bounding-box math post-processor for platform OCR (e.g. Apple Vision).
//!
//! Converts recognized text fragments with normalized top-left bounding boxes into clean LaTeX:
//! - Reconstructs fractions (`\frac{num}{den}`) from horizontal fraction bars or vertically
//!   stacked terms.
//! - Reconstructs limits on big operators (`\sum_{...}^{...}`, `\int_{...}^{...}`, `\prod_{...}^{...}`).
//! - Reconstructs superscripts (`^{...}`) and subscripts (`_{...}`) from vertical offset and size
//!   relative to baseline neighbors.
//! - Normalizes Unicode math operators, Greek letters, square roots, and Unicode super/subscripts.

/// One text fragment recognized by a platform OCR engine together with its bounding box in
/// normalized image coordinates (`0.0..=1.0`, top-left origin: `x` increases rightward, `y`
/// increases downward).
#[derive(Clone, Debug, PartialEq)]
pub struct VisionObservation {
    pub text: String,
    /// Left edge in `[0.0, 1.0]`.
    pub x: f32,
    /// Top edge in `[0.0, 1.0]` (smaller `y` is higher on the page).
    pub y: f32,
    /// Box width in `[0.0, 1.0]`.
    pub width: f32,
    /// Box height in `[0.0, 1.0]`.
    pub height: f32,
    /// Recognition confidence in `[0.0, 1.0]`.
    pub confidence: f32,
}

impl VisionObservation {
    /// Creates an observation using top-left-origin normalized coordinates.
    #[must_use]
    pub fn new(text: impl Into<String>, x: f32, y: f32, width: f32, height: f32, confidence: f32) -> Self {
        Self { text: text.into(), x, y, width, height, confidence }
    }

    /// Converts a bottom-left-origin rectangle (such as Apple Vision `VNRecognizedTextObservation`'s
    /// `boundingBox`, where `y_bottom` is measured upward from the bottom edge) into top-left-origin
    /// [`VisionObservation`] coordinates.
    #[must_use]
    pub fn from_bottom_left(text: impl Into<String>, x: f32, y_bottom: f32, width: f32, height: f32, confidence: f32) -> Self {
        Self { text: text.into(), x, y: (1.0 - y_bottom - height).max(0.0), width, height, confidence }
    }
}

#[derive(Clone, Debug)]
struct Item {
    raw_text: String,
    latex: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    is_composite: bool,
}

impl Item {
    fn from_observation(obs: &VisionObservation, aspect: f32) -> Option<Self> {
        let trimmed = obs.text.trim();
        if trimmed.is_empty()
            || !obs.x.is_finite()
            || !obs.y.is_finite()
            || !obs.width.is_finite()
            || !obs.height.is_finite()
            || obs.width <= 0.0
            || obs.height <= 0.0
        {
            return None;
        }
        Some(Self {
            raw_text: trimmed.to_string(),
            latex: normalize_math_text(trimmed),
            x: obs.x * aspect,
            y: obs.y,
            width: obs.width * aspect,
            height: obs.height,
            is_composite: false,
        })
    }

    fn right(&self) -> f32 {
        self.x + self.width
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }

    fn x_center(&self) -> f32 {
        self.x + self.width * 0.5
    }

    fn y_center(&self) -> f32 {
        self.y + self.height * 0.5
    }
}

/// Converts 2D bounding-box text observations into a single line of normalized LaTeX,
/// assuming a square (`1.0`) image aspect ratio.
#[must_use]
pub fn observations_to_latex(observations: &[VisionObservation]) -> String {
    observations_to_latex_with_aspect(observations, 1.0)
}

/// Converts 2D bounding-box text observations into a single line of normalized LaTeX,
/// scaling horizontal coordinates by the source image's `aspect` ratio (`width / height`,
/// clamped to `[0.25, 16.0]`) so horizontal gaps and widths use the same isotropic units
/// as vertical heights across every 2D layout pass.
#[must_use]
pub fn observations_to_latex_with_aspect(observations: &[VisionObservation], aspect: f32) -> String {
    let aspect = if aspect.is_finite() { aspect.clamp(0.25, 16.0) } else { 1.0 };
    let items: Vec<Item> = observations.iter().filter_map(|obs| Item::from_observation(obs, aspect)).collect();
    if items.is_empty() {
        return String::new();
    }
    layout_items(items, 0)
}

#[derive(Clone, Debug)]
struct TextToken {
    text: String,
    x: f32,
    y: f32,
    height: f32,
}

impl TextToken {
    fn from_observation(obs: &VisionObservation) -> Option<Self> {
        let trimmed = obs.text.trim();
        if trimmed.is_empty()
            || !obs.x.is_finite()
            || !obs.y.is_finite()
            || !obs.width.is_finite()
            || !obs.height.is_finite()
            || obs.width <= 0.0
            || obs.height <= 0.0
        {
            return None;
        }
        Some(Self { text: trimmed.to_string(), x: obs.x, y: obs.y, height: obs.height })
    }

    fn y_center(&self) -> f32 {
        self.y + self.height * 0.5
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

struct TextLine {
    tokens: Vec<TextToken>,
    top: f32,
    bottom: f32,
    center_sum: f32,
    height_sum: f32,
}

impl TextLine {
    fn new(tok: TextToken) -> Self {
        let top = tok.y;
        let bottom = tok.bottom();
        let center_sum = tok.y_center();
        let height_sum = tok.height;
        Self { tokens: vec![tok], top, bottom, center_sum, height_sum }
    }

    fn push(&mut self, tok: TextToken) {
        self.top = self.top.min(tok.y);
        self.bottom = self.bottom.max(tok.bottom());
        self.center_sum += tok.y_center();
        self.height_sum += tok.height;
        self.tokens.push(tok);
    }

    fn avg_center(&self) -> f32 {
        self.center_sum / self.tokens.len().max(1) as f32
    }

    fn avg_height(&self) -> f32 {
        self.height_sum / self.tokens.len().max(1) as f32
    }

    fn line_height(&self) -> f32 {
        (self.bottom - self.top).max(1e-4)
    }
}

/// Converts 2D bounding-box text observations into plain multi-line handwritten text,
/// preserving line breaks (`\n`) and larger paragraph breaks (`\n\n`).
#[must_use]
pub fn observations_to_text(observations: &[VisionObservation]) -> String {
    let mut tokens: Vec<TextToken> = observations.iter().filter_map(TextToken::from_observation).collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens.sort_by(|a, b| a.y_center().total_cmp(&b.y_center()).then_with(|| a.x.total_cmp(&b.x)));

    let mut lines: Vec<TextLine> = Vec::new();
    for tok in tokens {
        let mut best: Option<(usize, f32)> = None;
        for (idx, line) in lines.iter().enumerate() {
            let diff = (tok.y_center() - line.avg_center()).abs();
            let threshold = 0.55 * tok.height.min(line.avg_height());
            if diff <= threshold && best.is_none_or(|(_, best_diff)| diff < best_diff) {
                best = Some((idx, diff));
            }
        }
        match best {
            Some((idx, _)) => lines[idx].push(tok),
            None => lines.push(TextLine::new(tok)),
        }
    }

    for line in &mut lines {
        line.tokens.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    lines.sort_by(|a, b| a.avg_center().total_cmp(&b.avg_center()));

    let mut heights: Vec<f32> = lines.iter().map(TextLine::line_height).collect();
    heights.sort_by(f32::total_cmp);
    let median_line_height = heights[heights.len() / 2].max(1e-4);

    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            let prev = &lines[idx - 1];
            let gap = (line.top - prev.bottom).max(0.0);
            if gap > 1.6 * median_line_height {
                out.push_str("\n\n");
            } else {
                out.push('\n');
            }
        }
        for (t_idx, tok) in line.tokens.iter().enumerate() {
            if t_idx > 0 {
                out.push(' ');
            }
            out.push_str(&tok.text);
        }
    }
    out
}

fn layout_items(mut items: Vec<Item>, depth: usize) -> String {
    if items.is_empty() {
        return String::new();
    }
    if depth < 6 {
        items = collapse_big_operator_limits(items, depth + 1);
        items = collapse_explicit_bar_fractions(items, depth + 1);
        items = collapse_implicit_fractions(items, depth + 1);
    }
    if items.len() == 1 {
        return items.into_iter().next().map_or_else(String::new, |it| it.latex);
    }

    // Group into horizontal lines when items form distinct vertical rows (e.g. multi-line equations).
    let lines = split_into_lines(items);
    let rendered: Vec<String> = lines.into_iter().map(render_line).filter(|s| !s.is_empty()).collect();
    rendered.join(" ")
}

fn median_height(items: &[Item]) -> f32 {
    if items.is_empty() {
        return 0.05;
    }
    let mut hs: Vec<f32> = items.iter().map(|i| i.height).collect();
    hs.sort_by(f32::total_cmp);
    hs[hs.len() / 2].max(1e-4)
}

fn union_bbox(items: &[Item]) -> (f32, f32, f32, f32) {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for it in items {
        x0 = x0.min(it.x);
        y0 = y0.min(it.y);
        x1 = x1.max(it.right());
        y1 = y1.max(it.bottom());
    }
    (x0, y0, (x1 - x0).max(1e-4), (y1 - y0).max(1e-4))
}

fn x_overlap(a: &Item, b: &Item) -> f32 {
    (a.right().min(b.right()) - a.x.max(b.x)).max(0.0)
}

fn is_bar_text(raw: &str) -> bool {
    let trimmed = raw.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, '-' | '—' | '–' | '_' | '─' | '―' | '−'))
}

fn is_big_operator(latex: &str) -> bool {
    matches!(latex.trim(), "\\sum" | "\\prod" | "\\int" | "\\oint")
}

fn is_plain_operator(latex: &str) -> bool {
    matches!(
        latex.trim(),
        "+" | "-" | "=" | "<" | ">" | "," | ";" | ":" | "\\times" | "\\div" | "\\pm" | "\\mp" | "\\cdot" | "\\le" | "\\ge" | "\\ne" | "\\approx"
    )
}

/// Groups items directly above and/or below a big operator (`\sum`, `\prod`, `\int`, `\oint`)
/// into `\op_{lower}^{upper}`.
fn collapse_big_operator_limits(mut items: Vec<Item>, depth: usize) -> Vec<Item> {
    loop {
        let med_h = median_height(&items);
        let Some(op_idx) = items.iter().position(|it| !it.is_composite && is_big_operator(&it.latex)) else {
            break;
        };
        let op = items[op_idx].clone();
        let pad = op.width.max(op.height) * 0.65;
        let max_vgap = op.height.max(med_h) * 1.35;

        let mut above_indices = Vec::new();
        let mut below_indices = Vec::new();
        for (idx, it) in items.iter().enumerate() {
            if idx == op_idx {
                continue;
            }
            let horiz = it.x_center() >= op.x - pad && it.x_center() <= op.right() + pad;
            if !horiz {
                continue;
            }
            if it.bottom() <= op.y + op.height * 0.25 && (op.y - it.bottom()) <= max_vgap {
                above_indices.push(idx);
            } else if it.y >= op.bottom() - op.height * 0.25 && (it.y - op.bottom()) <= max_vgap {
                below_indices.push(idx);
            }
        }
        if above_indices.is_empty() && below_indices.is_empty() {
            items[op_idx].is_composite = true;
            continue;
        }
        let mut consumed = vec![false; items.len()];
        consumed[op_idx] = true;
        let mut above_items = Vec::new();
        let mut below_items = Vec::new();
        for &i in &above_indices {
            consumed[i] = true;
            above_items.push(items[i].clone());
        }
        for &i in &below_indices {
            consumed[i] = true;
            below_items.push(items[i].clone());
        }
        let mut group_for_box = vec![op.clone()];
        group_for_box.extend(above_items.iter().cloned());
        group_for_box.extend(below_items.iter().cloned());
        let (x, y, width, height) = union_bbox(&group_for_box);

        let mut latex = op.latex.clone();
        if !below_items.is_empty() {
            let lower = layout_items(below_items, depth);
            if !lower.is_empty() {
                latex.push_str(&format!("_{{{lower}}}"));
            }
        }
        if !above_items.is_empty() {
            let upper = layout_items(above_items, depth);
            if !upper.is_empty() {
                latex.push_str(&format!("^{{{upper}}}"));
            }
        }

        let mut next: Vec<Item> = items.into_iter().enumerate().filter(|(i, _)| !consumed[*i]).map(|(_, it)| it).collect();
        next.push(Item { raw_text: latex.clone(), latex, x, y, width, height, is_composite: true });
        items = next;
    }
    items
}

/// Finds explicit horizontal fraction bars with items both above and below them, collapsing the
/// narrowest (innermost) bar first so nested fractions work cleanly.
fn collapse_explicit_bar_fractions(mut items: Vec<Item>, depth: usize) -> Vec<Item> {
    loop {
        let med_h = median_height(&items);
        let mut best: Option<(usize, Vec<usize>, Vec<usize>, f32)> = None;

        for (bar_idx, bar) in items.iter().enumerate() {
            if bar.is_composite || !is_bar_text(&bar.raw_text) {
                continue;
            }
            let pad = (bar.width * 0.25).max(med_h * 0.35);
            let max_vgap = bar.width.max(med_h * 3.0);
            let mut above = Vec::new();
            let mut below = Vec::new();

            for (idx, it) in items.iter().enumerate() {
                if idx == bar_idx {
                    continue;
                }
                let in_span =
                    (it.x_center() >= bar.x - pad && it.x_center() <= bar.right() + pad) || x_overlap(it, bar) >= it.width.min(bar.width) * 0.5;
                if !in_span {
                    continue;
                }
                if it.y_center() < bar.y && (bar.y - it.bottom()) <= max_vgap {
                    above.push(idx);
                } else if it.y_center() > bar.bottom() && (it.y - bar.bottom()) <= max_vgap {
                    below.push(idx);
                }
            }
            if !above.is_empty() && !below.is_empty() {
                let replace = best.as_ref().is_none_or(|(_, _, _, w)| bar.width < *w);
                if replace {
                    best = Some((bar_idx, above, below, bar.width));
                }
            }
        }

        let Some((bar_idx, above, below, _)) = best else { break };
        let mut consumed = vec![false; items.len()];
        consumed[bar_idx] = true;
        let mut num_items = Vec::new();
        let mut den_items = Vec::new();
        for &i in &above {
            consumed[i] = true;
            num_items.push(items[i].clone());
        }
        for &i in &below {
            consumed[i] = true;
            den_items.push(items[i].clone());
        }
        let mut all_box = vec![items[bar_idx].clone()];
        all_box.extend(num_items.iter().cloned());
        all_box.extend(den_items.iter().cloned());
        let (x, y, width, height) = union_bbox(&all_box);
        let num = layout_items(num_items, depth);
        let den = layout_items(den_items, depth);
        let latex = format!("\\frac{{{num}}}{{{den}}}");

        let mut next: Vec<Item> = items.into_iter().enumerate().filter(|(i, _)| !consumed[*i]).map(|(_, it)| it).collect();
        next.push(Item { raw_text: latex.clone(), latex, x, y, width, height, is_composite: true });
        items = next;
    }
    items
}

/// Detects bar-less vertically stacked fractions (common when Vision omits thin ink bars):
/// two horizontally aligned bands separated vertically, either flanked by a middle-baseline neighbor
/// (e.g. `y = ` beside `1` over `2`) or forming the only two tightly stacked expressions without `=`.
fn collapse_implicit_fractions(mut items: Vec<Item>, depth: usize) -> Vec<Item> {
    loop {
        if items.len() < 2 {
            break;
        }
        let med_h = median_height(&items);
        let mut found: Option<(Vec<usize>, Vec<usize>)> = None;

        for i in 0..items.len() {
            for j in 0..items.len() {
                if i == j {
                    continue;
                }
                let top = &items[i];
                let bot = &items[j];
                if top.bottom() > bot.y + med_h * 0.15 {
                    continue;
                }
                let vgap = (bot.y - top.bottom()).max(0.0);
                if vgap > med_h * 1.4 {
                    continue;
                }
                let min_w = top.width.min(bot.width).max(1e-4);
                let overlap = x_overlap(top, bot);
                let centers_aligned = (top.x_center() - bot.x_center()).abs() <= top.width.max(bot.width) * 0.4;
                if overlap < min_w * 0.45 && !centers_aligned {
                    continue;
                }
                if top.raw_text.contains('=') || bot.raw_text.contains('=') {
                    continue;
                }
                if is_big_operator(&top.latex) || is_big_operator(&bot.latex) {
                    continue;
                }

                // Collect any sibling items sharing the same top/bottom row within the column.
                let col_left = top.x.min(bot.x) - med_h * 0.35;
                let col_right = top.right().max(bot.right()) + med_h * 0.35;
                let mid_y = (top.bottom() + bot.y) * 0.5;
                let mut num_idxs = Vec::new();
                let mut den_idxs = Vec::new();
                let mut has_mid_neighbor = false;

                for (k, other) in items.iter().enumerate() {
                    let in_col = other.x_center() >= col_left && other.x_center() <= col_right;
                    if in_col && other.y_center() < mid_y && (mid_y - other.bottom()) <= med_h * 1.4 {
                        num_idxs.push(k);
                    } else if in_col && other.y_center() > mid_y && (other.y - mid_y) <= med_h * 1.4 {
                        den_idxs.push(k);
                    } else if !in_col && other.y_center() >= top.y_center() && other.y_center() <= bot.y_center() {
                        has_mid_neighbor = true;
                    }
                }

                // A taller non-operator base immediately to the left (e.g. `x` before small `2`
                // above and small `i` below) is simultaneous superscript + subscript (`x_{i}^{2}`),
                // not a bar-less fraction (`x \frac{2}{i}`).
                let stack_left = top.x.min(bot.x);
                let is_dual_scripts_of_base = items.iter().enumerate().any(|(k, left)| {
                    !num_idxs.contains(&k)
                        && !den_idxs.contains(&k)
                        && left.right() <= stack_left + left.height * 0.35
                        && (stack_left - left.right()) <= left.height * 0.65
                        && !ends_with_operator(&left.latex)
                        && top.height <= left.height * 0.78
                        && bot.height <= left.height * 0.78
                        && left.y_center() >= top.y_center()
                        && left.y_center() <= bot.y_center()
                });
                if is_dual_scripts_of_base {
                    continue;
                }

                let all_covered = num_idxs.len() + den_idxs.len() == items.len();
                let tight_solo = all_covered && vgap <= med_h * 0.9;
                if !num_idxs.is_empty() && !den_idxs.is_empty() && (has_mid_neighbor || tight_solo) {
                    found = Some((num_idxs, den_idxs));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        let Some((num_idxs, den_idxs)) = found else { break };
        let mut consumed = vec![false; items.len()];
        let mut num_items = Vec::new();
        let mut den_items = Vec::new();
        for &k in &num_idxs {
            consumed[k] = true;
            num_items.push(items[k].clone());
        }
        for &k in &den_idxs {
            consumed[k] = true;
            den_items.push(items[k].clone());
        }
        let mut all_box = num_items.clone();
        all_box.extend(den_items.iter().cloned());
        let (x, y, width, height) = union_bbox(&all_box);
        let num = layout_items(num_items, depth);
        let den = layout_items(den_items, depth);
        let latex = format!("\\frac{{{num}}}{{{den}}}");

        let mut next: Vec<Item> = items.into_iter().enumerate().filter(|(k, _)| !consumed[*k]).map(|(_, it)| it).collect();
        next.push(Item { raw_text: latex.clone(), latex, x, y, width, height, is_composite: true });
        items = next;
    }
    items
}

fn ends_with_operator(latex: &str) -> bool {
    let trimmed = latex.trim();
    is_plain_operator(trimmed) || trimmed.ends_with(['+', '-', '=', '<', '>', ',', ';', ':', '('])
}

/// Splits items into distinct horizontal reading lines when vertically separated by more than
/// normal superscript/subscript offsets.
fn split_into_lines(mut items: Vec<Item>) -> Vec<Vec<Item>> {
    let med_h = median_height(&items);
    items.sort_by(|a, b| a.y_center().total_cmp(&b.y_center()).then_with(|| a.x.total_cmp(&b.x)));

    let mut lines: Vec<Vec<Item>> = Vec::new();
    for it in items {
        let placed = lines.iter_mut().find(|line| {
            let (mut top, mut bot) = (f32::MAX, f32::MIN);
            for member in line.iter() {
                top = top.min(member.y);
                bot = bot.max(member.bottom());
            }
            let vertical_overlap = it.bottom().min(bot) - it.y.max(top);
            let gap = if vertical_overlap >= 0.0 { 0.0 } else { -vertical_overlap };
            gap <= med_h * 0.65
        });
        match placed {
            Some(line) => line.push(it),
            None => lines.push(vec![it]),
        }
    }
    for line in &mut lines {
        // On equal `x`, place lower items (subscripts) before upper items (superscripts): `x_{i}^{2}`.
        line.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| b.y.total_cmp(&a.y)));
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Super,
    Sub,
}

#[derive(Clone, Debug)]
struct Anchor {
    y_top: f32,
    y_bottom: f32,
    y_center: f32,
    height: f32,
    right: f32,
    can_script: bool,
}

impl Anchor {
    fn from_item(it: &Item) -> Self {
        Self {
            y_top: it.y,
            y_bottom: it.bottom(),
            y_center: it.y_center(),
            height: it.height,
            right: it.right(),
            can_script: !is_plain_operator(&it.latex),
        }
    }
}

fn classify_script(anchor: &Anchor, cur: &Item) -> Option<ScriptKind> {
    if !anchor.can_script || is_plain_operator(&cur.latex) {
        return None;
    }
    let hgap = cur.x - anchor.right;
    if hgap > anchor.height * 1.4 || cur.x + cur.width * 0.5 < anchor.right - anchor.height * 1.2 {
        return None;
    }
    let smaller = cur.height <= anchor.height * 0.85;
    let is_sup = cur.y_center() < anchor.y_center - anchor.height * 0.20
        && cur.bottom() < anchor.y_bottom - anchor.height * 0.15
        && (smaller || cur.bottom() <= anchor.y_center + anchor.height * 0.12);
    if is_sup {
        return Some(ScriptKind::Super);
    }
    let is_sub = cur.y_center() > anchor.y_center + anchor.height * 0.20
        && cur.y > anchor.y_top + anchor.height * 0.15
        && (smaller || cur.y >= anchor.y_center - anchor.height * 0.12);
    if is_sub {
        return Some(ScriptKind::Sub);
    }
    None
}

fn render_line(items: Vec<Item>) -> String {
    let mut out = String::new();
    let mut anchor: Option<Anchor> = None;
    let mut active_script: Option<(ScriptKind, String, f32)> = None;

    let flush_script = |out: &mut String, active: &mut Option<(ScriptKind, String, f32)>| {
        if let Some((kind, buf, _)) = active.take() {
            let trimmed = buf.trim();
            if !trimmed.is_empty() {
                match kind {
                    ScriptKind::Super => out.push_str(&format!("^{{{trimmed}}}")),
                    ScriptKind::Sub => out.push_str(&format!("_{{{trimmed}}}")),
                }
            }
        }
    };

    for it in items {
        if let Some(ref mut anch) = anchor
            && let Some(kind) = classify_script(anch, &it)
        {
            match &mut active_script {
                Some((cur_kind, buf, script_right)) if *cur_kind == kind && (it.x - *script_right) <= anch.height * 1.0 => {
                    append_token(buf, &it.latex);
                    *script_right = script_right.max(it.right());
                    anch.right = anch.right.max(it.right());
                }
                _ => {
                    flush_script(&mut out, &mut active_script);
                    let right = it.right();
                    anch.right = anch.right.max(right);
                    active_script = Some((kind, it.latex, right));
                }
            }
            continue;
        }
        flush_script(&mut out, &mut active_script);
        append_token(&mut out, &it.latex);
        anchor = Some(Anchor::from_item(&it));
    }
    flush_script(&mut out, &mut active_script);
    out.trim().to_string()
}

fn append_token(out: &mut String, token: &str) {
    let tok = token.trim();
    if tok.is_empty() {
        return;
    }
    if out.is_empty() {
        out.push_str(tok);
        return;
    }
    let last = out.chars().last();
    let first = tok.chars().next();
    let needs_space = !matches!((last, first), (Some(' '), _) | (_, Some(',' | ';' | ':' | ')' | ']' | '}')) | (Some('(' | '[' | '{'), _));
    if needs_space {
        out.push(' ');
    }
    out.push_str(tok);
}

fn unicode_sup_char(c: char) -> Option<char> {
    match c {
        '⁰' => Some('0'),
        '¹' => Some('1'),
        '²' => Some('2'),
        '³' => Some('3'),
        '⁴' => Some('4'),
        '⁵' => Some('5'),
        '⁶' => Some('6'),
        '⁷' => Some('7'),
        '⁸' => Some('8'),
        '⁹' => Some('9'),
        '⁺' => Some('+'),
        '⁻' => Some('-'),
        'ⁿ' => Some('n'),
        'ⁱ' => Some('i'),
        _ => None,
    }
}

fn unicode_sub_char(c: char) -> Option<char> {
    match c {
        '₀' => Some('0'),
        '₁' => Some('1'),
        '₂' => Some('2'),
        '₃' => Some('3'),
        '₄' => Some('4'),
        '₅' => Some('5'),
        '₆' => Some('6'),
        '₇' => Some('7'),
        '₈' => Some('8'),
        '₉' => Some('9'),
        '₊' => Some('+'),
        '₋' => Some('-'),
        _ => None,
    }
}

fn unicode_symbol_to_latex(c: char) -> Option<&'static str> {
    Some(match c {
        // Binary operators & relations
        '×' => "\\times",
        '÷' => "\\div",
        '±' => "\\pm",
        '∓' => "\\mp",
        '·' | '⋅' => "\\cdot",
        '≤' => "\\le",
        '≥' => "\\ge",
        '≠' => "\\ne",
        '≈' => "\\approx",
        '≡' => "\\equiv",
        '∝' => "\\propto",
        '∼' => "\\sim",
        '≅' => "\\cong",
        '≪' => "\\ll",
        '≫' => "\\gg",
        '−' => "-",
        // Calculus, sets, logic, arrows, misc
        '∞' => "\\infty",
        '∂' => "\\partial",
        '∇' => "\\nabla",
        '∑' => "\\sum",
        '∏' => "\\prod",
        '∫' => "\\int",
        '∮' => "\\oint",
        '∈' => "\\in",
        '∉' => "\\notin",
        '⊂' => "\\subset",
        '⊃' => "\\supset",
        '⊆' => "\\subseteq",
        '⊇' => "\\supseteq",
        '∪' => "\\cup",
        '∩' => "\\cap",
        '∅' => "\\emptyset",
        '∀' => "\\forall",
        '∃' => "\\exists",
        '¬' => "\\neg",
        '∧' => "\\land",
        '∨' => "\\lor",
        '→' => "\\to",
        '←' => "\\leftarrow",
        '↔' => "\\leftrightarrow",
        '⇒' => "\\Rightarrow",
        '⇐' => "\\Leftarrow",
        '⇔' => "\\Leftrightarrow",
        '°' => "^\\circ",
        '′' => "'",
        '…' => "\\ldots",
        '⋯' => "\\cdots",
        '⊥' => "\\perp",
        '∥' => "\\parallel",
        '∠' => "\\angle",
        '△' => "\\triangle",
        // Lowercase Greek
        'α' => "\\alpha",
        'β' => "\\beta",
        'γ' => "\\gamma",
        'δ' => "\\delta",
        'ε' | 'ϵ' => "\\epsilon",
        'ζ' => "\\zeta",
        'η' => "\\eta",
        'θ' | 'ϑ' => "\\theta",
        'ι' => "\\iota",
        'κ' => "\\kappa",
        'λ' => "\\lambda",
        'μ' => "\\mu",
        'ν' => "\\nu",
        'ξ' => "\\xi",
        'π' | 'ϖ' => "\\pi",
        'ρ' | 'ϱ' => "\\rho",
        'σ' | 'ς' => "\\sigma",
        'τ' => "\\tau",
        'υ' => "\\upsilon",
        'φ' | 'ϕ' => "\\phi",
        'χ' => "\\chi",
        'ψ' => "\\psi",
        'ω' => "\\omega",
        // Uppercase Greek
        'Γ' => "\\Gamma",
        'Δ' => "\\Delta",
        'Θ' => "\\Theta",
        'Λ' => "\\Lambda",
        'Ξ' => "\\Xi",
        'Π' => "\\Pi",
        'Σ' => "\\Sigma",
        'Υ' => "\\Upsilon",
        'Φ' => "\\Phi",
        'Ψ' => "\\Psi",
        'Ω' => "\\Omega",
        _ => return None,
    })
}

/// Normalizes Unicode mathematical operators, Greek symbols, radicals, and superscript/subscript
/// characters into clean LaTeX.
#[must_use]
pub fn normalize_math_text(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 16);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Unicode superscript runs (e.g. `²` -> `^2`, `²³` -> `^{23}`).
        if let Some(first_sup) = unicode_sup_char(c) {
            let mut run = String::new();
            run.push(first_sup);
            i += 1;
            while i < chars.len() {
                if let Some(next_sup) = unicode_sup_char(chars[i]) {
                    run.push(next_sup);
                    i += 1;
                } else {
                    break;
                }
            }
            if run.len() == 1 {
                out.push('^');
                out.push_str(&run);
            } else {
                out.push_str(&format!("^{{{run}}}"));
            }
            continue;
        }

        // Unicode subscript runs (e.g. `₁` -> `_1`, `₁₂` -> `_{12}`).
        if let Some(first_sub) = unicode_sub_char(c) {
            let mut run = String::new();
            run.push(first_sub);
            i += 1;
            while i < chars.len() {
                if let Some(next_sub) = unicode_sub_char(chars[i]) {
                    run.push(next_sub);
                    i += 1;
                } else {
                    break;
                }
            }
            if run.len() == 1 {
                out.push('_');
                out.push_str(&run);
            } else {
                out.push_str(&format!("_{{{run}}}"));
            }
            continue;
        }

        // Radical sign `√`: wrap a parenthesized argument `√(a+b)` -> `\sqrt{a+b}` or a single
        // immediately following token `√x` -> `\sqrt{x}` / `√2` -> `\sqrt{2}`.
        if c == '√' {
            i += 1;
            if i < chars.len() && chars[i] == '(' {
                let mut depth = 0usize;
                let start = i + 1;
                let mut end = None;
                let mut j = i;
                while j < chars.len() {
                    if chars[j] == '(' {
                        depth += 1;
                    } else if chars[j] == ')' {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    j += 1;
                }
                if let Some(close) = end {
                    let inner: String = chars[start..close].iter().collect();
                    out.push_str(&format!("\\sqrt{{{}}}", normalize_math_text(&inner)));
                    i = close + 1;
                    continue;
                }
            } else if i < chars.len() && !chars[i].is_whitespace() {
                let start = i;
                if chars[i].is_ascii_digit() {
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
                let inner: String = chars[start..i].iter().collect();
                out.push_str(&format!("\\sqrt{{{}}}", normalize_math_text(&inner)));
                continue;
            }
            out.push_str("\\sqrt");
            continue;
        }

        if let Some(macro_str) = unicode_symbol_to_latex(c) {
            out.push_str(macro_str);
            // Prevent a LaTeX macro like `\alpha` from fusing with a following ASCII letter.
            if macro_str.starts_with('\\') && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
                out.push(' ');
            }
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(text: &str, x: f32, y: f32, w: f32, h: f32) -> VisionObservation {
        VisionObservation::new(text, x, y, w, h, 0.99)
    }

    #[test]
    fn empty_and_invalid_observations_produce_empty_string() {
        assert_eq!(observations_to_latex(&[]), "");
        assert_eq!(observations_to_latex(&[obs("   ", 0.1, 0.1, 0.2, 0.1)]), "");
        assert_eq!(observations_to_latex(&[obs("x", f32::NAN, 0.1, 0.2, 0.1)]), "");
        assert_eq!(observations_to_latex(&[obs("x", 0.1, 0.1, 0.0, 0.1)]), "");
    }

    #[test]
    fn bottom_left_conversion_flips_y_axis() {
        let converted = VisionObservation::from_bottom_left("2", 0.2, 0.7, 0.1, 0.1, 0.95);
        assert!((converted.y - 0.2).abs() < 1e-5);
        assert_eq!(converted.x, 0.2);
        assert_eq!(converted.width, 0.1);
        assert_eq!(converted.height, 0.1);
    }

    #[test]
    fn normalizes_unicode_math_and_greek_symbols() {
        assert_eq!(normalize_math_text("a × b ÷ c ± d"), "a \\times b \\div c \\pm d");
        assert_eq!(normalize_math_text("x ≤ y ≥ z ≠ 0"), "x \\le y \\ge z \\ne 0");
        assert_eq!(normalize_math_text("α+β+γ = πr²"), "\\alpha+\\beta+\\gamma = \\pi r^2");
        assert_eq!(normalize_math_text("x³ + y²³ + a₁ + b₁₂"), "x^3 + y^{23} + a_1 + b_{12}");
        assert_eq!(normalize_math_text("∫ ∞ √x + √(x+1)"), "\\int \\infty \\sqrt{x} + \\sqrt{x+1}");
        assert_eq!(normalize_math_text("∑ Δθ → Ω"), "\\sum \\Delta\\theta \\to \\Omega");
        assert_eq!(normalize_math_text("√"), "\\sqrt");
    }

    #[test]
    fn reconstructs_fraction_with_explicit_horizontal_bar() {
        let input = vec![obs("a + b", 0.30, 0.15, 0.40, 0.14), obs("—", 0.25, 0.34, 0.50, 0.04), obs("2c", 0.40, 0.44, 0.20, 0.14)];
        assert_eq!(observations_to_latex(&input), "\\frac{a + b}{2c}");
    }

    #[test]
    fn reconstructs_fraction_in_equation_without_consuming_minus_sign() {
        let input = vec![
            obs("y", 0.05, 0.30, 0.08, 0.16),
            obs("=", 0.16, 0.32, 0.08, 0.12),
            obs("1", 0.32, 0.15, 0.10, 0.14),
            obs("-", 0.28, 0.34, 0.18, 0.04),
            obs("2", 0.32, 0.44, 0.10, 0.14),
            obs("-", 0.52, 0.36, 0.08, 0.04),
            obs("x", 0.64, 0.30, 0.08, 0.16),
        ];
        assert_eq!(observations_to_latex(&input), "y = \\frac{1}{2} - x");
    }

    #[test]
    fn reconstructs_barless_vertically_stacked_fraction() {
        let input = vec![obs("y =", 0.05, 0.32, 0.18, 0.16), obs("x", 0.35, 0.18, 0.12, 0.14), obs("2", 0.35, 0.42, 0.12, 0.14)];
        assert_eq!(observations_to_latex(&input), "y = \\frac{x}{2}");
    }

    #[test]
    fn reconstructs_superscript_and_subscript_from_2d_geometry() {
        let input = vec![
            obs("x", 0.10, 0.40, 0.10, 0.20),
            obs("2", 0.21, 0.32, 0.06, 0.11),
            obs("+", 0.32, 0.43, 0.08, 0.14),
            obs("a", 0.44, 0.40, 0.10, 0.20),
            obs("i", 0.55, 0.52, 0.05, 0.11),
        ];
        assert_eq!(observations_to_latex(&input), "x^{2} + a_{i}");
    }

    #[test]
    fn reconstructs_simultaneous_subscript_and_superscript() {
        let input = vec![obs("x", 0.10, 0.40, 0.10, 0.22), obs("i", 0.21, 0.54, 0.05, 0.11), obs("2", 0.21, 0.30, 0.06, 0.11)];
        assert_eq!(observations_to_latex(&input), "x_{i}^{2}");
    }

    #[test]
    fn reconstructs_big_operator_limits() {
        let input = vec![
            obs("∑", 0.15, 0.35, 0.14, 0.22),
            obs("n", 0.19, 0.18, 0.06, 0.10),
            obs("i=1", 0.14, 0.62, 0.16, 0.10),
            obs("k", 0.34, 0.38, 0.08, 0.16),
            obs("2", 0.43, 0.30, 0.05, 0.09),
        ];
        assert_eq!(observations_to_latex(&input), "\\sum_{i=1}^{n} k^{2}");
    }

    #[test]
    fn wide_8_to_1_equation_does_not_absorb_distant_terms_as_scripts() {
        // Coordinates on an 800x100 (8:1) image:
        // - `x` at x=40..60px (0.05..0.075), y=35..75px (h=0.40)
        // - real superscript `2` right beside `x` at x=64..78px (0.08..0.0975), y=20..42px
        // - distant slightly-raised `y` way over at x=240..260px (0.30..0.325), y=22..46px
        let input = vec![obs("x", 0.05, 0.35, 0.025, 0.40), obs("2", 0.08, 0.20, 0.0175, 0.22), obs("y", 0.30, 0.22, 0.025, 0.24)];
        // Without aspect-ratio scaling, normalized x distances are compressed 8x so `y` is wrongly
        // captured into the superscript of `x`.
        assert_eq!(observations_to_latex(&input), "x^{2 y}");
        // With the true 8:1 aspect ratio, `2` stays a superscript while distant `y` stays separate.
        assert_eq!(observations_to_latex_with_aspect(&input, 8.0), "x^{2} y");
    }

    #[test]
    fn observations_to_text_groups_lines_and_preserves_paragraph_breaks() {
        let input = vec![
            obs("   ", 0.01, 0.01, 0.10, 0.05),
            obs("bad", f32::NAN, 0.10, 0.10, 0.05),
            obs("New paragraph", 0.05, 0.40, 0.40, 0.06),
            obs("world", 0.30, 0.105, 0.18, 0.06),
            obs("Hello", 0.05, 0.10, 0.20, 0.06),
            obs("line.", 0.30, 0.192, 0.15, 0.06),
            obs("Second", 0.05, 0.19, 0.22, 0.06),
        ];
        assert_eq!(observations_to_text(&input), "Hello world\nSecond line.\n\nNew paragraph");
        assert_eq!(observations_to_text(&[]), "");
    }
}
