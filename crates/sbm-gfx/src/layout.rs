use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.w)
            && y < self.y.saturating_add(self.h)
    }

    pub fn union(self, other: Self) -> Self {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self
            .x
            .saturating_add(self.w)
            .max(other.x.saturating_add(other.w));
        let y1 = self
            .y
            .saturating_add(self.h)
            .max(other.y.saturating_add(other.h));
        Self {
            x: x0,
            y: y0,
            w: x1.saturating_sub(x0),
            h: y1.saturating_sub(y0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub font: u32,
    pub header: Rect,
    pub footer: Rect,
    pub cards: Vec<(usize, Rect)>,
    pub actions: Vec<Rect>,
    pub cols: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Card(usize),
    Action(usize),
    None,
}

pub fn font_px(height: u32) -> u32 {
    if height >= 2000 {
        36
    } else if height >= 1400 {
        28
    } else if height >= 1000 {
        22
    } else if height >= 700 {
        16
    } else {
        14
    }
}

pub fn layout_menu(
    width: u32,
    height: u32,
    n_cards: usize,
    n_actions: usize,
    selected: usize,
) -> Layout {
    let font = font_px(height).max(8);
    let margin = font;
    let gap = (font / 2).max(4);
    let header_h = font.saturating_mul(3).min(height / 5).max(font);
    let footer_h = font.saturating_mul(2).min(height / 8).max(font / 2);
    let action_h = font.saturating_mul(2).saturating_add(gap);
    let header = Rect {
        x: 0,
        y: 0,
        w: width,
        h: header_h.min(height),
    };
    let footer = Rect {
        x: 0,
        y: height.saturating_sub(footer_h.min(height)),
        w: width,
        h: footer_h.min(height),
    };
    let action_y = footer.y.saturating_sub(gap.saturating_add(action_h));
    let content_top = header.h.saturating_add(gap);
    let content_bottom = action_y.saturating_sub(gap).max(content_top);
    let content_h = content_bottom.saturating_sub(content_top).max(font);
    let content_w = width.saturating_sub(margin.saturating_mul(2)).max(font);

    let card_h = font
        .saturating_mul(5)
        .min(content_h)
        .max(font.saturating_mul(3));
    let min_card_w = font.saturating_mul(12).max(1);
    let mut cols = ((content_w.saturating_add(gap)) / min_card_w.saturating_add(gap)).max(1);
    while cols > 1 && content_w / cols < min_card_w {
        cols -= 1;
    }
    let cols = cols as usize;
    let rows_fit = ((content_h.saturating_add(gap)) / card_h.saturating_add(gap)).max(1) as usize;
    let visible = cols.saturating_mul(rows_fit).max(1);
    let selected = if n_cards == 0 {
        0
    } else {
        selected.min(n_cards - 1)
    };
    let start = if n_cards == 0 {
        0
    } else {
        (selected / visible) * visible
    };
    let end = (start + visible).min(n_cards);
    let shown = end - start;
    let used_cols = if shown == 0 {
        1
    } else {
        cols.min(shown).max(1)
    };
    let card_w = if used_cols <= 1 {
        content_w
            .min(font.saturating_mul(22))
            .max(min_card_w.min(content_w))
    } else {
        let gaps = gap.saturating_mul(used_cols as u32 - 1);
        content_w.saturating_sub(gaps) / used_cols as u32
    };
    let rows = if shown == 0 {
        0
    } else {
        shown.div_ceil(used_cols)
    };
    let grid_w = if used_cols == 0 {
        0
    } else {
        (used_cols as u32)
            .saturating_mul(card_w)
            .saturating_add(gap.saturating_mul(used_cols as u32 - 1))
    };
    let grid_h = if rows == 0 {
        0
    } else {
        (rows as u32)
            .saturating_mul(card_h)
            .saturating_add(gap.saturating_mul(rows as u32 - 1))
    };
    let origin_x = margin.saturating_add(content_w.saturating_sub(grid_w) / 2);
    let origin_y = content_top.saturating_add(content_h.saturating_sub(grid_h) / 2);
    let mut cards = Vec::with_capacity(shown);
    for index in 0..shown {
        let col = index % used_cols;
        let row = index / used_cols;
        cards.push((
            start + index,
            Rect {
                x: origin_x.saturating_add((col as u32).saturating_mul(card_w.saturating_add(gap))),
                y: origin_y.saturating_add((row as u32).saturating_mul(card_h.saturating_add(gap))),
                w: card_w,
                h: card_h,
            },
        ));
    }

    let mut actions = Vec::with_capacity(n_actions);
    if n_actions > 0 && width > 0 {
        let action_w =
            (content_w / n_actions as u32).clamp(font.saturating_mul(6), font.saturating_mul(12));
        let total = (n_actions as u32)
            .saturating_mul(action_w)
            .saturating_add(gap.saturating_mul(n_actions as u32 - 1));
        let origin = margin.saturating_add(content_w.saturating_sub(total.min(content_w)) / 2);
        for index in 0..n_actions {
            actions.push(Rect {
                x: origin
                    .saturating_add((index as u32).saturating_mul(action_w.saturating_add(gap))),
                y: action_y,
                w: action_w,
                h: action_h.min(height.saturating_sub(action_y)),
            });
        }
    }

    Layout {
        font,
        header,
        footer,
        cards,
        actions,
        cols: used_cols,
    }
}

pub fn hit_test(layout: &Layout, x: u32, y: u32) -> Hit {
    for (index, rect) in &layout.cards {
        if rect.contains(x, y) {
            return Hit::Card(*index);
        }
    }
    for (index, rect) in layout.actions.iter().enumerate() {
        if rect.contains(x, y) {
            return Hit::Action(index);
        }
    }
    Hit::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(screen_w: u32, screen_h: u32, rect: Rect) -> bool {
        rect.w > 0
            && rect.h > 0
            && rect.x.saturating_add(rect.w) <= screen_w
            && rect.y.saturating_add(rect.h) <= screen_h
    }

    #[test]
    fn selected_card_is_visible_and_on_screen() {
        for (w, h, n, selected) in [
            (1024, 768, 6, 5),
            (1920, 1080, 3, 2),
            (800, 600, 8, 7),
            (3840, 2160, 4, 0),
        ] {
            let layout = layout_menu(w, h, n, 3, selected);
            assert!(
                layout.cards.iter().any(|(index, _)| *index == selected),
                "{w}x{h}"
            );
            for (_, rect) in &layout.cards {
                assert!(inside(w, h, *rect), "{w}x{h} {rect:?}");
            }
            for rect in &layout.actions {
                assert!(inside(w, h, *rect), "{w}x{h} action {rect:?}");
            }
            let selected_rect = layout
                .cards
                .iter()
                .find(|(index, _)| *index == selected)
                .unwrap()
                .1;
            let hit = hit_test(
                &layout,
                selected_rect.x + selected_rect.w / 2,
                selected_rect.y + selected_rect.h / 2,
            );
            assert_eq!(hit, Hit::Card(selected));
        }
    }
}
