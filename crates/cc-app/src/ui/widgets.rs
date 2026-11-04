use freya::prelude::*;

/// A titled card container.
pub fn card(title: &str) -> Rect {
    rect()
        .width(Size::fill())
        .padding(16.)
        .spacing(10.)
        .corner_radius(10.)
        .background((30, 30, 37))
        .child(
            label()
                .text(title.to_string())
                .font_size(15.)
                .font_weight(FontWeight::BOLD),
        )
}

pub fn muted(text: impl Into<String>) -> Label {
    label()
        .text(text.into())
        .font_size(13.)
        .color((150, 150, 165))
}

/// Small coloured status pill.
pub fn badge(text: &str, color: (u8, u8, u8)) -> Rect {
    rect()
        .padding((3., 10.))
        .corner_radius(99.)
        .background(color)
        .child(
            label()
                .text(text.to_string())
                .font_size(12.)
                .color((15, 15, 18)),
        )
}

/// A labelled row: `title` on the left, `control` on the right.
pub fn setting_row(title: &str, description: &str, control: impl IntoElement) -> Rect {
    rect()
        .horizontal()
        .content(Content::flex())
        .width(Size::fill())
        .spacing(16.)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::flex(1.))
                .spacing(2.)
                .child(label().text(title.to_string()).font_size(14.))
                .child(muted(description)),
        )
        .child(control)
}
