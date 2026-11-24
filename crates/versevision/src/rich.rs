//! A small piece of Python's `rich` (15.x): exactly what the VerseVision command line (and typer's help pages) use, so the text it prints
//! is the text the Python app prints, byte for byte, when the output is not a terminal (and the same layout with colours when it is).
//!
//! Covered: console markup (`[red]x[/red]`), word wrapping with `fold`/`ellipsis`/`crop` overflow, `Table` (the default heavy-head box and
//! the borderless grids typer uses), `Panel` (rounded), `Padding`, `Columns`, and the pretty printer behind `console.print(dict)`.
//! NOT covered: syntax highlighting of numbers, strings and paths (it only colours; the text is unchanged), emoji codes, right/centre
//! justification, tab expansion, hyperlinks, and styles other than bold/dim and the eight basic colours.

use unicode_width::UnicodeWidthChar;

// ---------------------------------------------------------------------------------------------------------------- style

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub dim: bool,
    /// 0 = none, 1..=7 = red, green, yellow, blue, magenta, cyan, white
    pub fg: u8,
}

impl Style {
    pub const NONE: Style = Style {
        bold: false,
        dim: false,
        fg: 0,
    };

    /// `"bold cyan"`, `"dim red"`, `"red"`, `""`. Names rich knows but this does not draw are ignored (as an unknown style is).
    pub fn parse(spec: &str) -> Style {
        let mut style = Style::NONE;
        for word in spec.split_whitespace() {
            match word {
                "bold" => style.bold = true,
                "dim" => style.dim = true,
                "red" => style.fg = 1,
                "green" => style.fg = 2,
                "yellow" => style.fg = 3,
                "blue" => style.fg = 4,
                "magenta" => style.fg = 5,
                "cyan" => style.fg = 6,
                "white" => style.fg = 7,
                _ => {}
            }
        }
        style
    }

    pub fn over(self, base: Style) -> Style {
        Style {
            bold: self.bold || base.bold,
            dim: self.dim || base.dim,
            fg: if self.fg != 0 { self.fg } else { base.fg },
        }
    }

    fn sgr(self) -> String {
        let mut codes: Vec<String> = Vec::new();
        if self.bold {
            codes.push("1".into());
        }
        if self.dim {
            codes.push("2".into());
        }
        if self.fg != 0 {
            codes.push(format!("{}", 30 + u32::from(self.fg) - 1));
        }
        codes.join(";")
    }
}

/// Styled text: one `(char, style)` per character.
pub type Cells = Vec<(char, Style)>;

pub fn cells_of(text: &str, style: Style) -> Cells {
    text.chars().map(|c| (c, style)).collect()
}

pub fn plain(cells: &[(char, Style)]) -> String {
    cells.iter().map(|(c, _)| *c).collect()
}

pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn cell_len_str(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

fn cell_len(cells: &[(char, Style)]) -> usize {
    cells.iter().map(|(c, _)| char_width(*c)).sum()
}

// -------------------------------------------------------------------------------------------------------------- markup

/// `rich.markup.render`: `[style]text[/style]`, `[/]`, `\[` escapes. A tag is `[` + a letter, `#`, `/` or `@` + anything up to `]`.
/// A tag that does not name a style rich knows still disappears from the text (its style is simply not drawn).
pub fn markup(text: &str, base: Style) -> Cells {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Cells = Vec::new();
    // open tags: (normalised name, style); a closing tag closes the last open tag of that name
    let mut stack: Vec<(String, Style)> = Vec::new();
    let current = |stack: &Vec<(String, Style)>| stack.iter().fold(base, |acc, (_, s)| s.over(acc));
    let mut i = 0;
    while i < chars.len() {
        // a run of backslashes directly before a '[' that opens a tag escapes it
        if chars[i] == '\\' {
            let mut j = i;
            while j < chars.len() && chars[j] == '\\' {
                j += 1;
            }
            if j < chars.len() && chars[j] == '[' && tag_end(&chars, j).is_some() {
                let count = j - i;
                for _ in 0..count / 2 {
                    out.push(('\\', current(&stack)));
                }
                if count % 2 == 1 {
                    // escaped: the tag is literal text
                    let end = tag_end(&chars, j).unwrap_or(j);
                    for c in &chars[j..=end] {
                        out.push((*c, current(&stack)));
                    }
                    i = end + 1;
                } else {
                    i = j;
                }
                continue;
            }
            for c in &chars[i..j] {
                out.push((*c, current(&stack)));
            }
            i = j;
            continue;
        }
        if chars[i] == '[' {
            if let Some(end) = tag_end(&chars, i) {
                let inner: String = chars[i + 1..end].iter().collect();
                if let Some(rest) = inner.strip_prefix('/') {
                    let name = rest.trim().to_lowercase();
                    if name.is_empty() {
                        stack.pop();
                    } else if let Some(position) = stack.iter().rposition(|(n, _)| *n == name) {
                        stack.remove(position);
                    }
                } else {
                    stack.push((inner.trim().to_lowercase(), Style::parse(&inner)));
                }
                i = end + 1;
                continue;
            }
        }
        out.push((chars[i], current(&stack)));
        i += 1;
    }
    out
}

fn tag_end(chars: &[char], start: usize) -> Option<usize> {
    // chars[start] == '['; the next character must be a-z, '#', '/' or '@', and no '[' may come before the ']'
    let first = *chars.get(start + 1)?;
    if !(first.is_ascii_lowercase() || first == '#' || first == '/' || first == '@') {
        return None;
    }
    let mut j = start + 2;
    while j < chars.len() {
        match chars[j] {
            ']' => return Some(j),
            '[' => return None,
            _ => j += 1,
        }
    }
    None
}

/// `rich.markup.escape`.
pub fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' && tag_end(&chars, i).is_some() {
            // backslashes before the tag are doubled, then the tag gets one
            let trailing = out.chars().rev().take_while(|c| *c == '\\').count();
            for _ in 0..trailing {
                out.push('\\');
            }
            out.push('\\');
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// -------------------------------------------------------------------------------------------------------------- wrapping

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    Fold,
    Crop,
    Ellipsis,
}

fn is_space(c: char) -> bool {
    c.is_whitespace()
}

/// `rich._wrap.words`: `\s*\S+\s*` pieces as (start, end) char offsets.
fn words(chars: &[(char, Style)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let start = i;
        while i < n && is_space(chars[i].0) {
            i += 1;
        }
        if i >= n {
            // only whitespace left: rich's regex needs a non-space, so this tail is not a word
            break;
        }
        while i < n && !is_space(chars[i].0) {
            i += 1;
        }
        while i < n && is_space(chars[i].0) {
            i += 1;
        }
        out.push((start, i));
    }
    out
}

fn chop_cells(chars: &[(char, Style)], width: usize) -> Vec<Cells> {
    // `rich.cells.chop_cells`: pieces of at most `width` cells
    let mut lines: Vec<Cells> = Vec::new();
    let mut current: Cells = Vec::new();
    let mut used = 0;
    for &(c, s) in chars {
        let w = char_width(c);
        if used + w > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push((c, s));
        used += w;
    }
    lines.push(current);
    lines
}

/// `rich._wrap.divide_line`: the offsets at which a line is cut so every piece fits `width` cells.
fn divide_line(text: &[(char, Style)], width: usize, fold: bool) -> Vec<usize> {
    let mut breaks = Vec::new();
    let mut offset = 0usize;
    for (start, end) in words(text) {
        let word = &text[start..end];
        let trimmed_len = {
            let mut e = word.len();
            while e > 0 && is_space(word[e - 1].0) {
                e -= 1;
            }
            cell_len(&word[..e])
        };
        let remaining = width as isize - offset as isize;
        if remaining >= trimmed_len as isize {
            offset += cell_len(word);
        } else if trimmed_len > width {
            if fold {
                let folded = chop_cells(word, width);
                let mut start_pos = start;
                let count = folded.len();
                for (index, line) in folded.iter().enumerate() {
                    if start_pos > 0 {
                        breaks.push(start_pos);
                    }
                    if index + 1 == count {
                        offset = cell_len(line);
                    } else {
                        start_pos += line.len();
                    }
                }
            } else {
                if start > 0 {
                    breaks.push(start);
                }
                offset = cell_len(word);
            }
        } else if offset > 0 && start > 0 {
            breaks.push(start);
            offset = cell_len(word);
        }
    }
    breaks
}

fn truncate(mut line: Cells, width: usize, overflow: Overflow) -> Cells {
    if cell_len(&line) <= width {
        return line;
    }
    let keep = if overflow == Overflow::Ellipsis {
        width.saturating_sub(1)
    } else {
        width
    };
    let mut used = 0;
    let mut cut = 0;
    for (index, (c, _)) in line.iter().enumerate() {
        let w = char_width(*c);
        if used + w > keep {
            break;
        }
        used += w;
        cut = index + 1;
    }
    line.truncate(cut);
    if overflow == Overflow::Ellipsis && width > 0 {
        let style = line.last().map_or(Style::NONE, |(_, s)| *s);
        line.push(('…', style));
    }
    line
}

/// `Text.wrap`: the lines of a text at `width` cells.
pub fn wrap(text: &[(char, Style)], width: usize, overflow: Overflow, no_wrap: bool) -> Vec<Cells> {
    let mut result: Vec<Cells> = Vec::new();
    for raw in split_lines_keep_blank(text) {
        let pieces: Vec<Cells> = if no_wrap {
            vec![raw]
        } else {
            let offsets = divide_line(&raw, width, overflow == Overflow::Fold);
            let mut pieces = Vec::new();
            let mut last = 0;
            for offset in offsets {
                pieces.push(raw[last..offset].to_vec());
                last = offset;
            }
            pieces.push(raw[last..].to_vec());
            pieces
        };
        for mut piece in pieces {
            // rstrip_end: whitespace beyond the width goes
            let len = piece.len();
            if len > width {
                let excess = len - width;
                let trailing = piece.iter().rev().take_while(|(c, _)| is_space(*c)).count();
                let cut = trailing.min(excess);
                piece.truncate(len - cut);
            }
            result.push(truncate(piece, width, overflow));
        }
    }
    result
}

fn split_lines_keep_blank(text: &[(char, Style)]) -> Vec<Cells> {
    let mut lines: Vec<Cells> = vec![Vec::new()];
    for &(c, s) in text {
        if c == '\n' {
            lines.push(Vec::new());
        } else {
            lines.last_mut().expect("one line").push((c, s));
        }
    }
    lines
}

// ------------------------------------------------------------------------------------------------------- ratio helpers

fn round_half_even(x: f64) -> i64 {
    let floor = x.floor();
    let diff = x - floor;
    if (diff - 0.5).abs() < 1e-12 {
        let f = floor as i64;
        if f % 2 == 0 {
            f
        } else {
            f + 1
        }
    } else {
        x.round() as i64
    }
}

fn ratio_reduce(total: i64, ratios: &[i64], maximums: &[i64], values: &[i64]) -> Vec<i64> {
    let ratios: Vec<i64> = ratios
        .iter()
        .zip(maximums)
        .map(|(r, m)| if *m != 0 { *r } else { 0 })
        .collect();
    let mut total_ratio: i64 = ratios.iter().sum();
    if total_ratio == 0 {
        return values.to_vec();
    }
    let mut remaining = total;
    let mut result = Vec::new();
    for ((ratio, maximum), value) in ratios.iter().zip(maximums).zip(values) {
        if *ratio != 0 && total_ratio > 0 {
            let distributed = (*maximum).min(round_half_even(
                *ratio as f64 * remaining as f64 / total_ratio as f64,
            ));
            result.push(value - distributed);
            remaining -= distributed;
            total_ratio -= ratio;
        } else {
            result.push(*value);
        }
    }
    result
}

fn ratio_distribute(total: i64, ratios: &[i64], minimums: Option<&[i64]>) -> Vec<i64> {
    let ratios: Vec<i64> = match minimums {
        Some(mins) => ratios
            .iter()
            .zip(mins)
            .map(|(r, m)| if *m != 0 { *r } else { 0 })
            .collect(),
        None => ratios.to_vec(),
    };
    let mut total_ratio: i64 = ratios.iter().sum();
    let mut remaining = total;
    let mut out = Vec::new();
    for (index, ratio) in ratios.iter().enumerate() {
        let minimum = minimums.map_or(0, |m| m[index]);
        let distributed = if total_ratio > 0 {
            minimum.max((*ratio as f64 * remaining as f64 / total_ratio as f64).ceil() as i64)
        } else {
            remaining
        };
        out.push(distributed);
        total_ratio -= ratio;
        remaining -= distributed;
    }
    out
}

// ------------------------------------------------------------------------------------------------------- renderables

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    None,
    HeavyHead,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub header: Cells,
    pub cells: Vec<R>,
    pub style: Style,
    pub no_wrap: bool,
    pub width: Option<usize>,
    pub ratio: Option<usize>,
    pub overflow: Overflow,
}

impl Column {
    pub fn new(header: &str) -> Column {
        Column {
            header: cells_of(header, Style::NONE),
            cells: Vec::new(),
            style: Style::NONE,
            no_wrap: false,
            width: None,
            ratio: None,
            overflow: Overflow::Ellipsis,
        }
    }
    fn flexible(&self) -> bool {
        self.ratio.is_some_and(|r| r > 0)
    }
}

#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<Column>,
    pub show_header: bool,
    pub header_style: Style,
    pub box_kind: BoxKind,
    pub padding: [usize; 4], // top, right, bottom, left
    pub pad_edge: bool,
    pub collapse_padding: bool,
    pub expand: bool,
}

#[derive(Debug, Clone)]
pub enum R {
    Text {
        cells: Cells,
        overflow: Overflow,
        no_wrap: bool,
    },
    Table(Box<Table>),
    Panel {
        inner: Box<R>,
        title: String,
        border: Style,
    },
    Padding(Box<R>, [usize; 4]),
    /// `rich.columns.Columns` with the default padding: items side by side while they fit
    Columns(Vec<R>),
    /// Pre-rendered lines (a group of renderables, one under the other, each as wide as it is)
    Group(Vec<R>),
    /// `Align(renderable, pad=False)`: the content is wrapped at its own widest line (at most the width)
    Align(Box<R>),
}

pub fn text(markup_text: &str) -> R {
    R::Text {
        cells: markup(markup_text, Style::NONE),
        overflow: Overflow::Fold,
        no_wrap: false,
    }
}

pub fn text_styled(markup_text: &str, style: &str) -> R {
    R::Text {
        cells: markup(markup_text, Style::parse(style)),
        overflow: Overflow::Fold,
        no_wrap: false,
    }
}

impl Table {
    pub fn new(headers: &[&str]) -> Table {
        Table {
            columns: headers.iter().map(|h| Column::new(h)).collect(),
            show_header: true,
            header_style: Style::parse("bold"),
            box_kind: BoxKind::HeavyHead,
            padding: [0, 1, 0, 1],
            pad_edge: true,
            collapse_padding: false,
            expand: false,
        }
    }

    /// A table without a box, header or edge padding (what typer builds for its help panels and `Columns` builds inside).
    pub fn plain(columns: usize) -> Table {
        Table {
            columns: (0..columns).map(|_| Column::new("")).collect(),
            show_header: false,
            header_style: Style::NONE,
            box_kind: BoxKind::None,
            padding: [0, 1, 0, 1],
            pad_edge: false,
            collapse_padding: false,
            expand: false,
        }
    }

    pub fn add_row(&mut self, cells: Vec<R>) {
        for (column, cell) in self.columns.iter_mut().zip(cells) {
            column.cells.push(cell);
        }
    }

    /// A row of markup strings.
    pub fn add_row_text(&mut self, cells: &[&str]) {
        self.add_row(cells.iter().map(|c| text(c)).collect());
    }

    fn row_count(&self) -> usize {
        self.columns.first().map_or(0, |c| c.cells.len())
    }

    fn extra_width(&self) -> usize {
        let n = self.columns.len();
        match self.box_kind {
            BoxKind::None => 0,
            BoxKind::HeavyHead => n + 1,
        }
    }

    fn padding_width(&self, index: usize) -> usize {
        let [_, right, _, left] = self.padding;
        let (mut left, mut right) = (left, right);
        if self.collapse_padding {
            left = 0;
            right = left.abs_diff(right);
        }
        if !self.pad_edge {
            if index == 0 {
                left = 0;
            }
            if index + 1 == self.columns.len() {
                right = 0;
            }
        }
        left + right
    }

    /// (left, right) padding of the cells of column `index`.
    fn cell_padding(&self, index: usize) -> (usize, usize) {
        let [_, right, _, left] = self.padding;
        let (mut left, right) = (left, right);
        if self.collapse_padding && index != 0 {
            left = left.saturating_sub(right);
        }
        let mut right = right;
        if !self.pad_edge {
            if index == 0 {
                left = 0;
            }
            if index + 1 == self.columns.len() {
                right = 0;
            }
        }
        (left, right)
    }

    fn measure_column(&self, index: usize, max_width: usize) -> (usize, usize) {
        let column = &self.columns[index];
        if max_width < 1 {
            return (0, 0);
        }
        let pad = self.padding_width(index);
        if let Some(width) = column.width {
            let w = (width + pad).min(max_width);
            return (w, w);
        }
        let mut min_w = 0;
        let mut max_w = 0;
        let mut any = false;
        let mut consider = |item: &R| {
            let (lo, hi) = measure(item, max_width.saturating_sub(pad));
            let (lo, hi) = (lo + pad, hi + pad);
            let (lo, hi) = (lo.min(max_width), hi.min(max_width));
            min_w = min_w.max(lo);
            max_w = max_w.max(hi);
            any = true;
        };
        if self.show_header {
            consider(&R::Text {
                cells: column.header.clone(),
                overflow: column.overflow,
                no_wrap: false,
            });
        }
        for cell in &column.cells {
            consider(cell);
        }
        if !any {
            return (1, max_width);
        }
        (min_w, max_w)
    }

    fn calculate_widths(&self, max_width: usize) -> Vec<usize> {
        let ranges: Vec<(usize, usize)> = (0..self.columns.len())
            .map(|i| self.measure_column(i, max_width))
            .collect();
        let mut widths: Vec<i64> = ranges
            .iter()
            .map(|r| if r.1 == 0 { 1 } else { r.1 as i64 })
            .collect();
        if self.expand {
            let ratios: Vec<i64> = self
                .columns
                .iter()
                .filter(|c| c.flexible())
                .map(|c| c.ratio.unwrap_or(0) as i64)
                .collect();
            if ratios.iter().any(|r| *r != 0) {
                let fixed: Vec<i64> = ranges
                    .iter()
                    .zip(&self.columns)
                    .map(|(r, c)| if c.flexible() { 0 } else { r.1 as i64 })
                    .collect();
                let flex_minimum: Vec<i64> = self
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.flexible())
                    .map(|(i, c)| c.width.unwrap_or(1) as i64 + self.padding_width(i) as i64)
                    .collect();
                let flexible_width = max_width as i64 - fixed.iter().sum::<i64>();
                let flex_widths = ratio_distribute(flexible_width, &ratios, Some(&flex_minimum));
                let mut iter = flex_widths.into_iter();
                for (index, column) in self.columns.iter().enumerate() {
                    if column.flexible() {
                        widths[index] = fixed[index] + iter.next().unwrap_or(0);
                    }
                }
            }
        }
        let mut table_width: i64 = widths.iter().sum();
        if table_width > max_width as i64 {
            let wrapable: Vec<bool> = self
                .columns
                .iter()
                .map(|c| c.width.is_none() && !c.no_wrap)
                .collect();
            widths = collapse_widths(widths, &wrapable, max_width as i64);
            table_width = widths.iter().sum();
            if table_width > max_width as i64 {
                let excess = table_width - max_width as i64;
                let ones = vec![1; widths.len()];
                widths = ratio_reduce(excess, &ones, &widths, &widths);
            }
            let ranges: Vec<(usize, usize)> = widths
                .iter()
                .enumerate()
                .map(|(i, w)| self.measure_column(i, (*w).max(0) as usize))
                .collect();
            widths = ranges.iter().map(|r| r.1 as i64).collect();
            table_width = widths.iter().sum();
        }
        if table_width < max_width as i64 && self.expand {
            let pad = ratio_distribute(max_width as i64 - table_width, &widths, None);
            widths = widths.iter().zip(pad).map(|(w, p)| w + p).collect();
        }
        widths.iter().map(|w| (*w).max(0) as usize).collect()
    }

    fn render(&self, max_width: usize) -> Vec<Cells> {
        if self.columns.is_empty() {
            return vec![Vec::new()];
        }
        let extra = self.extra_width();
        let widths = self.calculate_widths(max_width.saturating_sub(extra));
        let rows = self.row_count();
        let mut lines: Vec<Cells> = Vec::new();
        let (tl, tr, bl, br) = ('┏', '┓', '└', '┘');
        let boxed = self.box_kind == BoxKind::HeavyHead;
        let rule = |left: char, mid: char, right: char, fill: char| -> Cells {
            let mut out: Cells = vec![(left, Style::NONE)];
            for (index, w) in widths.iter().enumerate() {
                if index > 0 {
                    out.push((mid, Style::NONE));
                }
                for _ in 0..*w {
                    out.push((fill, Style::NONE));
                }
            }
            out.push((right, Style::NONE));
            out
        };
        if boxed {
            lines.push(rule(tl, '┳', tr, '━'));
        }
        let render_row = |cells: Vec<(&R, Style)>, head: bool| -> Vec<Cells> {
            let mut rendered: Vec<Vec<Cells>> = Vec::new();
            for (index, (cell, style)) in cells.iter().enumerate() {
                let (pl, pr) = self.cell_padding(index);
                let inner_w = widths[index].saturating_sub(pl + pr);
                let mut cell_lines = render_lines(cell, inner_w, *style);
                for line in &mut cell_lines {
                    let mut padded: Cells = Vec::new();
                    for _ in 0..pl {
                        padded.push((' ', Style::NONE));
                    }
                    padded.append(line);
                    for _ in 0..pr {
                        padded.push((' ', Style::NONE));
                    }
                    *line = padded;
                }
                rendered.push(cell_lines);
            }
            let height = rendered.iter().map(Vec::len).max().unwrap_or(1).max(1);
            let mut out = Vec::new();
            let vertical = if boxed && head { '┃' } else { '│' };
            for row_line in 0..height {
                let mut line: Cells = Vec::new();
                if boxed {
                    line.push((vertical, Style::NONE));
                }
                for (index, cell_lines) in rendered.iter().enumerate() {
                    if index > 0 && boxed {
                        line.push((vertical, Style::NONE));
                    }
                    let mut piece = cell_lines.get(row_line).cloned().unwrap_or_default();
                    let have = cell_len(&piece);
                    for _ in have..widths[index] {
                        piece.push((' ', Style::NONE));
                    }
                    line.extend(piece);
                }
                if boxed {
                    line.push((vertical, Style::NONE));
                }
                out.push(line);
            }
            out
        };
        if self.show_header {
            let header_cells: Vec<R> = self
                .columns
                .iter()
                .map(|c| R::Text {
                    cells: c.header.clone(),
                    overflow: c.overflow,
                    no_wrap: false,
                })
                .collect();
            let row: Vec<(&R, Style)> = header_cells
                .iter()
                .map(|c| (c, self.header_style))
                .collect();
            lines.extend(render_row(row, true));
            if boxed {
                lines.push(rule('┡', '╇', '┩', '━'));
            }
        }
        for row_index in 0..rows {
            let row: Vec<(&R, Style)> = self
                .columns
                .iter()
                .map(|c| (&c.cells[row_index], c.style))
                .collect();
            lines.extend(render_row(row, false));
        }
        if boxed {
            lines.push(rule(bl, '┴', br, '─'));
        }
        lines
    }
}

fn collapse_widths(mut widths: Vec<i64>, wrapable: &[bool], max_width: i64) -> Vec<i64> {
    let mut total: i64 = widths.iter().sum();
    let mut excess = total - max_width;
    if wrapable.iter().any(|w| *w) {
        while total != 0 && excess > 0 {
            let max_column = widths
                .iter()
                .zip(wrapable)
                .filter(|(_, w)| **w)
                .map(|(x, _)| *x)
                .max()
                .unwrap_or(0);
            let second = widths
                .iter()
                .zip(wrapable)
                .map(|(x, w)| if *w && *x != max_column { *x } else { 0 })
                .max()
                .unwrap_or(0);
            let difference = max_column - second;
            let ratios: Vec<i64> = widths
                .iter()
                .zip(wrapable)
                .map(|(x, w)| i64::from(*x == max_column && *w))
                .collect();
            if ratios.iter().all(|r| *r == 0) || difference == 0 {
                break;
            }
            let max_reduce = vec![excess.min(difference); widths.len()];
            widths = ratio_reduce(excess, &ratios, &max_reduce, &widths);
            total = widths.iter().sum();
            excess = total - max_width;
        }
    }
    widths
}

/// `Measurement.get`: (minimum, maximum) width of a renderable given the space there is.
pub fn measure(item: &R, max_width: usize) -> (usize, usize) {
    let (lo, hi) = measure_raw(item, max_width);
    (lo.min(max_width), hi.min(max_width))
}

fn measure_raw(item: &R, max_width: usize) -> (usize, usize) {
    match item {
        R::Text { cells, .. } => {
            let plain = plain(cells);
            let max_text = plain.lines().map(cell_len_str).max().unwrap_or(0);
            let min_text = plain
                .split_whitespace()
                .map(cell_len_str)
                .max()
                .unwrap_or(max_text);
            (min_text, max_text)
        }
        R::Padding(inner, pad) => {
            let extra = pad[1] + pad[3];
            if max_width < extra + 1 {
                return (max_width, max_width);
            }
            let (lo, hi) = measure(inner, max_width - extra);
            ((lo + extra).min(max_width), (hi + extra).min(max_width))
        }
        R::Align(inner) => measure(inner, max_width),
        R::Group(items) => {
            let mut lo = 0;
            let mut hi = 0;
            for item in items {
                let (a, b) = measure(item, max_width);
                lo = lo.max(a);
                hi = hi.max(b);
            }
            (lo, hi)
        }
        // rich has no measurement for these: the whole width, minimum 0
        R::Columns(_) | R::Table(_) | R::Panel { .. } => (0, max_width),
    }
}

/// `console.render_lines(renderable, width, pad=True)`: every line exactly `width` cells (cropped or padded).
pub fn render_lines(item: &R, width: usize, base: Style) -> Vec<Cells> {
    let mut lines = render(item, width);
    for line in &mut lines {
        if base != Style::NONE {
            for cell in line.iter_mut() {
                cell.1 = cell.1.over(base);
            }
        }
        let have = cell_len(line);
        if have > width {
            *line = truncate(std::mem::take(line), width, Overflow::Crop);
        } else {
            for _ in have..width {
                line.push((' ', base));
            }
        }
    }
    lines
}

/// The lines a renderable makes at `width` (not padded unless the renderable pads itself).
pub fn render(item: &R, width: usize) -> Vec<Cells> {
    match item {
        R::Text {
            cells,
            overflow,
            no_wrap,
        } => wrap(cells, width, *overflow, *no_wrap),
        R::Table(table) => table.render(width),
        R::Padding(inner, pad) => {
            let [top, right, bottom, left] = *pad;
            let inner_w = width.saturating_sub(left + right);
            let mut out: Vec<Cells> = Vec::new();
            let blank = || vec![(' ', Style::NONE); width];
            for _ in 0..top {
                out.push(blank());
            }
            for line in render_lines(inner, inner_w, Style::NONE) {
                let mut full: Cells = vec![(' ', Style::NONE); left];
                full.extend(line);
                full.extend(vec![(' ', Style::NONE); right]);
                out.push(full);
            }
            for _ in 0..bottom {
                out.push(blank());
            }
            out
        }
        R::Panel {
            inner,
            title,
            border,
        } => {
            let inner_w = width.saturating_sub(4);
            let mut out: Vec<Cells> = Vec::new();
            let b = |c: char| (c, *border);
            let mut top: Cells = vec![b('╭'), b('─')];
            if title.is_empty() || width <= 4 {
                top = vec![b('╭')];
                top.extend(std::iter::repeat_n(b('─'), width.saturating_sub(2)));
            } else {
                let title_text = format!(" {title} ");
                let tw = cell_len_str(&title_text);
                for c in title_text.chars() {
                    top.push((c, Style::NONE));
                }
                let room = (width - 4).saturating_sub(tw);
                top.extend(std::iter::repeat_n(b('─'), room));
                top.push(b('─'));
            }
            top.push(b('╮'));
            out.push(top);
            for line in render_lines(inner, inner_w, Style::NONE) {
                let mut full: Cells = vec![b('│'), (' ', Style::NONE)];
                full.extend(line);
                full.push((' ', Style::NONE));
                full.push(b('│'));
                out.push(full);
            }
            let mut bottom: Cells = vec![b('╰')];
            bottom.extend(std::iter::repeat_n(b('─'), width.saturating_sub(2)));
            bottom.push(b('╯'));
            out.push(bottom);
            out
        }
        R::Align(inner) => {
            let (_, max) = measure(inner, width);
            let w = max.min(width);
            let lines = render(inner, w);
            let shape = lines.iter().map(|l| cell_len(l)).max().unwrap_or(0);
            lines
                .into_iter()
                .map(|mut l| {
                    for _ in cell_len(&l)..shape {
                        l.push((' ', Style::NONE));
                    }
                    l
                })
                .collect()
        }
        R::Group(items) => items.iter().flat_map(|i| render(i, width)).collect(),
        R::Columns(items) => render_columns(items, width),
    }
}

fn render_columns(items: &[R], max_width: usize) -> Vec<Cells> {
    if items.is_empty() {
        return Vec::new();
    }
    let widths: Vec<usize> = items.iter().map(|i| measure(i, max_width).1).collect();
    let padding = 1usize; // Columns' default padding (0, 1)
    let mut column_count = items.len();
    while column_count > 1 {
        let mut col_widths: Vec<usize> = vec![0; column_count];
        let mut seen = 0usize;
        let mut column_no = 0usize;
        let mut shrunk = false;
        for (index, w) in widths.iter().enumerate() {
            let _ = index;
            col_widths[column_no] = col_widths[column_no].max(*w);
            seen = seen.max(column_no + 1);
            let total: usize = col_widths[..seen].iter().sum::<usize>() + padding * (seen - 1);
            if total > max_width {
                column_count = seen - 1;
                shrunk = true;
                break;
            }
            column_no = (column_no + 1) % column_count;
        }
        if !shrunk {
            break;
        }
    }
    let mut table = Table::plain(column_count.max(1));
    table.collapse_padding = true;
    let mut row: Vec<R> = Vec::new();
    for item in items {
        row.push(item.clone());
        if row.len() == column_count {
            table.add_row(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        while row.len() < column_count {
            row.push(R::Text {
                cells: Vec::new(),
                overflow: Overflow::Fold,
                no_wrap: false,
            });
        }
        table.add_row(row);
    }
    table.render(max_width)
}

// -------------------------------------------------------------------------------------------------------------- console

#[derive(Debug, Clone)]
pub struct Console {
    pub width: usize,
    pub color: bool,
}

impl Console {
    pub fn new(width: usize, color: bool) -> Console {
        Console { width, color }
    }

    /// `console.print(markup_text)` as the text that goes to the stream.
    pub fn print(&self, markup_text: &str) -> String {
        self.print_r(&text(markup_text))
    }

    /// `console.print(markup_text, style=...)`.
    pub fn print_styled(&self, markup_text: &str, style: &str) -> String {
        self.print_r(&text_styled(markup_text, style))
    }

    pub fn print_r(&self, item: &R) -> String {
        let lines = render(item, self.width);
        let mut out = String::new();
        for line in lines {
            out.push_str(&self.line_text(&line));
            out.push('\n');
        }
        out
    }

    pub fn line_text(&self, line: &[(char, Style)]) -> String {
        if !self.color {
            return plain(line);
        }
        let mut out = String::new();
        let mut index = 0;
        while index < line.len() {
            let style = line[index].1;
            let mut end = index;
            while end < line.len() && line[end].1 == style {
                end += 1;
            }
            let chunk: String = line[index..end].iter().map(|(c, _)| *c).collect();
            if style == Style::NONE {
                out.push_str(&chunk);
            } else {
                out.push_str(&format!("\x1b[{}m{}\x1b[0m", style.sgr(), chunk));
            }
            index = end;
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------- pretty

/// What `rich.pretty` prints for a value: Python's `repr` laid out over several lines when it does not fit `width`.
#[derive(Debug, Clone)]
pub enum Pretty {
    Scalar(String),
    List(Vec<Pretty>),
    Dict(Vec<(String, Pretty)>),
}

impl Pretty {
    fn one_line(&self) -> String {
        match self {
            Pretty::Scalar(s) => s.clone(),
            Pretty::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Pretty::one_line)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Pretty::Dict(items) => format!(
                "{{{}}}",
                items
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.one_line()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The lines at `width` cells (`rich.pretty.pretty_repr`, indent 4, no maximum depth/length).
    pub fn lines(&self, width: usize) -> Vec<String> {
        let mut out = Vec::new();
        self.expand(width, 0, "", &mut out, "");
        out
    }

    fn expand(
        &self,
        width: usize,
        level: usize,
        prefix: &str,
        out: &mut Vec<String>,
        suffix: &str,
    ) {
        let indent = " ".repeat(level * 4);
        let flat = format!("{indent}{prefix}{}{suffix}", self.one_line());
        // rich measures the one-line form of the node at its position; a trailing comma counts
        if cell_len_str(&flat) <= width || matches!(self, Pretty::Scalar(_)) {
            out.push(flat);
            return;
        }
        match self {
            Pretty::Scalar(_) => {}
            Pretty::List(items) if items.is_empty() => {
                out.push(format!("{indent}{prefix}[]{suffix}"))
            }
            Pretty::Dict(items) if items.is_empty() => {
                out.push(format!("{indent}{prefix}{{}}{suffix}"))
            }
            Pretty::List(items) => {
                out.push(format!("{indent}{prefix}["));
                for item in items {
                    item.expand(width, level + 1, "", out, ",");
                }
                out.push(format!("{indent}]{suffix}"));
            }
            Pretty::Dict(items) => {
                out.push(format!("{indent}{prefix}{{"));
                for (key, value) in items {
                    value.expand(width, level + 1, &format!("{key}: "), out, ",");
                }
                out.push(format!("{indent}}}{suffix}"));
            }
        }
    }
}

/// Python's `repr` of a JSON-shaped value, as the nodes of the pretty printer.
pub fn pretty_of(value: &serde_json::Value) -> Pretty {
    use serde_json::Value;
    match value {
        Value::Null => Pretty::Scalar("None".into()),
        Value::Bool(b) => Pretty::Scalar(if *b { "True".into() } else { "False".into() }),
        Value::Number(n) => Pretty::Scalar(match n.as_f64() {
            Some(f) if n.is_f64() => crate::pyfloat(f),
            _ => n.to_string(),
        }),
        Value::String(s) => Pretty::Scalar(versevision_pp::pyrepr::py_repr(s)),
        Value::Array(items) => Pretty::List(items.iter().map(pretty_of).collect()),
        Value::Object(map) => Pretty::Dict(
            map.iter()
                .map(|(k, v)| (versevision_pp::pyrepr::py_repr(k), pretty_of(v)))
                .collect(),
        ),
    }
}
